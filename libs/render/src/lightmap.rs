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
//! * **RGB** — incident light from placed lamps, encoded so atlas 1.0 decodes
//!   to [`LM_LAMP_CEIL`] of light. The range is deliberately SMALL: a lamp
//!   pool is a fraction of the noon sun's 0.72 direct term, and an 8-bit
//!   channel stretched over 2× overbright spent 4/5 of its steps on light no
//!   fixture emits while the pool itself banded in the remaining fifth.
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

/// Atlas RGB decode: atlas 1.0 is this much light, and the material shaders
/// multiply by exactly this (`shaders.rs`, four `lm.xyz * (0.5 * has_lm)`
/// sites) after the bake gathers divided by it (`shaders.rs`, two
/// `s * 2.0` sites). It is the CEILING a baked lamp can reach, so it must sit
/// above what a fixture actually lays on the ground and not much further:
/// the accumulation target is RGBA8, so every doubling of the ceiling halves
/// the steps the pool's falloff has to work with.
///
/// 0.5 puts a street lamp's ~0.3 pool at byte 153 — the same encode
/// resolution the old ×2.0 ceiling gave the (four times too bright) pool it
/// was tuned around.
pub const LM_LAMP_CEIL: f32 = 0.5;

/// What a street lamp lays on the ground straight below it, in the shader's
/// light units (the noon sun's DIRECT term is 0.72, its ambient 0.28, so a
/// fully sunlit ground reads 1.0). A few hundred lumens over a pool metres
/// across is a FRACTION of daylight; the old bake put 0.87 here — brighter
/// than noon — which is the white pool this constant exists to prevent.
pub const LM_LAMP_GROUND_PEAK: f32 = 0.30;

/// The pool a street lamp is meant to lay ACROSS THE GROUND, measured from
/// the pole's foot. Together with the mount height this is the whole
/// photometry: the light's `radius` is `mount + LM_LAMP_POOL_RADIUS`, which
/// puts the fixture's reach one pool past its own bulb however high it hangs.
/// (The ground is lit slightly further than this — `sqrt(P² + 2·mount·P)` —
/// but by then the falloff has spent all but a few percent of the peak; a
/// taller lamp throwing a little further is what a taller lamp does.)
pub const LM_LAMP_POOL_RADIUS: f32 = 8.0;

/// Ceiling on a lamp's source strength, for fixtures mounted so high that
/// normalising the pool would ask for more than any bulb emits. Past ~10 m of
/// mount the pool dims instead of the source growing without bound.
pub const LM_LAMP_MAX_STRENGTH: f32 = 1.5;

/// The most lightmap texels ONE lamp may drive to the atlas ceiling.
///
/// Sized off the pool, not off taste: 1024 texels at
/// [`LM_LAMP_SAT_DENSITY`] is a disc of radius 1.13 m — 2% of the area of an
/// 8 m pool. A fixture is allowed to blow out its own collar; it is not
/// allowed to blow out the street. [`cap_lamp_pool`] enforces it.
pub const LM_LAMP_SAT_TEXELS: f32 = 1024.0;

/// Texel density the saturation budget is measured at: the FINEST a planar
/// region ever packs ([`plan_atlas`] clamps planar density to 4×
/// [`LM_PLANAR_TEXELS_PER_UNIT`]). Fixed on purpose — a lamp must not change
/// brightness because the world grew and its ground region coarsened.
pub const LM_LAMP_SAT_DENSITY: f32 = 4.0 * LM_PLANAR_TEXELS_PER_UNIT;

/// A downlight's finite reach and source strength, from the fixture's
/// geometry rather than from whatever scale its mesh was placed at.
///
/// `mount` is the height of the emitter above the ground it lights — the one
/// thing about a lamp that legitimately follows the placement transform,
/// because the bulb really does sit where the scaled mesh puts it. Reach and
/// strength do NOT: the reach is `mount + LM_LAMP_POOL_RADIUS` so the pool on
/// the street stays the same size, and the strength is solved so the ground
/// straight below reads exactly [`LM_LAMP_GROUND_PEAK`]. A lamp kit placed at
/// ×1, ×2 and ×8 then lights the same street; only the pole gets taller.
///
/// The inversion is exact for the gather's falloff (`att = 1 - d/radius`,
/// squared, with N·L = 1 and the spot cone = 1 straight below the bulb).
pub fn lamp_photometry(mount: f32) -> (f32, f32) {
    let mount = mount.max(0.05);
    let radius = mount + LM_LAMP_POOL_RADIUS;
    // att at the nadir: 1 - mount/radius, which is exactly pool/radius.
    let att = LM_LAMP_POOL_RADIUS / radius;
    let strength = (LM_LAMP_GROUND_PEAK / (att * att)).min(LM_LAMP_MAX_STRENGTH);
    (radius, strength)
}

/// How close to a bulb the bake stops believing occluders, as the lamp depth
/// pass's near plane. Bounds for [`lamp_clearance`]; the low end is the value
/// the pass used before it learned to scale.
pub const LM_LAMP_CLEARANCE_MIN: f32 = 0.25;
pub const LM_LAMP_CLEARANCE_MAX: f32 = 1.5;

/// The near plane for one lamp's shadow cube — how close to the bulb the bake
/// stops believing occluders.
///
/// A fixture's own housing is the one thing guaranteed to sit between its bulb
/// and the street, so the pass clips it with a near plane. A FIXED plane is a
/// scale trap, and the mirror image of the overbright pool — same
/// constant-in-world-units bug, opposite symptom: a Kenney lantern's housing
/// floor sits 6% of the pole below its bulb, which clears a 0.25 m plane at
/// ×1 and hides 0.66 m INSIDE it at ×8, where the lamp then shadows itself
/// and lights nothing at all. Scaling the plane with the fixture keeps a lamp
/// lighting its street at any placed scale, while a wall metres away still
/// casts.
pub fn lamp_clearance(light: &LmLight) -> f32 {
    let mount = (light.radius - LM_LAMP_POOL_RADIUS).max(0.0);
    (mount * 0.12).clamp(LM_LAMP_CLEARANCE_MIN, LM_LAMP_CLEARANCE_MAX)
}

/// Light this downlight lays on the ground `rho` metres out from the pole —
/// the bake gather's term (`shaders.rs` `lamp_term`) evaluated on a flat
/// plane `mount` below the bulb. The bake's authority stays the shader; this
/// is the same arithmetic on the CPU, for sizing and for tests.
pub fn lamp_ground_light(mount: f32, rho: f32, radius: f32, strength: f32, spot: f32) -> f32 {
    let d = (rho * rho + mount * mount).sqrt();
    if d >= radius || d < 1e-4 {
        return 0.0;
    }
    let att = 1.0 - d / radius;
    let ndl = mount / d;
    let cone = ((ndl + 0.35) / 1.35).clamp(0.0, 1.0);
    strength * ndl * att * att * (cone * cone * spot + (1.0 - spot))
}

/// Ground texels one lamp drives to the atlas ceiling, at `texels_per_unit`.
/// Zero for any fixture whose pool peak stays under [`LM_LAMP_CEIL`].
pub fn lamp_saturated_ground_texels(
    mount: f32,
    radius: f32,
    strength: f32,
    spot: f32,
    texels_per_unit: f32,
) -> f32 {
    if lamp_ground_light(mount, 0.0, radius, strength, spot) <= LM_LAMP_CEIL {
        return 0.0;
    }
    // Monotone in rho below the bulb: bisect for the crossing.
    let (mut lo, mut hi) = (0.0f32, radius);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if lamp_ground_light(mount, mid, radius, strength, spot) > LM_LAMP_CEIL {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    std::f32::consts::PI * lo * lo * texels_per_unit * texels_per_unit
}

/// The sanity rail on one baked lamp: if its pool would clip the atlas over
/// more than [`LM_LAMP_SAT_TEXELS`] ground texels, dim it until it does not.
/// Returns the factor applied — 1.0 when the lamp was already sane, which is
/// every lamp [`lamp_photometry`] sized.
///
/// [`lamp_photometry`] normalises the pool peak to a third of the ceiling, so
/// this never fires for a harvested fixture. It is the rail under
/// hand-authored static lights and under any future emitter class: one lamp
/// can no longer paint a plaza white, whatever it asks for.
pub fn cap_lamp_pool(light: &mut LmLight, mount: f32, texels_per_unit: f32) -> f32 {
    if texels_per_unit <= 0.0 || light.radius <= 0.0 {
        return 1.0;
    }
    let cmax = light.color.x.max(light.color.y).max(light.color.z);
    if cmax <= 0.0 {
        return 1.0;
    }
    let over = |k: f32| {
        lamp_saturated_ground_texels(mount, light.radius, cmax * k, light.spot, texels_per_unit)
            > LM_LAMP_SAT_TEXELS
    };
    if !over(1.0) {
        return 1.0;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if over(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    light.color = light.color * lo;
    lo
}

/// A placed static light. `radius` is where its contribution reaches zero —
/// finite on purpose, it is the bound that keeps lamp cost local.
#[derive(Clone, Debug)]
pub struct LmLight {
    pub pos: Vec3f,
    /// Source strength × tint, in the shader's light units — NOT a 0..1
    /// colour. What lands on a surface is this times the falloff, and the
    /// atlas clips it at [`LM_LAMP_CEIL`]. Size it with [`lamp_photometry`]
    /// rather than by eye: a lamp's job is a pool of a given brightness on
    /// the ground, and the strength that delivers it depends on how high the
    /// fixture hangs.
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
///   (`128 + sd / LM_SDF_BAND * 127`), RGB = lamp light ÷ [`LM_LAMP_CEIL`].
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

    /// The whole point of deriving photometry from the fixture instead of
    /// from the mesh: the same lamp asset placed at ×1, ×2 and ×8 puts the
    /// SAME pool on the street. Only the pole gets taller.
    #[test]
    fn pool_brightness_is_independent_of_mesh_scale() {
        // A 1.56 m Kenney lantern at scale 1, 2, 3.5 and 5 — bulbs from
        // 1.37 m to 6.8 m up.
        for scale in [1.0f32, 2.0, 3.5, 5.0] {
            let mount = 1.369 * scale;
            let (radius, strength) = lamp_photometry(mount);
            let peak = lamp_ground_light(mount, 0.0, radius, strength, 1.0);
            assert!(
                (peak - LM_LAMP_GROUND_PEAK).abs() < 1e-4,
                "scale {scale}: pool peak {peak} drifted from {LM_LAMP_GROUND_PEAK}"
            );
            // And the pool is a pool: all but spent by the class's reach,
            // gone entirely not far past it — never a lit floor to the
            // horizon, whatever the mesh was scaled to.
            let edge = lamp_ground_light(mount, LM_LAMP_POOL_RADIUS, radius, strength, 1.0);
            assert!(
                edge < peak * 0.15,
                "scale {scale}: {edge} still on the ground at the pool radius"
            );
            let out = lamp_ground_light(mount, LM_LAMP_POOL_RADIUS * 2.0, radius, strength, 1.0);
            assert_eq!(out, 0.0, "scale {scale}: pool reaches past twice its radius");
        }
    }

    /// A lamp is a lamp, not a second sun: its pool must stay well under the
    /// noon sun's 0.72 direct term and well under the atlas ceiling, so the
    /// ground texture inside the pool survives.
    #[test]
    fn pool_peak_stays_below_the_sun_and_the_encode_ceiling() {
        let (radius, strength) = lamp_photometry(2.74);
        let peak = lamp_ground_light(2.74, 0.0, radius, strength, 1.0);
        assert!(peak < 0.72 * 0.5, "pool peak {peak} rivals the noon sun");
        assert!(peak < LM_LAMP_CEIL, "pool peak {peak} clips the atlas");
        // Soft, not a disc: monotone all the way out, no plateau at the top.
        let mut last = f32::MAX;
        for i in 0..=32 {
            let rho = i as f32 * 0.25;
            let v = lamp_ground_light(2.74, rho, radius, strength, 1.0);
            assert!(v < last, "pool flat at rho {rho}");
            last = v;
        }
    }

    /// The rail, both ways: the photometry above never trips it, and the
    /// pre-fix constants (a flat 2.0 source on a fixed 8 m reach — the white
    /// plaza the user reported) do.
    #[test]
    fn saturation_cap_catches_the_white_pool_and_leaves_sane_lamps_alone() {
        // A town-sized ground region packs at ~12 texels/unit.
        let tpu = 12.3;
        let mount = 2.74;
        let (radius, strength) = lamp_photometry(mount);
        let mut sane = LmLight {
            pos: Vec3f { x: 0.0, y: mount, z: 0.0 },
            color: Vec3f { x: strength, y: strength * 0.775, z: strength * 0.475 },
            radius,
            dir: Vec3f { x: 0.0, y: -1.0, z: 0.0 },
            spot: 1.0,
        };
        assert_eq!(cap_lamp_pool(&mut sane, mount, tpu), 1.0);
        assert_eq!(
            lamp_saturated_ground_texels(mount, radius, strength, 1.0, tpu),
            0.0
        );

        let mut blown = LmLight {
            pos: Vec3f { x: 0.0, y: mount, z: 0.0 },
            color: Vec3f { x: 2.0, y: 1.55, z: 0.95 },
            radius: 8.0,
            dir: Vec3f { x: 0.0, y: -1.0, z: 0.0 },
            spot: 1.0,
        };
        assert!(
            lamp_saturated_ground_texels(mount, 8.0, 2.0, 1.0, tpu) > LM_LAMP_SAT_TEXELS,
            "the reported white pool should be over budget"
        );
        let k = cap_lamp_pool(&mut blown, mount, tpu);
        assert!(k < 1.0, "cap did not fire on the white pool");
        assert!(
            lamp_saturated_ground_texels(mount, 8.0, blown.color.x, 1.0, tpu)
                <= LM_LAMP_SAT_TEXELS * 1.001,
            "cap left the pool over budget"
        );
    }

    /// The other half of "a lamp lights its street at any scale": the near
    /// plane must clear the fixture's OWN housing however big the mesh is.
    /// Measured against the real asset that broke — Kenney's fantasy-town
    /// lantern, whose housing floor sits 0.083 of 1.369 model units below its
    /// bulb (6.1% of the pole). A fixed 0.25 m plane clears that at ×1 and
    /// ×2, and buries it 0.66 m deep at ×8, where the lamp shadowed itself
    /// and lit nothing.
    #[test]
    fn the_near_plane_clears_the_fixtures_own_housing_at_every_scale() {
        const HOUSING_FRACTION: f32 = 0.083 / 1.369;
        for scale in [1.0f32, 2.0, 3.0, 5.0, 8.0, 12.0] {
            let mount = 1.369 * scale;
            let (radius, _) = lamp_photometry(mount);
            let light = LmLight {
                pos: Vec3f { x: 0.0, y: mount, z: 0.0 },
                color: Vec3f { x: 1.0, y: 1.0, z: 1.0 },
                radius,
                dir: Vec3f { x: 0.0, y: -1.0, z: 0.0 },
                spot: 1.0,
            };
            let housing = mount * HOUSING_FRACTION;
            assert!(
                lamp_clearance(&light) > housing,
                "scale {scale}: near plane {} does not clear a housing {housing} below the bulb",
                lamp_clearance(&light)
            );
        }
        // And it never grows into a plane that swallows real occluders.
        let far = LmLight::omni(Vec3f { x: 0.0, y: 0.0, z: 0.0 }, Vec3f { x: 1.0, y: 1.0, z: 1.0 }, 4000.0);
        assert_eq!(lamp_clearance(&far), LM_LAMP_CLEARANCE_MAX);
    }

    /// A fixture hung absurdly high does not ask for an infinite bulb.
    #[test]
    fn strength_is_bounded_for_tall_mounts() {
        for mount in [0.0f32, 1.0, 12.0, 60.0, 400.0] {
            let (radius, strength) = lamp_photometry(mount);
            assert!(radius > mount, "mount {mount}: reach never touches the ground");
            assert!(
                strength > 0.0 && strength <= LM_LAMP_MAX_STRENGTH,
                "mount {mount}: strength {strength} out of range"
            );
        }
    }

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
