//! The static-light bake's STRUCTURAL layer: scene snapshot types, region
//! sizing, atlas packing, and the encode conventions. The per-texel work —
//! sun rays, lamp rays, the chamfer SDF, the shadow-top rays — runs on the
//! GPU as fragment passes (gpu_lightmap.rs); this module is the contract
//! both sides of that boundary agree on.
//!
//! The output is one RGBA8 atlas over the whole static scene:
//!
//! * **A** — sun visibility as a signed-distance field: 128 is the shadow
//!   boundary, above is lit, below is shadowed, in steps of the SDF band.
//!   The shader reconstructs a soft penumbra with one `smoothstep`, so shadow
//!   softness is a runtime knob and the bake stores only WHERE the edge is.
//!   Storing distance rather than coverage is what lets a coarse texel grid
//!   draw a clean anti-aliased shadow line — the "soft outlined iso line".
//! * **RGB** — incident light from placed lamps, encoded ×127.5 so 1.0 sits
//!   at half range and lamps can go 2× overbright, exactly the Quake
//!   convention.
//!
//! Sunlight itself is NOT stored in RGB: the shader already computes the
//! analytic `sun_color × N·L` term per pixel, so the map only needs to gate
//! it. That keeps the sun's colour and intensity live-tunable without a
//! rebake, and keeps RGB free for the lamps.
//!
//! # Parameterisation: no new vertex data
//!
//! Every baked model already carries a chart parameterisation — its `ao_uv`
//! into its own shrink-wrapped AO texture. The lightmap REUSES it: each
//! placed instance gets a rectangle of the atlas shaped like the model's AO
//! layout, and the shader remaps `ao_uv` into it with one per-instance
//! offset/scale vec4. Terrain needs no vertex data at all — a heightfield is
//! exactly a planar map, so its tiles get rectangles addressed straight from
//! world xz.

use crate::ao::MeshRaycaster;
use makepad_draw::makepad_math::{Mat4f, Vec3f, Vec4f};
use std::sync::Arc;

/// Lightmap texels per texel of a model's own AO layout. The AO bake packs at
/// ~64 texels/unit; a quarter of that (16/unit, ~6cm) is where shadow edges
/// stop visibly stair-stepping once the SDF smooths them.
pub const LM_MESH_SCALE: f32 = 0.5;

/// Lightmap texels per world unit on planar receivers (terrain). Coarser than
/// models on purpose: the ground is where the SDF has the most room to work,
/// and terrain tiles are by far the largest regions in the atlas.
pub const LM_PLANAR_TEXELS_PER_UNIT: f32 = 4.0;

/// Signed-distance band, in texels, encoded into A. ±band maps to 0..255
/// around 128. Wider survives more smoothing in the shader; narrower resolves
/// finer double edges. 4 texels is 25cm on models, 1m on terrain.
pub const LM_SDF_BAND: f32 = 4.0;

/// Hard ceiling on the atlas. 2048² RGBA8 is 16MB — the single biggest GPU
/// object in the game, but it replaces every shadow triangle there was.
pub const LM_ATLAS_MAX: usize = 2048;

/// Empty texels of padding around each region, so bilinear at a region's rim
/// reads this region's dilated values and never a neighbour's.
const LM_PAD: usize = 1;

/// A placed static light. `radius` is where its contribution reaches zero —
/// finite on purpose, it is the bound that keeps lamp cost local.
#[derive(Clone, Debug)]
pub struct LmLight {
    pub pos: Vec3f,
    pub color: Vec3f,
    pub radius: f32,
    /// Emission axis for a spot-shaped light (street lamps point DOWN — an
    /// omni at the bulb lights the roof beside it brighter than the street
    /// below, which reads as the fixture shining upward).
    pub dir: Vec3f,
    /// 0 = omni. Otherwise how tightly emission hugs `dir`: the factor is
    /// `clamp((dot(to_texel, dir) + spill) / (1 + spill), 0, 1)` squared,
    /// so 1.0 gives a wide soft downlight with a little wall spill.
    pub spot: f32,
}

impl LmLight {
    /// An omnidirectional light.
    pub fn omni(pos: Vec3f, color: Vec3f, radius: f32) -> Self {
        LmLight { pos, color, radius, dir: Vec3f { x: 0.0, y: -1.0, z: 0.0 }, spot: 0.0 }
    }
}

/// Per-MODEL data the bake needs, shared by every placed copy. Everything is
/// MODEL space; instances add the transform. The GPU bake reads the bounds
/// and the chart shape — its "rays" are depth-map compares against the
/// model's rasterized GPU geometry.
pub struct LmMeshSource {
    /// Triangles + bounds. Positions/indices live inside.
    pub caster: MeshRaycaster,
    /// Per-vertex chart uv into the model's own AO layout, 0..1.
    pub ao_uv: Vec<[f32; 2]>,
    /// Per-vertex material rgb (a stand-in for albedo).
    pub albedo: Vec<Vec3f>,
    /// The model's AO texture size — the lightmap region is this shape,
    /// scaled by [`LM_MESH_SCALE`].
    pub ao_w: usize,
    pub ao_h: usize,
}

/// One placed copy of a model.
pub struct LmMeshInstance {
    pub source: Arc<LmMeshSource>,
    pub transform: Mat4f,
}

impl LmMeshInstance {
    /// World AABB of this copy (transformed model bounds).
    pub fn world_bounds(&self) -> (Vec3f, Vec3f) {
        world_bounds(&self.transform, self.source.caster.bounds())
    }
}

/// A terrain-tile snapshot: heights are copied so the bake owns its data.
/// `None` heights = a flat rectangle at `y`.
pub struct LmPlanar {
    pub x0: f32,
    pub z0: f32,
    pub x1: f32,
    pub z1: f32,
    pub y: f32,
    pub field: Option<LmHeightField>,
}

/// Heightfield sampling data. Terrain snapshots and the synthetic ground
/// field (terrain ∪ static box tops) both arrive as this. The sampling
/// semantics — bilinear over cell-clamped coordinates, central-difference
/// normals — live in the GPU gather shaders' `hf_h`/`hf_n` (shaders.rs).
pub struct LmHeightField {
    pub origin_x: f32,
    pub origin_z: f32,
    pub cell: f32,
    pub n: usize,
    pub heights: Arc<Vec<f32>>,
}

/// Everything the bake reads. Built on the main thread as a snapshot —
/// nothing here refers back to live renderer state.
pub struct LmScene {
    pub meshes: Vec<LmMeshInstance>,
    pub planars: Vec<LmPlanar>,
    /// Static world cubes as axis-aligned occluder boxes (they cast; they do
    /// not receive — receivers need a parameterisation; box roads receive
    /// through the synthetic ground field instead).
    pub boxes: Vec<(Vec3f, Vec3f)>,
    pub lights: Vec<LmLight>,
    /// Toward the sun, normalized (SunLight convention).
    pub sun_dir: Vec3f,
    /// Sun tint × intensity. The direct sun term stays analytic in the
    /// shader; this rides on the snapshot for tooling.
    pub sun_color: Vec3f,
    /// Reserved: the deleted CPU bake's bounce tier. The GPU pipeline does
    /// not implement bounce (yet); the flag stays on the snapshot contract.
    pub bounce: bool,
}

/// Where one receiver landed in the atlas, in texels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LmRect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl LmRect {
    /// The shader-side remap: `atlas_uv = offset + local_uv * scale`, as
    /// (offset_u, offset_v, scale_u, scale_v) normalized by atlas size.
    pub fn uv_remap(&self, atlas: usize) -> Vec4f {
        let s = 1.0 / atlas as f32;
        Vec4f {
            x: self.x as f32 * s,
            y: self.y as f32 * s,
            z: self.w as f32 * s,
            w: self.h as f32 * s,
        }
    }
}

/// The finished-atlas CONTRACT — the layout the GPU bake renders into and
/// the material shaders consume. `mesh_rects` and `planar_rects` are
/// parallel to the scene's lists. Kept as the documented reference for the
/// encode conventions even though the pixels now live only on the GPU:
///
/// * `pixels` is RGBA8 row-major, A = the sun-visibility SDF byte
///   (`128 + sd / LM_SDF_BAND * 127`), RGB = lamp light ×127.5.
/// * `top_pixels` is the shadow-top height plane, one byte per atlas texel
///   (same layout, planar regions only): for a SHADOWED ground-field texel,
///   the ABSOLUTE world height at which its sun ray was blocked, encoded
///   `(blocked_h - top_base) / top_range` in 0..254; 255 = lit / nothing
///   measured. A receiver ABOVE the blocker (a head over a fence rail, a
///   ramp top over a fence's grass shadow) rejects a shadow the flat field
///   could only express as "this xz is dark". Mesh regions never need it:
///   they bake ON their own surface, so bake height and receive height
///   agree by construction.
pub struct LmBaked {
    pub size: usize,
    /// RGBA8, row-major.
    pub pixels: Vec<u8>,
    pub top_pixels: Vec<u8>,
    pub top_base: f32,
    pub top_range: f32,
    pub mesh_rects: Vec<LmRect>,
    pub planar_rects: Vec<LmRect>,
}

/// Region sizing + shelf packing for a scene — the single source of truth
/// for the atlas LAYOUT. Rects come back in scene order: meshes first, then
/// planars. Deterministic: same scene, same layout.
pub fn plan_atlas(scene: &LmScene) -> (usize, Vec<LmRect>) {
    // Region sizes first, atlas fit second: try the shelf pack at each size,
    // shrinking everything by 15% until it fits, exactly like the AO fit
    // loop. Regions keep their aspect so the chart parameterisation is
    // uniformly scaled.
    let mut want: Vec<(usize, usize)> = Vec::new();
    for m in &scene.meshes {
        let w = ((m.source.ao_w as f32 * LM_MESH_SCALE).ceil() as usize).clamp(4, 512);
        let h = ((m.source.ao_h as f32 * LM_MESH_SCALE).ceil() as usize).clamp(4, 512);
        want.push((w, h));
    }
    for p in &scene.planars {
        // Adaptive density: small scenes take the fine end, a big world is
        // bounded by the 1024 region cap rather than silently cropped.
        let (sx, sz) = (p.x1 - p.x0, p.z1 - p.z0);
        let d = (1024.0 / sx.max(sz).max(1.0)).clamp(2.0, 4.0 * LM_PLANAR_TEXELS_PER_UNIT);
        let w = ((sx * d).ceil() as usize).clamp(4, 1024);
        let h = ((sz * d).ceil() as usize).clamp(4, 1024);
        want.push((w, h));
    }
    pack_regions(&want)
}

/// Shelf-pack all regions (padded), growing the atlas ×2 from 256 and then
/// shrinking every region 15% at a time until everything fits. Tallest
/// first, remapped back to input order.
fn pack_regions(want: &[(usize, usize)]) -> (usize, Vec<LmRect>) {
    let mut scale = 1.0f32;
    loop {
        let sized: Vec<(usize, usize)> = want
            .iter()
            .map(|(w, h)| {
                (
                    (((*w as f32 * scale) as usize).max(4)) + LM_PAD * 2,
                    (((*h as f32 * scale) as usize).max(4)) + LM_PAD * 2,
                )
            })
            .collect();
        let mut order: Vec<usize> = (0..sized.len()).collect();
        order.sort_by(|a, b| sized[*b].1.cmp(&sized[*a].1));
        let mut size = 256usize;
        'grow: loop {
            let (mut sx, mut sy, mut sh) = (0usize, 0usize, 0usize);
            let mut rects = vec![LmRect::default(); want.len()];
            let mut ok = true;
            for &i in &order {
                let (w, h) = sized[i];
                if w > size {
                    ok = false;
                    break;
                }
                if sx + w > size {
                    sx = 0;
                    sy += sh;
                    sh = 0;
                }
                if sy + h > size {
                    ok = false;
                    break;
                }
                rects[i] = LmRect {
                    x: sx + LM_PAD,
                    y: sy + LM_PAD,
                    w: w - LM_PAD * 2,
                    h: h - LM_PAD * 2,
                };
                sx += w;
                sh = sh.max(h);
            }
            if ok {
                return (size, rects);
            }
            if size < LM_ATLAS_MAX {
                size *= 2;
                continue 'grow;
            }
            break;
        }
        scale *= 0.85;
        // 4-texel minimum regions always fit a 2048 atlas long before this.
        assert!(scale > 0.01, "lightmap regions cannot fit the atlas");
    }
}

/// World AABB of a model box under a transform (8 transformed corners).
pub fn world_bounds(t: &Mat4f, (min, max): (Vec3f, Vec3f)) -> (Vec3f, Vec3f) {
    let mut lo = Vec3f { x: f32::MAX, y: f32::MAX, z: f32::MAX };
    let mut hi = Vec3f { x: f32::MIN, y: f32::MIN, z: f32::MIN };
    for i in 0..8 {
        let c = Vec4f {
            x: if i & 1 == 0 { min.x } else { max.x },
            y: if i & 2 == 0 { min.y } else { max.y },
            z: if i & 4 == 0 { min.z } else { max.z },
            w: 1.0,
        };
        let p = t.transform_vec4(c).to_vec3f();
        lo = Vec3f { x: lo.x.min(p.x), y: lo.y.min(p.y), z: lo.z.min(p.z) };
        hi = Vec3f { x: hi.x.max(p.x), y: hi.y.max(p.y), z: hi.z.max(p.z) };
    }
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_never_overlap() {
        let want = vec![(64, 32), (128, 128), (4, 4), (500, 20), (20, 500)];
        let (size, rects) = pack_regions(&want);
        assert!(size <= LM_ATLAS_MAX);
        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                let apart = a.x + a.w + LM_PAD <= b.x
                    || b.x + b.w + LM_PAD <= a.x
                    || a.y + a.h + LM_PAD <= b.y
                    || b.y + b.h + LM_PAD <= a.y;
                assert!(apart, "{:?} overlaps {:?}", a, b);
            }
            assert!(a.x + a.w <= size && a.y + a.h <= size);
        }
    }

    /// The pack is the one piece of layout math the GPU consumes verbatim —
    /// pin its determinism (same input, same rects), since the atlas remaps
    /// ship to shaders as plain floats.
    #[test]
    fn packing_is_deterministic() {
        let want = vec![(64, 32), (128, 128), (4, 4), (500, 20), (20, 500), (77, 33)];
        let a = pack_regions(&want);
        let b = pack_regions(&want);
        assert_eq!(a.0, b.0);
        assert_eq!(a.1, b.1);
    }
}
