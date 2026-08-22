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

/// Signed-distance band encoded into A, as a WORLD half-width: ±band maps to
/// 0..255 around 128. World-sized on purpose — the band used to be 4 TEXELS
/// of the owning region, so a shadow edge crossing from a city tile's chart
/// (~4 texels/unit) onto the ground region (~12/unit) changed penumbra
/// softness threefold at the boundary. Every region now encodes the same
/// world band ([`GpuLightmapBaker`] converts per region via its measured
/// chart density); 0.35 is the town ground region's old 4-texel look, the
/// width the whole game was tuned against.
pub const LM_SUN_BAND_WORLD: f32 = 0.35;

/// Floor on a mesh chart's texel density, in texels per world unit. Region
/// size follows the model's AO layout, which is MODEL-relative: an 8m city
/// tile and a 0.4m lantern both landed ~64-texel regions, leaving the tile's
/// top face ~4 texels/unit — a street lamp's pool rendered on it as visibly
/// straight-edged bilinear facets (the user's "triangular wedge"), while the
/// same pool on the ~12/unit ground region was round. [`plan_atlas`] upsizes
/// a starved chart to this floor (never shrinks a dense one); the shelf pack
/// still owns fitting the result.
pub const LM_MESH_MIN_TPU: f32 = 8.0;

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
/// 0.9 puts the night street lamp's 0.72 pool at byte 204 — 25% encode
/// headroom over the brightest fixture, at 283 byte-levels per light unit
/// (the composite's world-hash dither hides the banding a magnified 8-bit
/// gradient would otherwise show).
pub const LM_LAMP_CEIL: f32 = 0.9;

/// What a street lamp lays on the ground straight below it, in the shader's
/// light units (the noon sun's DIRECT term is 0.72, its ambient 0.28, so a
/// fully sunlit ground reads 1.0). At REAL night the lamp is the street's
/// anchor light: 0.72 — the direct sun's own term — is the user-graded
/// presence ("a LOT brighter"; 0.30 read as a dim ~2m puddle, its 8m rim
/// under the visibility floor). Never a white disc: [`lamp_daylight_scale`]
/// rails the DELIVERED pool to `min(peak, 1 - daylight)` — daylight looks
/// keep their old pools bit-for-bit wherever the sky has spent the range —
/// and the encode ceiling holds 25% headroom over the full peak.
pub const LM_LAMP_GROUND_PEAK: f32 = 0.72;

/// The pool a street lamp is meant to lay ACROSS THE GROUND, measured from
/// the pole's foot. Together with the mount height this is the whole
/// photometry: the light's `radius` is `mount + LM_LAMP_POOL_RADIUS`, which
/// puts the fixture's reach one pool past its own bulb however high it hangs.
/// (The ground is lit slightly further than this — `sqrt(P² + 2·mount·P)` —
/// but by then the falloff has spent all but a few percent of the peak; a
/// taller lamp throwing a little further is what a taller lamp does.)
/// 12: at the night peak the att² falloff over an 8-unit reach spent the
/// pool by ~4 units (the user's "dim ~2m puddle"); the wider reach carries
/// a visible mid-pool to ~6 units and a whisper to ~8, and every consumer
/// — bake gathers, the frame's analytic lights, shadow anchors — reads the
/// same `radius` off the light, so they agree by construction.
pub const LM_LAMP_POOL_RADIUS: f32 = 12.0;

/// Ceiling on a lamp's source strength, for fixtures mounted so high that
/// normalising the pool would ask for more than any bulb emits. Past ~7 m of
/// mount the pool dims instead of the source growing without bound. 3.6 is
/// the old 1.5 scaled with the peak's 0.30 -> 0.72 night raise, so the
/// mount at which a tall lamp starts dimming is unchanged.
pub const LM_LAMP_MAX_STRENGTH: f32 = 3.6;

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

/// Light an unshadowed, FLAT, UP-FACING ground texel already receives with
/// no lamp on it at all, in the channel that reaches white first.
///
/// This is exactly what the composites give such a fragment before the lamp
/// term is added — `ambient + direct` with the hemisphere term collapsed to
/// `sun_sky` (a texel whose normal is +y) and `N·L` collapsed to the sun's
/// elevation (`sun_dir.y`):
///
/// ```text
/// daylight = max_channel(sun_sky + sun_color * max(sun_dir.y, 0))
/// ```
///
/// Flat up-facing ground is the matched case for [`LM_LAMP_GROUND_PEAK`],
/// which is defined on exactly that surface directly under the bulb — the
/// place where daylight and lamp light are BOTH at their maximum and where
/// the sum therefore clips first. A slope tilted into the sun reads more
/// daylight, but a downlight's own `N·L` falls off with the same tilt, so
/// the pair stays the one that matters.
pub fn daylight_on_ground(sun_dir: Vec3f, sun_color: Vec3f, sun_sky: Vec3f) -> f32 {
    let ndl = sun_dir.y.max(0.0);
    let r = sun_sky.x + sun_color.x * ndl;
    let g = sun_sky.y + sun_color.y * ndl;
    let b = sun_sky.z + sun_color.z * ndl;
    r.max(g).max(b)
}

/// The share of its pool a lamp may actually lay on ground the sky is
/// ALREADY lighting: a lamp gets the headroom daylight leaves, and no more.
///
/// This is the rail that was missing. [`LM_LAMP_CEIL`], [`cap_lamp_pool`]
/// and [`LM_LAMP_SAT_TEXELS`] all measure ONE lamp against the ATLAS encode
/// range (0.5), which is 1.67× the pool peak — so for anything
/// [`lamp_photometry`] sized they can never fire. Nothing measured the
/// number that actually reaches the screen: the SUM
///
/// ```text
/// out = albedo * (ambient + direct * sun_vis + lamp)
/// ```
///
/// [`LM_LAMP_GROUND_PEAK`]'s doc sizes 0.30 against "the noon sun's 0.72
/// direct term" — but the same doc says a fully sunlit ground reads 1.0,
/// ambient included. At noon there is no headroom at all: a 0.30 pool on a
/// near-white plaza is `0.97 × 1.30 = 1.26`, and the pool's core clips to
/// white with a warm rim where the tint's red channel saturates before its
/// blue. That is the blown plaza — not the atlas, which was never over its
/// own ceiling; the FRAME.
///
/// So the pool peak becomes `min(LM_LAMP_GROUND_PEAK, 1 - daylight)`:
///
/// ```text
/// scale = clamp((1 - daylight) / LM_LAMP_GROUND_PEAK, 0, 1)
/// ```
///
/// which makes `daylight + peak ≤ 1` an identity at every hour and on every
/// albedo ≤ 1 — **a lamp can never push a texel to white.** Below
/// `daylight = 1 - LM_LAMP_GROUND_PEAK` (0.70) the scale is exactly 1, so
/// dusk, night and every interior are bit-for-bit what they were; only the
/// hours with no room left dim, and they dim continuously.
pub fn lamp_daylight_scale(daylight: f32) -> f32 {
    let headroom = (1.0 - daylight).clamp(0.0, 1.0);
    (headroom / LM_LAMP_GROUND_PEAK).clamp(0.0, 1.0)
}

/// The ground light at which a lamp pool has COMPLETELY drowned the sun's
/// shadow inside it — a quarter of [`LM_LAMP_GROUND_PEAK`].
///
/// Not taste: the peak itself is only reached directly under the bulb, a
/// patch the fixture's own housing largely hides. Measured on the shipped
/// Kenney lantern at ×2 (bulb 2.74 m up, 10.7 m reach), full-peak light
/// stops 4 cm from the pole while the pool's visible disc runs to 6 m — so
/// filling only at the peak leaves the shadow standing across the whole part
/// of the pool anyone looks at, which was exactly the reported symptom. A
/// quarter of the peak is still on the ground 3.1 m out and gone by 5.5 m:
/// the fill covers the pool's bright core and tapers away with the pool's own
/// falloff, well inside its 8 m rim. It scales with the fixture like
/// everything else here — a lamp on a taller pole spreads its pool wider and
/// fills wider with it.
pub const LM_LAMP_SHADOW_FILL_AT: f32 = 0.25 * LM_LAMP_GROUND_PEAK;

/// How much of the SUN's shadow a local light standing in it fills back in.
///
/// A shadow is only the absence of SUN light, and every composite in
/// `shaders.rs` has always honoured that: the shadow term gates the DIRECT
/// sun and nothing else, while the lamp term (baked atlas RGB plus the
/// per-frame `dl_sum`) is added unshadowed on top. Additive light alone,
/// though, cannot DROWN a shadow — it only lifts its floor. At
/// [`LM_LAMP_GROUND_PEAK`] against the noon sun's 0.72 direct term, the streak
/// a street lamp's own pole throws across its own pool survives at better than
/// 2:1 contrast, which is the thing that reads wrong: a pool bright enough to
/// paint the road is somehow not bright enough to erase a shadow inside it.
///
/// So the pool's own strength lifts the sun's visibility term as well:
///
/// ```text
/// fill       = clamp(lamp / LM_LAMP_SHADOW_FILL_AT, 0, 1)
/// sun_filled = sun_vis + (1 - sun_vis) * fill
/// ```
///
/// Over the pool's bright core the shadow inside it is gone; past
/// [`LM_LAMP_SHADOW_FILL_AT`] the fill tapers with the pool's own falloff, so
/// the filled region is a lit region and never a hard-edged disc; away from
/// every lamp `lamp` is 0, so sun, sky, AO and the cascades are untouched by
/// construction.
///
/// **It cannot blow anything out.** A filled fragment reads
/// `ambient + direct + lamp` — precisely what the SAME fragment reads standing
/// in open sun, which is a value the composite already produces wherever a
/// pool falls on unshadowed ground. The ceiling of the sum is therefore
/// unchanged, and with it every guarantee [`cap_lamp_pool`] and
/// [`LM_LAMP_CEIL`] make about it. `fill` saturates at 1, so a lamp can put
/// the sun back and never more than back.
///
/// This is the CPU twin of the four `sun_filled` shader functions; the shaders
/// are the authority and the constant is pinned into their source by
/// `every_composite_fills_the_sun_shadow_from_the_same_constant`.
pub fn lamp_shadow_fill(lamp: f32, sun_vis: f32) -> f32 {
    let fill = (lamp / LM_LAMP_SHADOW_FILL_AT).clamp(0.0, 1.0);
    sun_vis + (1.0 - sun_vis) * fill
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

/// Ground texels one lamp drives past `ceiling`, at `texels_per_unit`.
///
/// Two ceilings matter and they are NOT the same number. [`LM_LAMP_CEIL`]
/// is what the ATLAS can encode; `1 - daylight` (see
/// [`lamp_daylight_scale`]) is what the FRAME can show. A pool can sit
/// comfortably inside the first and paint a street white through the
/// second — which is exactly the bug this argument exists to let the bake
/// report on.
pub fn lamp_ground_texels_over(
    mount: f32,
    radius: f32,
    strength: f32,
    spot: f32,
    texels_per_unit: f32,
    ceiling: f32,
) -> f32 {
    // One display byte of tolerance: the frame is 8-bit, so a texel within
    // 1/255 of the clip point cannot READ as clipped — and the daylight
    // rail delivers a peak of exactly `1 - daylight`, which lands a float
    // ULP either side of the ceiling depending on the peak constant.
    let ceiling = ceiling + 1.0 / 255.0;
    if lamp_ground_light(mount, 0.0, radius, strength, spot) <= ceiling {
        return 0.0;
    }
    // Monotone in rho below the bulb: bisect for the crossing.
    let (mut lo, mut hi) = (0.0f32, radius);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if lamp_ground_light(mount, mid, radius, strength, spot) > ceiling {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    std::f32::consts::PI * lo * lo * texels_per_unit * texels_per_unit
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
    lamp_ground_texels_over(mount, radius, strength, spot, texels_per_unit, LM_LAMP_CEIL)
}

/// A fixture's implied mount from its reach: `lamp_photometry` sets
/// `radius = mount + LM_LAMP_POOL_RADIUS`, so this inverts it exactly for
/// anything harvested and degrades gracefully for a hand-set light.
pub fn lamp_mount_from_radius(radius: f32) -> f32 {
    (radius - LM_LAMP_POOL_RADIUS).max(0.05)
}

/// Upper bound on the light one lamp can lay ANYWHERE inside a world box —
/// the gather's distance falloff at the box's closest approach, with `N·L`
/// and the spot cone both at their maxima (1). Cheap, never optimistic, and
/// exact for the flat ground straight under a downlight, which is the case
/// that matters. The bake reports with it (`gpu_lightmap.rs`), so a blown
/// region names its own numbers in the log instead of needing a screenshot.
pub fn lamp_peak_in_box(light: &LmLight, min: Vec3f, max: Vec3f) -> f32 {
    let cmax = light.color.x.max(light.color.y).max(light.color.z);
    if cmax <= 0.0 || light.radius <= 0.0 {
        return 0.0;
    }
    let c = |v: f32, lo: f32, hi: f32| v.clamp(lo.min(hi), hi.max(lo));
    let dx = light.pos.x - c(light.pos.x, min.x, max.x);
    let dy = light.pos.y - c(light.pos.y, min.y, max.y);
    let dz = light.pos.z - c(light.pos.z, min.z, max.z);
    let d = (dx * dx + dy * dy + dz * dz).sqrt();
    if d >= light.radius {
        return 0.0;
    }
    let att = 1.0 - d / light.radius;
    cmax * att * att
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
    /// Hemisphere ambient from above. Rides along with `sun_color` so the
    /// bake can state, per region, how much of the display's 0..1 the sky
    /// has already spent before a lamp adds anything
    /// ([`daylight_on_ground`]).
    pub sun_sky: Vec3f,
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
    /// This region's rect PLUS the pad ring [`pack_regions`] reserves around
    /// it, clamped to the atlas.
    ///
    /// THE BAKE'S WRITE FOOTPRINT. Every bake-side write that reaches past
    /// the chart uses exactly this rect — the encode's "fully lit, no lamps"
    /// default ring and the lamp accumulator's zero — and the pack keeps
    /// [`LM_PAD`] between regions, so it can never touch a neighbour's.
    ///
    /// The zero and the writes MUST be the same rect. When the zero was
    /// chart-shaped (a rasterized coverage prepass) while the dilate and the
    /// encode were rect-shaped, re-baking a settled 98-region town into its
    /// own targets grew the lit set 13142 -> 31775 texels over five bakes,
    /// every one of them brighter and none dimmer: the rim texels the chart
    /// never covers kept the previous bake's light, and each dilate spread
    /// it one ring further out.
    pub fn padded(&self, atlas: usize) -> LmRect {
        let x = self.x.saturating_sub(LM_PAD);
        let y = self.y.saturating_sub(LM_PAD);
        LmRect {
            x,
            y,
            w: (self.w + (self.x - x) + LM_PAD).min(atlas.saturating_sub(x)),
            h: (self.h + (self.y - y) + LM_PAD).min(atlas.saturating_sub(y)),
        }
    }

    /// Do these two rects share a texel?
    pub fn intersects(&self, other: &LmRect) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }

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
///   (`128 + sd / band * 127`, band = [`LM_SUN_BAND_WORLD`] in the region's
///   texels), RGB = lamp light ÷ [`LM_LAMP_CEIL`].
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

/// The measured texel density of one placed chart: how many atlas texels a
/// world unit of this instance's SURFACE gets, if its chart lands in a
/// `rect_w` x `rect_h` region. The chart's uv parameterisation is the
/// authority — a region's rect-vs-AABB ratio lies (a city tile's chart puts
/// its 8m top face across ~half the uv span, so the surface bakes at HALF
/// the rect's nominal density). Area-weighted over every triangle:
/// sqrt(chart texel area / world surface area).
pub fn chart_density(m: &LmMeshInstance, rect_w: usize, rect_h: usize) -> f32 {
    let src = &m.source;
    let (mut uv_area, mut w_area) = (0.0f64, 0.0f64);
    for t in 0..src.caster.tri_count() as u32 {
        let [i0, i1, i2] = src.caster.triangle_verts(t);
        let (a, b, c) = src.caster.triangle(t);
        let wp = |p: Vec3f| {
            let q = m.transform.transform_vec4(Vec4f { x: p.x, y: p.y, z: p.z, w: 1.0 });
            Vec3f { x: q.x, y: q.y, z: q.z }
        };
        let (wa, wb, wc) = (wp(a), wp(b), wp(c));
        let e1 = wb - wa;
        let e2 = wc - wa;
        let cx = e1.y * e2.z - e1.z * e2.y;
        let cy = e1.z * e2.x - e1.x * e2.z;
        let cz = e1.x * e2.y - e1.y * e2.x;
        w_area += 0.5 * ((cx * cx + cy * cy + cz * cz) as f64).sqrt();
        let ua = src.ao_uv[i0 as usize];
        let ub = src.ao_uv[i1 as usize];
        let uc = src.ao_uv[i2 as usize];
        let du1 = ((ub[0] - ua[0]) * rect_w as f32, (ub[1] - ua[1]) * rect_h as f32);
        let du2 = ((uc[0] - ua[0]) * rect_w as f32, (uc[1] - ua[1]) * rect_h as f32);
        uv_area += 0.5 * ((du1.0 * du2.1 - du1.1 * du2.0) as f64).abs();
    }
    if w_area <= 1e-9 || uv_area <= 1e-9 {
        // A degenerate chart: fall back to the rect-vs-AABB nominal.
        let (lo, hi) = m.world_bounds();
        let span = (hi.x - lo.x).max(hi.z - lo.z).max(0.001);
        return rect_w.max(rect_h) as f32 / span;
    }
    (uv_area / w_area).sqrt() as f32
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
        let w0 = ((m.source.ao_w as f32 * LM_MESH_SCALE).ceil() as usize).clamp(4, 512);
        let h0 = ((m.source.ao_h as f32 * LM_MESH_SCALE).ceil() as usize).clamp(4, 512);
        // World-density floor: the AO layout is model-relative, so a big
        // flat piece can land a chart too coarse to draw a lamp pool
        // without visible bilinear facets. Upsize (never shrink) so the
        // measured chart density reaches LM_MESH_MIN_TPU; x4 cap keeps a
        // pathological chart from eating the atlas, and the shelf pack's
        // global shrink loop still has the last word.
        let tpu = chart_density(m, w0, h0);
        let s = if tpu > 0.0001 { (LM_MESH_MIN_TPU / tpu).clamp(1.0, 4.0) } else { 1.0 };
        let w = ((w0 as f32 * s).ceil() as usize).clamp(4, 512);
        let h = ((h0 as f32 * s).ceil() as usize).clamp(4, 512);
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
        // The user-graded night presence: the lamp may MATCH the direct
        // sun's 0.72 term — at real night it IS the street's sun — but
        // never exceed it, and the atlas ceiling keeps 25% encode
        // headroom over the full peak.
        assert!(peak <= 0.72 + 1e-4, "pool peak {peak} outshines the noon sun");
        assert!(peak < LM_LAMP_CEIL * 0.85, "pool peak {peak} crowds the atlas ceiling");
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

        // 4.0 flat: the pre-fix white pool, rescaled to stay a white pool
        // against the raised encode ceiling (the historical 2.0 on an 8 m
        // reach peaks at 0.86 — under the new 0.9, it no longer saturates
        // the ATLAS at all; the frame rail is what bounds it now).
        let mut blown = LmLight {
            pos: Vec3f { x: 0.0, y: mount, z: 0.0 },
            color: Vec3f { x: 4.0, y: 3.1, z: 1.9 },
            radius: 8.0,
            dir: Vec3f { x: 0.0, y: -1.0, z: 0.0 },
            spot: 1.0,
        };
        assert!(
            lamp_saturated_ground_texels(mount, 8.0, 4.0, 1.0, tpu) > LM_LAMP_SAT_TEXELS,
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

    /// THE rail this lane exists for: whatever the hour, whatever the
    /// albedo, a lamp may not push a fragment to white. The sum the frame
    /// actually shows — `albedo * (daylight + pool)` — stays at or under 1.
    ///
    /// The bug it closes: `LM_LAMP_GROUND_PEAK` was sized against the noon
    /// sun's 0.72 DIRECT term, but the composite adds the 0.28 ambient too,
    /// so a fully sunlit up-facing texel is already at 1.0 with no headroom
    /// left. A 0.30 pool on the reported plaza (albedo 0.97) read
    /// `0.97 × 1.30 = 1.26` — clipped white, with the warm rim of the
    /// tint's red channel saturating a step before its blue.
    #[test]
    fn a_lamp_can_never_push_a_texel_to_white() {
        let mount = 1.369 * 2.0;
        let (radius, strength) = lamp_photometry(mount);
        // Every hour of a full day at the village's latitude, and the two
        // ends of the legacy rig.
        let mut hours: Vec<f32> = (0..=48).map(|i| i as f32 * 0.5).collect();
        hours.push(12.5); // the world in the report
        for h in hours {
            let sun = crate::sun::SunLight::from_time_of_day(h, 52.0);
            let daylight = daylight_on_ground(sun.dir, sun.color, sun.sky);
            let scale = lamp_daylight_scale(daylight);
            // The pool the bake will actually lay, at its brightest texel.
            let peak = lamp_ground_light(mount, 0.0, radius, strength * scale, 1.0);
            assert!(
                daylight + peak <= 1.0 + 1e-4,
                "hour {h}: daylight {daylight} + pool {peak} = {} clips",
                daylight + peak
            );
            // ...and it is never a NEGATIVE lamp, nor brighter than the
            // fixture was ever meant to be.
            assert!((0.0..=LM_LAMP_GROUND_PEAK + 1e-6).contains(&peak), "hour {h}: {peak}");
        }
    }

    /// The rail costs nothing where there is room: dusk, night and every
    /// interior keep the pool that was signed off, unchanged.
    #[test]
    fn the_headroom_rail_only_bites_where_the_sky_left_no_room() {
        // Below the crossover the scale is exactly 1 — bit-for-bit the old
        // behaviour.
        for daylight in [0.0f32, 0.05, 0.1, 0.2, 1.0 - LM_LAMP_GROUND_PEAK] {
            assert_eq!(
                lamp_daylight_scale(daylight),
                1.0,
                "daylight {daylight} dimmed a lamp that had room"
            );
        }
        // Above it the taper is continuous and monotone — no step in a day
        // cycle, ever.
        let mut prev = 1.0f32;
        for i in 0..=100 {
            let daylight = (1.0 - LM_LAMP_GROUND_PEAK)
                + LM_LAMP_GROUND_PEAK * i as f32 / 100.0;
            let s = lamp_daylight_scale(daylight);
            assert!(s <= prev + 1e-6, "scale rose at daylight {daylight}");
            assert!((0.0..=1.0).contains(&s));
            prev = s;
        }
        assert_eq!(lamp_daylight_scale(1.0), 0.0);
        assert_eq!(lamp_daylight_scale(2.0), 0.0, "an over-bright sky is still zero, not negative");
        // The village's own numbers, pinned: at the authored noon the sky
        // has spent 86% of the range, so the lamp gets the last 14%.
        let noon = crate::sun::SunLight::from_time_of_day(12.5, 52.0);
        let daylight = daylight_on_ground(noon.dir, noon.color, noon.sky);
        assert!(
            (0.80..0.90).contains(&daylight),
            "the authored noon moved: daylight {daylight}"
        );
        // The DELIVERED noon pool is the same 14% whatever the peak
        // constant reads — the scale is just that headroom over the peak.
        let noon_pool = lamp_daylight_scale(daylight) * LM_LAMP_GROUND_PEAK;
        assert!(
            ((1.0 - daylight) - noon_pool).abs() < 1e-3,
            "noon delivered pool {noon_pool} is not the sky's headroom"
        );
        // And the dusk the shadow-fill lane verified against delivers AT
        // LEAST the pool it was signed off with (the night peak raise can
        // only add light where the sky leaves room).
        let dusk = crate::sun::SunLight::from_time_of_day(17.6, 52.0);
        let dusk_pool = lamp_daylight_scale(daylight_on_ground(dusk.dir, dusk.color, dusk.sky))
            * LM_LAMP_GROUND_PEAK;
        assert!(dusk_pool >= 0.2999, "the verified dusk pool got dimmed: {dusk_pool}");
    }

    /// The reported bug, as arithmetic: the pre-rail pool clipped the plaza
    /// and the railed one does not — measured on the exact fixture, sun and
    /// albedo of the world in the report.
    #[test]
    fn the_reported_plaza_stops_clipping() {
        /// The plaza tile, measured off a post-bake grab: 206/255 under a
        /// daylight of 0.8225 in red.
        const PLAZA_ALBEDO: f32 = 0.97;
        let sun = crate::sun::SunLight::from_time_of_day(12.5, 52.0);
        let daylight = daylight_on_ground(sun.dir, sun.color, sun.sky);
        let mount = 1.369 * 2.0; // the ×2 Kenney lantern in the slot
        let (radius, strength) = lamp_photometry(mount);

        let before = PLAZA_ALBEDO * (daylight + lamp_ground_light(mount, 0.0, radius, strength, 1.0));
        assert!(before > 1.0, "the reported blowout did not reproduce: {before}");

        let scale = lamp_daylight_scale(daylight);
        let after =
            PLAZA_ALBEDO * (daylight + lamp_ground_light(mount, 0.0, radius, strength * scale, 1.0));
        assert!(after <= 1.0, "the rail did not hold: {after}");
        // Still a POOL, not a switched-off lamp: visibly brighter than the
        // bare plaza beside it.
        let bare = PLAZA_ALBEDO * daylight;
        assert!(
            after > bare * 1.10,
            "the pool vanished instead of fitting: {after} vs bare {bare}"
        );
    }

    /// The bake's report is only worth logging if its bound is a real one:
    /// never under what a lamp can actually deliver inside the box.
    #[test]
    fn the_reported_region_peak_is_an_upper_bound() {
        let mount = 1.369 * 2.0;
        let (radius, strength) = lamp_photometry(mount);
        let light = LmLight {
            pos: Vec3f { x: 0.0, y: mount, z: 0.0 },
            color: Vec3f { x: strength, y: strength * 0.775, z: strength * 0.475 },
            radius,
            dir: Vec3f { x: 0.0, y: -1.0, z: 0.0 },
            spot: 1.0,
        };
        // A ground region under the lamp: the bound must cover the nadir
        // peak, which is the brightest texel that exists.
        let min = Vec3f { x: -20.0, y: 0.0, z: -20.0 };
        let max = Vec3f { x: 20.0, y: 0.0, z: 20.0 };
        let bound = lamp_peak_in_box(&light, min, max);
        let real = lamp_ground_light(mount, 0.0, radius, strength, 1.0);
        assert!(bound >= real - 1e-6, "bound {bound} under the real peak {real}");
        // Out of reach is zero, not a tiny number.
        let far = Vec3f { x: 1000.0, y: 0.0, z: 1000.0 };
        assert_eq!(lamp_peak_in_box(&light, far, far), 0.0);
    }

    /// The lamp-fill law: a pool erases the sun's shadow over its bright core,
    /// an unlit fragment is untouched, and the two ends are joined
    /// monotonically by the pool's own falloff — never a hard disc edge.
    #[test]
    fn a_pool_drowns_the_sun_shadow_and_darkness_changes_nothing() {
        // Deep in a building's shadow (sun_vis 0), under the pole.
        assert!(
            (lamp_shadow_fill(LM_LAMP_GROUND_PEAK, 0.0) - 1.0).abs() < 1e-6,
            "a pool at its own peak must put the whole sun back"
        );
        // No lamp anywhere: sun, sky, AO and the cascades untouched.
        for vis in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            assert_eq!(lamp_shadow_fill(0.0, vis), vis, "an unlit fragment moved");
        }
        // Open sun stays open sun — the fill has nothing to give back.
        assert_eq!(lamp_shadow_fill(LM_LAMP_GROUND_PEAK, 1.0), 1.0);
        // And it saturates: brighter than the fill point cannot go past "lit".
        assert_eq!(lamp_shadow_fill(LM_LAMP_GROUND_PEAK * 4.0, 0.0), 1.0);

        // Across a real pool the fill follows the pool, so the filled region
        // IS a lit region — no hard-edged disc of restored sunlight. The
        // numbers in LM_LAMP_SHADOW_FILL_AT's doc are these, measured.
        let mount = 1.369 * 2.0;
        let (radius, strength) = lamp_photometry(mount);
        let fill_at = |rho: f32| {
            lamp_shadow_fill(lamp_ground_light(mount, rho, radius, strength, 1.0), 0.0)
        };
        let mut prev = 2.0f32;
        for step in 0..=64 {
            let rho = LM_LAMP_POOL_RADIUS * step as f32 / 64.0;
            let v = fill_at(rho);
            assert!(v <= prev + 1e-6, "fill rose again {rho} m out: {prev} -> {v}");
            assert!((0.0..=1.0).contains(&v), "fill left [0,1] at {rho} m: {v}");
            prev = v;
        }
        assert_eq!(fill_at(3.0), 1.0, "the pool's bright core stopped filling");
        assert!(fill_at(5.5) < 0.35, "the fill reached too far: {}", fill_at(5.5));
        assert!(prev < 0.15, "the pool's rim still erased the shadow: {prev}");
    }

    /// The fill may not blow anything out, and the argument is exact rather
    /// than a matter of taste: a FILLED fragment reads what the very same
    /// fragment reads standing in open sun with the same pool on it, which is
    /// a value the composite already produced before the fill existed. The
    /// ceiling of the sum is therefore unchanged, and [`cap_lamp_pool`] and
    /// [`LM_LAMP_CEIL`] keep bounding exactly what they bounded.
    #[test]
    fn filling_a_shadow_never_beats_standing_in_the_open_sun() {
        // shaders.rs light units: hemisphere ambient + sun DIRECT = 1.0 lit.
        const AMBIENT: f32 = 0.28;
        const DIRECT: f32 = 0.72;
        let mount = 1.369 * 2.0;
        let (radius, strength) = lamp_photometry(mount);
        for step in 0..=32 {
            let rho = LM_LAMP_POOL_RADIUS * step as f32 / 32.0;
            let lamp = lamp_ground_light(mount, rho, radius, strength, 1.0);
            // What a fragment in the open reads: the pre-existing ceiling.
            let open = AMBIENT + DIRECT + lamp;
            for vis in [0.0f32, 0.2, 0.5, 0.8, 1.0] {
                let filled = AMBIENT + DIRECT * lamp_shadow_fill(lamp, vis) + lamp;
                assert!(
                    filled <= open + 1e-6,
                    "{rho} m out at vis {vis}: filled {filled} beat open sun {open}"
                );
                // And it is never darker than it was without the fill, or the
                // shadow would have got deeper for standing near a lamp.
                let unfilled = AMBIENT + DIRECT * vis + lamp;
                assert!(
                    filled >= unfilled - 1e-6,
                    "{rho} m out at vis {vis}: fill darkened {unfilled} to {filled}"
                );
            }
        }
    }

    /// The four shader composites solve the same fill against the same
    /// constant. The number lives twice — once as [`LM_LAMP_SHADOW_FILL_AT`],
    /// once as a literal in each `sun_filled`, because a shader function
    /// cannot read a Rust const — so pin them together: a lamp that fills to a
    /// different depth on terrain than on a wall is the bug this catches.
    #[test]
    fn every_composite_fills_the_sun_shadow_from_the_same_constant() {
        let src = include_str!("shaders.rs");
        let at = format!("/ {LM_LAMP_SHADOW_FILL_AT:.3}, 0.0, 1.0)");
        assert_eq!(
            src.matches(&at).count(),
            4,
            "expected 4 sun_filled bodies dividing by {at}"
        );
        assert_eq!(
            src.matches("sun_filled: fn(sun_vis: float, local: vec3) -> float").count(),
            4,
            "a sun_filled declaration appeared or vanished"
        );
        // DrawSceneCube, DrawSceneSkinned, DrawScenePbr (inherited),
        // DrawSceneSkinnedGpu and DrawSceneTerrain each call it once.
        assert_eq!(
            src.matches("self.sun_filled(").count(),
            5,
            "a composite stopped filling the sun shadow, or gained a call"
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

    /// Source pin for the chart-edge lamp repair (the boundary-seam fix).
    /// The AO charts place face edges exactly ON texel centers, so a tile's
    /// outermost chart texel goes to whichever face wins the rasterization
    /// tie — on a max edge that is the sub-texel vertical SKIRT, which
    /// bakes ~0.43x the top face's lamp light into the very texel the top
    /// face's edge samples at full weight: a hard dark line at every
    /// static-static boundary crossing a pool (measured on the town plaza:
    /// boundary row 38-55 against a ground-region truth of 89-128, and a
    /// rendered step of 22 RGB units that the repair takes to <2). The
    /// repair lives in DrawLmLampDilate: partial-coverage texels are
    /// distrusted and rebuilt from the fully-covered interior, with a
    /// keep-rule for texels within their neighbors' range (shadow edges
    /// only the raw sample gets right) and a monotone guard so the
    /// continuation never extrapolates across a shadow edge. This test
    /// pins the pieces together so a refactor cannot silently drop one.
    #[test]
    fn the_lamp_dilate_distrusts_chart_edge_texels() {
        let src = include_str!("shaders.rs");
        let dilate = {
            let start = src.find("mod.draw.DrawLmLampDilate").expect("dilate shader");
            let end = src[start..].find("mod.draw.DrawLmEncode").expect("encode follows");
            &src[start..start + end]
        };
        for needle in [
            // the coverage tap and both trust gates
            "cov_tex: texture_2d(float)",
            "covf: fn(off: vec2) -> float",
            "if self.covf(vec2(0.0, 0.0)) > 0.99 {",
            // the keep-rule margin for dark-side-of-gradient texels
            "if om >= minv * 0.85 {",
            // the shadow-edge extrapolation guard
            "if s2m >= sm * 0.5 {",
        ] {
            assert!(
                dilate.contains(needle),
                "DrawLmLampDilate lost its chart-edge repair piece: {needle}"
            );
        }
        // The smooth pass must keep skipping partial texels, or it eats the
        // repair back out (measured: a rebuilt 122 fell to 91).
        assert_eq!(
            dilate.matches("if self.covf(vec2(0.0, 0.0)) > 0.99 {").count() >= 1
                && dilate.matches("covf").count() >= 4,
            true,
            "the smooth pass stopped consulting coverage"
        );
        // The ALPHA channel's twin repairs: the chamfer seed takes a
        // chart-edge texel's lit fraction from its fully-covered ring, and
        // the encode's sub-texel nudge only trusts fully-covered fractions.
        let chamfer = {
            let start = src.find("mod.draw.DrawLmChamfer").expect("chamfer shader");
            let end = src[start..].find("mod.draw.DrawLmLampGatherMesh").expect("gather follows");
            &src[start..start + end]
        };
        assert!(
            chamfer.contains("cvf: fn(off: vec2) -> vec2")
                && chamfer.matches("s.y > 0.996").count() == 8,
            "the chamfer seed stopped distrusting chart-edge lit fractions"
        );
        let encode = {
            let start = src.find("mod.draw.DrawLmEncode").expect("encode shader");
            let end = src[start..].find("mod.draw.DrawLmTop").expect("top follows");
            &src[start..start + end]
        };
        for needle in [
            // world-band decode: the baker's per-region band rides misc_a.z
            "var band = self.misc_a.z",
            "sd * (127.0 / band)",
            // the nudge's full-coverage gate
            "if cov.y > 0.996 {",
        ] {
            assert!(
                encode.contains(needle),
                "DrawLmEncode lost its world-band/trust piece: {needle}"
            );
        }
    }

    /// The sun penumbra is a WORLD width. A region's chart density converts
    /// it to texels at encode time; the density itself must come from the
    /// chart's uv-vs-world areas — the rect-vs-AABB nominal LIES about any
    /// chart that does not fill its region (a city tile's top face takes
    /// half the uv span, so its surface bakes at half the nominal density,
    /// which is exactly how the tile got wedge-shaped lamp pools).
    #[test]
    fn chart_density_measures_the_chart_not_the_rect() {
        // An 8x8 world quad whose chart occupies HALF the uv span of a
        // 64-texel region: 32 texels over 8 units = 4 texels/unit, where
        // the nominal rect/span claims 8.
        let positions = vec![
            Vec3f { x: 0.0, y: 0.0, z: 0.0 },
            Vec3f { x: 8.0, y: 0.0, z: 0.0 },
            Vec3f { x: 8.0, y: 0.0, z: 8.0 },
            Vec3f { x: 0.0, y: 0.0, z: 8.0 },
        ];
        let indices = vec![0, 1, 2, 0, 2, 3];
        let source = std::sync::Arc::new(LmMeshSource {
            caster: crate::ao::MeshRaycaster::new(
                positions,
                indices,
                Vec3f { x: 0.0, y: 0.0, z: 0.0 },
                Vec3f { x: 8.0, y: 0.0, z: 8.0 },
            ),
            ao_uv: vec![[0.0, 0.0], [0.5, 0.0], [0.5, 0.5], [0.0, 0.5]],
            albedo: Vec::new(),
            ao_w: 128,
            ao_h: 128,
        });
        let inst = LmMeshInstance { source, transform: Mat4f::identity() };
        let d = chart_density(&inst, 64, 64);
        assert!((d - 4.0).abs() < 0.05, "density {d}, expected 4.0");
        // A x2-scaled placement spreads the same chart over twice the
        // world: half the density. The floor in plan_atlas exists for
        // exactly this — placed scale starves charts the model thought
        // were fine.
        let mut t = Mat4f::identity();
        t.v[0] = 2.0;
        t.v[5] = 2.0;
        t.v[10] = 2.0;
        let inst2 = LmMeshInstance { source: inst.source.clone(), transform: t };
        let d2 = chart_density(&inst2, 64, 64);
        assert!((d2 - 2.0).abs() < 0.05, "scaled density {d2}, expected 2.0");
        // And plan_atlas lifts the starved chart to the floor: the region
        // it asks for is scaled by LM_MESH_MIN_TPU / density (capped x4).
        let scene = LmScene {
            meshes: vec![inst2],
            planars: Vec::new(),
            boxes: Vec::new(),
            lights: Vec::new(),
            sun_dir: Vec3f { x: 0.0, y: 1.0, z: 0.0 },
            sun_color: Vec3f { x: 1.0, y: 1.0, z: 1.0 },
            sun_sky: Vec3f { x: 0.2, y: 0.2, z: 0.2 },
            bounce: false,
        };
        let (_, rects) = plan_atlas(&scene);
        // ao 128 * LM_MESH_SCALE = 64 nominal; density 2.0 wants x4 to
        // reach LM_MESH_MIN_TPU=8 -> 256.
        assert_eq!(rects[0].w, 256, "the density floor did not lift the region");
    }

    /// The bake writes — and therefore must zero — one texel MORE than a
    /// region's chart on every side: the encode stamps that ring with the
    /// region's own default and the lamp dilate reads it. So the ring has to
    /// be this region's alone, and `padded` has to stay inside the atlas.
    ///
    /// (The bug this pins: the lamp accumulator was zeroed by rasterizing
    /// the CHART while the dilate and the encode wrote the RECT. Re-baking a
    /// settled 98-region town into its own targets then grew the lit set
    /// 13142 -> 20777 -> 25669 -> 29019 -> 31775 texels over five bakes,
    /// every differing texel brighter and none dimmer.)
    #[test]
    fn a_regions_pad_ring_is_its_own_and_stays_in_the_atlas() {
        let want = vec![(64, 32), (128, 128), (4, 4), (500, 20), (20, 500), (77, 33)];
        let (size, rects) = pack_regions(&want);
        for (i, a) in rects.iter().enumerate() {
            let pad = a.padded(size);
            assert!(
                pad.x <= a.x
                    && pad.y <= a.y
                    && pad.x + pad.w >= a.x + a.w
                    && pad.y + pad.h >= a.y + a.h,
                "the zero footprint {pad:?} does not cover region {i} {a:?}"
            );
            assert!(
                pad.x + pad.w <= size && pad.y + pad.h <= size,
                "region {i}'s footprint {pad:?} leaves the {size}px atlas"
            );
            for (j, b) in rects.iter().enumerate() {
                if i != j {
                    assert!(
                        !pad.intersects(b),
                        "region {i}'s pad ring {pad:?} reaches into region {j} {b:?}"
                    );
                }
            }
        }
        // A region hard against the atlas origin clamps instead of wrapping.
        let corner = LmRect { x: 0, y: 0, w: 8, h: 8 }.padded(16);
        assert_eq!(corner, LmRect { x: 0, y: 0, w: 9, h: 9 });
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
