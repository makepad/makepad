//! Ported from ands/lightmapper (<https://github.com/ands/lightmapper>),
//! public domain (fallback: perpetual irrevocable copy/modify license).
//!
//! CPU port of the AO-relevant core of `lightmapper.h` — hemicube ambient
//! occlusion baked from lightmap-UV space, with the header's conservative
//! rasterizer, hierarchical interpolation passes, weighting kernel and
//! validity handling ported 1:1 and the OpenGL renderer replaced by a small
//! software rasterizer. This is the DEFAULT atlas evaluator (see
//! [`crate::ao_atlas::AoBakerKind`]); [`dedup_twins`] is the engine's own
//! preprocessing, run by `bake_into` before charts are grown.
//!
//! # Art knobs (env vars, also `ao-bake --lm-strength/--lm-gamma`)
//!
//! * `AO_LM_STRENGTH` (default 1.0) — scales the OCCLUSION after the
//!   open-sky rescale: `value = 1 - strength * (1 - ao)`, clamped. Open
//!   faces stay white at any strength; >1 deepens every crease.
//! * `AO_LM_GAMMA` (default 2.2) — the example.c display `pow(1/gamma)`
//!   encode. Lower = darker mids; 1.0 = linear.
//!
//! # What "AO" means here
//!
//! Exactly what the lightmapper example bakes: the scene is rendered from
//! every texel's surface position over the hemisphere (five hemicube faces),
//! with a pure white sky as the clear color and the geometry drawn black.
//! The weighted integral of what the hemicube sees IS sky visibility. No
//! distance falloff, no cosine kernel (the header's default weight function
//! is `1.0` and `example.c` never overrides it), no remap — an open flat
//! surface integrates to 1.0.
//!
//! # The port, function by function
//!
//! | lightmapper.h                                        | here                       |
//! |------------------------------------------------------|----------------------------|
//! | `lm_convexClip` / `lm_lineIntersection` / `lm_leftOf`| `convex_clip` etc.         |
//! | `lm_toBarycentric`                                   | `to_barycentric`           |
//! | `lm_setMeshPosition`                                 | `TriangleWalk::new`        |
//! | `lm_trySampling…RasterizerPosition`                  | `TriangleWalk::try_sample` |
//! | `lm_moveToNext…` / `lm_find{First,Next}…`            | `TriangleWalk::{move_next, find_first, find_next}` |
//! | `lm_passStepSize` / `lm_passOffset{X,Y}`             | `pass_step_size` / `pass_offset` |
//! | `lm_beginSampleHemisphere` (the 5 views)             | `HemiRenderer::render`     |
//! | `lmSetHemisphereWeights`                             | `hemisphere_weights`       |
//! | first pass + downsample shaders (weighted sum)       | `HemiRenderer::integrate`  |
//! | `lm_writeResultsToLightmap` (validity > 0.9 gate)    | `apply_results`            |
//! | `lmImageDilate` / `lmImageSmooth` / `lmImagePower`   | `image_dilate` / `image_smooth` / `image_power` |
//! | `LM_DEBUG_INTERPOLATION` texel statistics            | `LightmapStats` + `debug`  |
//!
//! # Faithful-but-not-identical (each deliberate, none affects semantics)
//!
//! * **No GPU.** The five hemicube faces are software-rasterized into the
//!   header's exact 3s x 1s framebuffer layout (`[C|R|L|D/U]`), with the
//!   header's exact view matrices and asymmetric frusta, depth-tested
//!   (`GL_LESS`), backface culling DISABLED, and `gl_FrontFacing` derived
//!   from screen-space winding — which is what the README's
//!   `(gl_FrontFacing ? 1.0 : 0.0)` validity marker is.
//! * **No hemi batching.** The header batches 64 hemicubes into one FBO to
//!   amortize GPU round trips, then applies all results to the lightmap at
//!   the END of a pass in storage-texture order. Here every job integrates
//!   immediately and results are applied at the end of the pass in
//!   rasterization order. The only observable difference is which of two
//!   SAME-PASS duplicate samples of one texel wins the first-write race —
//!   the header's winner is an artifact of its batch layout, ours of walk
//!   order; both deterministic.
//! * **`rand()` → xorshift.** The header jitters the hemisphere's roll angle
//!   by `0.1 * rand()/RAND_MAX` on top of its 3x3 base-angle pattern. libc
//!   rand is not reproducible across platforms; a fixed-seed xorshift32
//!   advanced in walk order is, and the jitter's only job is decorrelation.
//! * **One channel.** The scene is black-on-white by construction, so the
//!   lightmap is greyscale (`channels = 1` in header terms; the header's own
//!   c==1 path averages rgb, which for equal channels is the same value).
//! * **Coplanar twin determinism.** The target meshes are fully double-sided:
//!   every triangle has an opposite-winding twin over the SAME vertices,
//!   authored outward-face-first. On a GPU both interpolate bitwise-equal
//!   depth (same plane, same vertices), so `GL_LESS` keeps the first-drawn
//!   fragment — the outward face — and an interior view correctly reads
//!   backfaces. A CPU rasterizer must not let float noise break that tie, or
//!   the validity marker turns to salt-and-pepper; two measures keep it
//!   bitwise (see `raster_view`): the triangle's vertex sequence is
//!   canonicalized before clipping (so Sutherland–Hodgman hands both twins
//!   identical intersection points, with the true winding carried in a
//!   flag), and x/y frustum planes are never clipped geometrically at all
//!   (guard-band style, raster bounds clamped to the viewport instead,
//!   screen math in f64).

use makepad_draw::makepad_math::Vec3f;

// ---------------------------------------------------------------------------
// Small vector helpers (ports of the lm_vec2/lm_vec3 inlines actually used).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct V2 {
    x: f32,
    y: f32,
}

fn v2(x: f32, y: f32) -> V2 {
    V2 { x, y }
}

fn sub2(a: V2, b: V2) -> V2 {
    v2(a.x - b.x, a.y - b.y)
}

fn add2(a: V2, b: V2) -> V2 {
    v2(a.x + b.x, a.y + b.y)
}

fn dot2(a: V2, b: V2) -> f32 {
    a.x * b.x + a.y * b.y
}

/// `lm_cross2` — the pseudo cross product.
fn cross2(a: V2, b: V2) -> f32 {
    a.x * b.y - a.y * b.x
}

fn v3(x: f32, y: f32, z: f32) -> Vec3f {
    Vec3f { x, y, z }
}

fn add3(a: Vec3f, b: Vec3f) -> Vec3f {
    v3(a.x + b.x, a.y + b.y, a.z + b.z)
}

fn sub3(a: Vec3f, b: Vec3f) -> Vec3f {
    v3(a.x - b.x, a.y - b.y, a.z - b.z)
}

fn scale3(a: Vec3f, s: f32) -> Vec3f {
    v3(a.x * s, a.y * s, a.z * s)
}

fn neg3(a: Vec3f) -> Vec3f {
    v3(-a.x, -a.y, -a.z)
}

fn dot3(a: Vec3f, b: Vec3f) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

fn cross3(a: Vec3f, b: Vec3f) -> Vec3f {
    v3(
        a.y * b.z - b.y * a.z,
        a.z * b.x - b.z * a.x,
        a.x * b.y - b.x * a.y,
    )
}

fn length3sq(a: Vec3f) -> f32 {
    dot3(a, a)
}

fn normalize3(a: Vec3f) -> Vec3f {
    let l = length3sq(a).sqrt();
    scale3(a, 1.0 / l)
}

fn finite2(a: V2) -> bool {
    a.x.is_finite() && a.y.is_finite()
}

fn finite3(a: Vec3f) -> bool {
    a.x.is_finite() && a.y.is_finite() && a.z.is_finite()
}

/// `lm_pmodf` — the header's "positive mod", quirk included: it adds 1.0 (not
/// `b`) when `a` is negative. Only ever called with `b = 1.0`, where the two
/// agree.
fn pmodf(a: f32, b: f32) -> f32 {
    (if a < 0.0 { 1.0 } else { 0.0 }) + (a % b)
}

// ---------------------------------------------------------------------------
// lm_toBarycentric / lm_leftOf / lm_lineIntersection / lm_convexClip
// ---------------------------------------------------------------------------

/// Port of `lm_toBarycentric`. NOTE the header's convention: the returned
/// `u` runs along `p3 - p1` and `v` along `p2 - p1` — i.e. `u` weights the
/// THIRD vertex and `v` the SECOND. `try_sample` depends on this exact
/// pairing when it reconstructs the 3D position.
fn to_barycentric(p1: V2, p2: V2, p3: V2, p: V2) -> V2 {
    let v0 = sub2(p3, p1);
    let v1 = sub2(p2, p1);
    let v2_ = sub2(p, p1);
    let dot00 = dot2(v0, v0);
    let dot01 = dot2(v0, v1);
    let dot02 = dot2(v0, v2_);
    let dot11 = dot2(v1, v1);
    let dot12 = dot2(v1, v2_);
    let inv_denom = 1.0 / (dot00 * dot11 - dot01 * dot01);
    v2(
        (dot11 * dot02 - dot01 * dot12) * inv_denom,
        (dot00 * dot12 - dot01 * dot02) * inv_denom,
    )
}

/// Port of `lm_leftOf`: -1, 0 or 1.
fn left_of(a: V2, b: V2, c: V2) -> i32 {
    let x = cross2(sub2(b, a), sub2(c, b));
    if x < 0.0 {
        -1
    } else if x > 0.0 {
        1
    } else {
        0
    }
}

/// Port of `lm_lineIntersection`.
fn line_intersection(x0: V2, x1: V2, y0: V2, y1: V2) -> Option<V2> {
    let dx = sub2(x1, x0);
    let dy = sub2(y1, y0);
    let d = sub2(x0, y0);
    let dyx = cross2(dy, dx);
    if dyx == 0.0 {
        return None;
    }
    let dyx = cross2(d, dx) / dyx;
    if dyx <= 0.0 || dyx >= 1.0 {
        return None;
    }
    Some(v2(y0.x + dyx * dy.x, y0.y + dyx * dy.y))
}

/// Port of `lm_convexClip` — Sutherland–Hodgman with the header's exact
/// (slightly idiosyncratic) loop, including its in-place `poly` reuse.
fn convex_clip(poly: &mut [V2; 16], n_poly_in: usize, clip: &[V2], res: &mut [V2; 16]) -> usize {
    let n_clip = clip.len();
    let mut n_res = n_poly_in;
    let mut n_poly = n_poly_in;
    let dir = left_of(clip[0], clip[1], clip[2]);
    let mut j = n_clip - 1;
    for i in 0..n_clip {
        if n_res == 0 {
            break;
        }
        if i != 0 {
            poly[..n_res].copy_from_slice(&res[..n_res]);
            n_poly = n_res;
        }
        n_res = 0;
        let mut v0 = poly[n_poly - 1];
        let mut side0 = left_of(clip[j], clip[i], v0);
        if side0 != -dir {
            res[n_res] = v0;
            n_res += 1;
        }
        for k in 0..n_poly {
            let v1 = poly[k];
            let side1 = left_of(clip[j], clip[i], v1);
            if side0 + side1 == 0 && side0 != 0 {
                if let Some(x) = line_intersection(clip[j], clip[i], v0, v1) {
                    res[n_res] = x;
                    n_res += 1;
                }
            }
            if k == n_poly - 1 {
                break;
            }
            if side1 != -dir {
                res[n_res] = v1;
                n_res += 1;
            }
            v0 = v1;
            side0 = side1;
        }
        j = i;
    }
    n_res
}

// ---------------------------------------------------------------------------
// Pass scheduling (lm_passStepSize / lm_passOffsetX / lm_passOffsetY)
// ---------------------------------------------------------------------------

/// Port of `lm_passStepSize`. C's truncating division is what makes pass 0
/// use the full `2^interpolationPasses` step: `(0 - 1) / 3 == 0` in C and in
/// Rust's `i32` division alike.
fn pass_step_size(pass: i32, pass_count: i32) -> i32 {
    let shift = pass_count / 3 - (pass - 1) / 3;
    let step = 1i32 << shift;
    debug_assert!(step > 0);
    step
}

/// Port of `lm_passOffsetX`/`lm_passOffsetY`, fused: returns `(off_x, off_y)`.
fn pass_offset(pass: i32, pass_count: i32) -> (i32, i32) {
    if pass == 0 {
        return (0, 0);
    }
    let pass_type = (pass - 1) % 3;
    let half_step = pass_step_size(pass, pass_count) >> 1;
    (
        if pass_type != 1 { half_step } else { 0 },
        if pass_type != 0 { half_step } else { 0 },
    )
}

// ---------------------------------------------------------------------------
// Sample-position jitter (lm_baseAngles + rand())
// ---------------------------------------------------------------------------

const BASE_ANGLE: f32 = 0.1;
/// Port of `lm_baseAngles` — the 3x3 hemisphere-roll pattern.
const BASE_ANGLES: [[f32; 3]; 3] = [
    [BASE_ANGLE, BASE_ANGLE + 1.0 / 3.0, BASE_ANGLE + 2.0 / 3.0],
    [BASE_ANGLE + 1.0 / 3.0, BASE_ANGLE + 2.0 / 3.0, BASE_ANGLE],
    [BASE_ANGLE + 2.0 / 3.0, BASE_ANGLE, BASE_ANGLE + 1.0 / 3.0],
];

/// Stand-in for the header's `rand() / RAND_MAX`: xorshift32, fixed seed,
/// advanced in walk order — deterministic where libc rand is not.
struct Rng(u32);

impl Rng {
    fn next01(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
}

// ---------------------------------------------------------------------------
// Parameters / results
// ---------------------------------------------------------------------------

/// The `lmCreate` arguments that matter for AO, defaults straight from
/// `example.c`.
#[derive(Clone)]
pub struct LightmapParams {
    /// Hemicube face resolution ("hemisphereSize"). Power of two, 16..512.
    pub hemisphere_size: usize,
    pub z_near: f32,
    pub z_far: f32,
    /// Sky ("clear") color. Greyscale because the AO scene is.
    pub clear_color: f32,
    /// Hierarchical selective interpolation passes (0..=8).
    pub interpolation_passes: usize,
    /// Error below which texels interpolate instead of render.
    pub interpolation_threshold: f32,
    /// "cameraToSurfaceDistanceModifier": -1 sticks to the surface, 0 is the
    /// minimum lift for interpolated normals.
    pub camera_to_surface_distance_modifier: f32,
    /// The `lmSetHemisphereWeights` material term: `false` = the header's
    /// default `f(cos_theta) = 1` (plain sky visibility, what example.c
    /// bakes), `true` = `f(cos_theta) = cos_theta` (Lambertian irradiance —
    /// grazing sky counts less, so creases read softer and open faces stay
    /// bright).
    pub cosine_weighted: bool,
    /// The validity gate: a hemicube whose front-facing (non-backface)
    /// fraction is at or below this is REJECTED and its texel left for the
    /// buried fill. 0.9 is the header's constant — right for meshes whose
    /// members meet at faces. Kenney kits interpenetrate members DEEPLY, and
    /// a sample buried just inside a neighbour keeps a sliver of valid sky
    /// above the neighbour's wall: at 0.9 that sliver PASSES the gate and
    /// the texel bakes as if it saw only sky — a bright wedge in the middle
    /// of a joint. The atlas bake gates stricter ([`LightmapParams::atlas`]).
    pub validity_min: f32,
}

impl Default for LightmapParams {
    fn default() -> Self {
        Self {
            hemisphere_size: 64,
            z_near: 0.001,
            z_far: 100.0,
            clear_color: 1.0,
            interpolation_passes: 2,
            interpolation_threshold: 0.01,
            camera_to_surface_distance_modifier: 0.0,
            cosine_weighted: false,
            validity_min: 0.9,
        }
    }
}

impl LightmapParams {
    /// The parameter set the ATLAS bake uses (and the mapping test's oracle
    /// must mirror): example.c's AO parameters with the Lambertian cosine
    /// kernel and a validity gate placed between the kits' two measured
    /// populations.
    ///
    /// The gate sat at 0.98 for one release. That misclassified GRAZING
    /// samples — texels on a surface that meets a flush neighbour, whose
    /// hemicube view is honest but skims the neighbour's contact plane as
    /// backface (measured on the gate cornices: uniform-measure validity
    /// 0.87-0.98 with a perfectly renderable vis) — and their rejection is
    /// what fed whole visible strips to the buried fill, which then stepped
    /// mid-surface. With validity measured in the cosine kernel's own
    /// measure (see [`hemisphere_weights`]) the populations separate:
    ///
    /// * honest views bottom out at ~0.86 — the floor templates' groove
    ///   walls, which see the surrounding zero-thickness SHEET's underside
    ///   in a horizon band (dedup dropped the twin that read as matter
    ///   there; the models are sheets, not solids);
    /// * genuinely buried samples top out at ~0.49 (a keystone joint
    ///   slot), with flush-buried strips at 0.0.
    ///
    /// 0.7 sits between them with margin on both sides. The bright-wedge
    /// failure the 0.98 gate was chasing was the MEAN buried-fill's doing
    /// and is fixed in the fill itself ([`crate::ao_atlas::bake_into`]'s
    /// harmonic fill).
    pub fn atlas() -> Self {
        Self {
            cosine_weighted: true,
            validity_min: 0.7,
            ..Self::default()
        }
    }
}

/// Texel accounting, superset of the header's `LM_DEBUG_INTERPOLATION` stats.
#[derive(Default, Debug, Clone)]
pub struct LightmapStats {
    /// Hemicubes actually rendered (jobs).
    pub hemicubes_rendered: usize,
    /// Hemicube results discarded by the validity gate (`validity <= 0.9`) —
    /// samples whose view was mostly backfaces, i.e. taken from inside
    /// geometry.
    pub hemicubes_rejected: usize,
    /// Texels that received a valid hemicube result.
    pub texels_rendered: usize,
    /// Texels filled by hierarchical interpolation.
    pub texels_interpolated: usize,
    /// Texels that were rendered at least once but NEVER produced a valid
    /// result and were never interpolated — left at zero for dilation.
    /// Interpenetrating joints land here.
    pub texels_rejected: usize,
    /// Texels never touched by any triangle (gutters, padding).
    pub texels_wasted: usize,
}

/// A finished bake: linear float lightmap (0 = untouched/invalid, else
/// occlusion in (0, 1]) plus the debug/stat sidecars.
pub struct LightmapBake {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
    /// RGB debug image, the header's `LM_DEBUG_INTERPOLATION` output: red =
    /// rendered, green = interpolated, blue = rendered but rejected by
    /// validity (ours; the header leaves those black).
    pub debug: Vec<u8>,
    pub stats: LightmapStats,
}

// ---------------------------------------------------------------------------
// The conservative triangle rasterizer walk (meshPosition.*)
// ---------------------------------------------------------------------------

/// One hemicube to render: the texel it belongs to and the camera basis
/// `try_sample` derived for it.
struct Job {
    x: i32,
    y: i32,
    pos: Vec3f,
    dir: Vec3f,
    up: Vec3f,
}

/// `ctx->meshPosition` for one triangle: its lightmap-space UVs, world
/// positions/normals, and the conservative rasterizer cursor.
struct TriangleWalk {
    p: [Vec3f; 3],
    n: [Vec3f; 3],
    uv: [V2; 3],
    minx: i32,
    miny: i32,
    maxx: i32,
    maxy: i32,
    x: i32,
    y: i32,
}

impl TriangleWalk {
    /// Port of `lm_setMeshPosition` (identity model matrix — callers bake
    /// their transform into the vertices, as the repo's props already do).
    /// Returns `None` when the initial cursor is out of range, the header's
    /// "no samples on this triangle" state.
    #[allow(clippy::too_many_arguments)]
    fn new(
        tri: usize,
        positions: &[Vec3f],
        normals: Option<&[Vec3f]>,
        uvs: &[[f32; 2]],
        indices: &[u32],
        width: usize,
        height: usize,
        pass: i32,
        pass_count: i32,
    ) -> Option<Self> {
        let uv_scale = v2(width as f32, height as f32);
        let mut p = [v3(0.0, 0.0, 0.0); 3];
        let mut uv = [v2(0.0, 0.0); 3];
        let mut uv_min = v2(f32::MAX, f32::MAX);
        let mut uv_max = v2(-f32::MAX, -f32::MAX);
        let mut vi = [0usize; 3];
        for i in 0..3 {
            vi[i] = indices[tri * 3 + i] as usize;
            p[i] = positions[vi[i]];
            let raw = uvs[vi[i]];
            // pmod-wrap into [0,1) then scale to texels, exactly as the
            // header does for LM_FLOAT uvs.
            uv[i] = v2(pmodf(raw[0], 1.0) * uv_scale.x, pmodf(raw[1], 1.0) * uv_scale.y);
            uv_min = v2(uv_min.x.min(uv[i].x), uv_min.y.min(uv[i].y));
            uv_max = v2(uv_max.x.max(uv[i].x), uv_max.y.max(uv[i].y));
        }

        // LM_NONE normals: the triangle's own flat normal, like example.c.
        let flat = cross3(sub3(p[1], p[0]), sub3(p[2], p[0]));
        let mut n = [v3(0.0, 1.0, 0.0); 3];
        for i in 0..3 {
            let raw = match normals {
                Some(ns) => ns[vi[i]],
                None => flat,
            };
            n[i] = normalize3(raw);
        }

        // Area of interest for conservative rasterization.
        let bb_min = v2(uv_min.x.floor(), uv_min.y.floor());
        let bb_max = v2(uv_max.x.ceil(), uv_max.y.ceil());
        let minx = (bb_min.x as i32 - 1).max(0);
        let miny = (bb_min.y as i32 - 1).max(0);
        let maxx = (bb_max.x as i32 + 1).min(width as i32 - 1);
        let maxy = (bb_max.y as i32 + 1).min(height as i32 - 1);
        debug_assert!(minx <= maxx && miny <= maxy);
        let (off_x, off_y) = pass_offset(pass, pass_count);
        let x = minx + off_x;
        let y = miny + off_y;
        if x <= maxx && y <= maxy {
            Some(Self { p, n, uv, minx, miny, maxx, maxy, x, y })
        } else {
            None
        }
    }

    /// `lm_hasConservativeTriangleRasterizerFinished`.
    fn finished(&self) -> bool {
        self.y >= self.maxy
    }
}

// ---------------------------------------------------------------------------
// Hemicube software renderer
// ---------------------------------------------------------------------------

/// Weight table, port of `lmSetHemisphereWeights` with a pluggable
/// `f(cos_theta)` (default `1.0`, as in the header and example.c).
///
/// Layout matches the header's weights texture: `3*size` wide, `size` tall;
/// cell `(x, y)` weighs framebuffer pixel `(x, y)` of the `[C|R|L|D/U]`
/// hemicube layout. Returns `(color_weight, validity_weight)` per cell,
/// normalized so a fully valid hemicube integrates to 1.
///
/// # Validity is measured in the KERNEL's own measure
///
/// The header weighs the validity channel by plain solid angle. Under the
/// default `f = 1` kernel that IS the integral's measure and this code is
/// identical to the port. Under the cosine kernel the two measures split,
/// and the plain-solid-angle validity misclassifies: a texel on a surface
/// that meets a flush neighbour sees a thin band of that neighbour's
/// contact plane as backface exactly AT the horizon — up to ~13% of solid
/// angle (measured on the gate pillars) yet nearly nothing of the cosine
/// integral being computed — while a genuinely buried sample wears its
/// backfaces high in the view where the cosine measure weighs them MORE.
/// Weighing validity by `solid_angle * f` makes the validity gate ask the
/// only meaningful question — "how much of what I am integrating is
/// trustworthy" — which passes the grazing texels (honest values) and
/// rejects burial harder. It also makes the `1/validity` renormalization
/// in `apply_results` exact: previously a cosine-weighted colour was
/// divided by a uniform-weighted validity, which overshot (open texels
/// baked >1) wherever the horizon band was large.
pub fn hemisphere_weights(size: usize, f: impl Fn(f32) -> f32) -> Vec<(f32, f32)> {
    let mut weights = vec![(0.0f32, 0.0f32); 3 * size * size];
    let center = (size as f32 - 1.0) * 0.5;
    let mut sum = 0.0f64;
    let mut sum_v = 0.0f64;
    for y in 0..size {
        let dy = 2.0 * (y as f32 - center) / size as f32;
        for x in 0..size {
            let dx = 2.0 * (x as f32 - center) / size as f32;
            let v = normalize3(v3(dx, dy, 1.0));
            let solid_angle = v.z * v.z * v.z;
            // center face
            let wc = solid_angle * f(v.z);
            weights[y * (3 * size) + x] = (wc, wc);
            // left/right faces
            let wl = solid_angle * f(v.x.abs());
            weights[y * (3 * size) + size + x] = (wl, wl);
            // up/down faces
            let wu = solid_angle * f(v.y.abs());
            weights[y * (3 * size) + 2 * size + x] = (wu, wu);
            sum += 3.0 * solid_angle as f64;
            sum_v += (wc + wl + wu) as f64;
        }
    }
    let scale = (1.0 / sum) as f32;
    let scale_v = (1.0 / sum_v) as f32;
    for w in &mut weights {
        w.0 *= scale;
        w.1 *= scale_v;
    }
    weights
}

/// One thread's hemicube framebuffer: color + validity + depth over the
/// header's `3*size x size` five-face layout.
struct HemiRenderer<'a> {
    size: usize,
    w: usize,
    h: usize,
    z_near: f32,
    z_far: f32,
    clear_color: f32,
    color: Vec<f32>,
    valid: Vec<f32>,
    depth: Vec<f32>,
    weights: &'a [(f32, f32)],
    /// World-space triangles, in draw order. Order matters: coplanar twins
    /// resolve their depth tie to the FIRST drawn (see module docs).
    tris: &'a [[Vec3f; 3]],
}

/// Row-major 4x4, acting on column vectors.
#[derive(Clone, Copy)]
struct M4([f32; 16]);

impl M4 {
    fn mul_point(&self, p: Vec3f) -> [f32; 4] {
        let m = &self.0;
        [
            m[0] * p.x + m[1] * p.y + m[2] * p.z + m[3],
            m[4] * p.x + m[5] * p.y + m[6] * p.z + m[7],
            m[8] * p.x + m[9] * p.y + m[10] * p.z + m[11],
            m[12] * p.x + m[13] * p.y + m[14] * p.z + m[15],
        ]
    }
}

/// Port of `lm_setView`'s two matrices, fused to one clip transform:
/// `frustum(l,r,b,t,n,f) * lookAt(pos, pos+dir, up)`.
fn view_proj(pos: Vec3f, dir: Vec3f, up: Vec3f, l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> M4 {
    let side = cross3(dir, up);
    // view rows (the header stores column-major GL; these are the same rows).
    let vx = [side.x, side.y, side.z, -dot3(side, pos)];
    let vy = [up.x, up.y, up.z, -dot3(up, pos)];
    let vz = [-dir.x, -dir.y, -dir.z, dot3(dir, pos)];
    // frustum
    let ilr = 1.0 / (r - l);
    let ibt = 1.0 / (t - b);
    let ninf = -1.0 / (f - n);
    let n2 = 2.0 * n;
    // proj rows: [n2*ilr, 0, (r+l)*ilr, 0] / [0, n2*ibt, (t+b)*ibt, 0]
    //            [0, 0, (f+n)*ninf, f*n2*ninf] / [0, 0, -1, 0]
    let mut m = [0.0f32; 16];
    for c in 0..4 {
        let v = [vx[c], vy[c], vz[c], if c == 3 { 1.0 } else { 0.0 }];
        m[c] = n2 * ilr * v[0] + (r + l) * ilr * v[2];
        m[4 + c] = n2 * ibt * v[1] + (t + b) * ibt * v[2];
        m[8 + c] = (f + n) * ninf * v[2] + f * n2 * ninf * v[3];
        m[12 + c] = -v[2];
    }
    M4(m)
}

/// A vertex in clip space, for the homogeneous polygon clipper.
type Clip4 = [f32; 4];

/// Signed distance to clip plane `i` (0..=5: -x,+x,-y,+y,near,far), plus the
/// w >= eps guard plane as i == 6. Positive = inside.
fn plane_dist(v: &Clip4, i: usize) -> f32 {
    match i {
        0 => v[3] + v[0],
        1 => v[3] - v[0],
        2 => v[3] + v[1],
        3 => v[3] - v[1],
        4 => v[3] + v[2], // near: z >= -w
        5 => v[3] - v[2], // far:  z <= w
        _ => v[3] - 1.0e-6,
    }
}

impl<'a> HemiRenderer<'a> {
    fn new(
        size: usize,
        z_near: f32,
        z_far: f32,
        clear_color: f32,
        weights: &'a [(f32, f32)],
        tris: &'a [[Vec3f; 3]],
    ) -> Self {
        let (w, h) = (3 * size, size);
        Self {
            size,
            w,
            h,
            z_near,
            z_far,
            clear_color,
            color: vec![0.0; w * h],
            valid: vec![0.0; w * h],
            depth: vec![0.0; w * h],
            tris,
            weights,
        }
    }

    /// Render the five hemicube faces for one sample and integrate them.
    /// Returns `(weighted_color, weighted_validity)` — the header's storage
    /// texel.
    fn render(&mut self, job: &Job) -> (f32, f32) {
        // glClear to "valid background pixels": sky color, alpha 1.
        self.color.fill(self.clear_color);
        self.valid.fill(1.0);
        self.depth.fill(1.0);

        let size = self.size as i32;
        let n = self.z_near;
        let f = self.z_far;
        let pos = job.pos;
        let dir = job.dir;
        let up = job.up;
        let right = cross3(dir, up);

        // The header's five views, viewports relative to one hemicube:
        //   +-------+---+---+-------+
        //   |       |   |   |   D   |
        //   |   C   | R | L +-------+
        //   |       |   |   |   U   |
        //   +-------+---+---+-------+
        let views: [(i32, i32, i32, i32, Vec3f, Vec3f, [f32; 4]); 5] = [
            (0, 0, size, size, dir, up, [-n, n, -n, n]),
            (size, 0, size / 2, size, right, up, [-n, 0.0, -n, n]),
            (size + size / 2, 0, size / 2, size, neg3(right), up, [0.0, n, -n, n]),
            (2 * size, size / 2, size, size / 2, neg3(up), dir, [-n, n, 0.0, n]),
            (2 * size, 0, size, size / 2, up, neg3(dir), [-n, n, -n, 0.0]),
        ];
        for (vx, vy, vw, vh, d, u, fr) in views {
            let m = view_proj(pos, d, u, fr[0], fr[1], fr[2], fr[3], n, f);
            self.raster_view(&m, vx, vy, vw, vh);
        }
        self.integrate()
    }

    /// Draw every triangle into one face's viewport: homogeneous clip against
    /// NEAR and FAR only, perspective divide, viewport map, edge-function
    /// raster with depth LESS — culling disabled, validity = front-facing.
    ///
    /// # Guard-band rasterization, and why side planes are NOT clipped
    ///
    /// Clipping x/y geometrically would introduce new vertices, and
    /// Sutherland–Hodgman walks a polygon in vertex order — so a triangle and
    /// its reversed-winding coplanar twin (the double-sided models this bakes)
    /// would get intersection points that differ in the last ulp, their depth
    /// values would stop being bitwise equal, and the twins' depth tie —
    /// which MUST go to the first-drawn, outward face for the validity marker
    /// to read interiors as backfaces — would break per pixel (measured: 20%
    /// of every surface misread as backface). Real GPUs sidestep the same
    /// hazard with guard-band clipping: primitives within the band are
    /// rasterized from one plane equation, unclipped. This does the
    /// equivalent — raster bounds clamp to the viewport and the screen-space
    /// math runs in f64, which holds the post-divide coordinates (up to
    /// ~model_extent/z_near pixels off screen) losslessly enough that the
    /// tie-by-equality survives.
    fn raster_view(&mut self, m: &M4, vx: i32, vy: i32, vw: i32, vh: i32) {
        let mut poly: Vec<Clip4> = Vec::with_capacity(8);
        let mut next: Vec<Clip4> = Vec::with_capacity(8);
        for tri in self.tris {
            // Transform.
            let c0 = m.mul_point(tri[0]);
            let c1 = m.mul_point(tri[1]);
            let c2 = m.mul_point(tri[2]);
            // Trivial reject: all three outside one frustum plane.
            let mut out = false;
            for pl in 0..6 {
                if plane_dist(&c0, pl) < 0.0 && plane_dist(&c1, pl) < 0.0 && plane_dist(&c2, pl) < 0.0
                {
                    out = true;
                    break;
                }
            }
            if out {
                continue;
            }
            // Canonicalize the vertex sequence BEFORE clipping: start at the
            // lexicographically smallest clip-space vertex and walk toward
            // the smaller neighbor. A triangle and its reversed twin then
            // present the clipper with the IDENTICAL sequence, so clip
            // intersection points — which Sutherland–Hodgman computes in
            // traversal order — come out bitwise equal and the depth tie
            // between the twins survives near-plane clipping. The original
            // orientation is carried in `flip`.
            let verts = [c0, c1, c2];
            let key = |v: &Clip4| (v[0], v[1], v[2], v[3]);
            let mut m = 0;
            for i in 1..3 {
                if key(&verts[i]) < key(&verts[m]) {
                    m = i;
                }
            }
            let fwd = key(&verts[(m + 1) % 3]);
            let bwd = key(&verts[(m + 2) % 3]);
            let flip = bwd < fwd; // walk backward = reversed orientation
            let seq = if flip {
                [verts[m], verts[(m + 2) % 3], verts[(m + 1) % 3]]
            } else {
                [verts[m], verts[(m + 1) % 3], verts[(m + 2) % 3]]
            };
            // Clip against w-guard, near and far only (see above).
            poly.clear();
            poly.extend_from_slice(&seq);
            for pl in [6usize, 4, 5] {
                next.clear();
                let np = poly.len();
                if np == 0 {
                    break;
                }
                for i in 0..np {
                    let a = poly[i];
                    let b = poly[(i + 1) % np];
                    let da = plane_dist(&a, pl);
                    let db = plane_dist(&b, pl);
                    if da >= 0.0 {
                        next.push(a);
                    }
                    if (da >= 0.0) != (db >= 0.0) {
                        let t = da / (da - db);
                        next.push([
                            a[0] + t * (b[0] - a[0]),
                            a[1] + t * (b[1] - a[1]),
                            a[2] + t * (b[2] - a[2]),
                            a[3] + t * (b[3] - a[3]),
                        ]);
                    }
                }
                std::mem::swap(&mut poly, &mut next);
            }
            if poly.len() < 3 {
                continue;
            }
            // Divide + viewport map (GL convention, y up, pixel centers at
            // +0.5) — f64 from here on, see above.
            let mut scr: Vec<(f64, f64, f64)> = Vec::with_capacity(poly.len());
            for c in &poly {
                let inv_w = 1.0 / c[3] as f64;
                let ndc = (c[0] as f64 * inv_w, c[1] as f64 * inv_w, c[2] as f64 * inv_w);
                scr.push((
                    vx as f64 + (ndc.0 * 0.5 + 0.5) * vw as f64,
                    vy as f64 + (ndc.1 * 0.5 + 0.5) * vh as f64,
                    ndc.2 * 0.5 + 0.5,
                ));
            }
            // Fan-triangulate, raster each. `flip` restores the original
            // winding's facing.
            for k in 1..scr.len() - 1 {
                self.raster_tri(scr[0], scr[k], scr[k + 1], flip, vx, vy, vw, vh);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn raster_tri(
        &mut self,
        a: (f64, f64, f64),
        b: (f64, f64, f64),
        c: (f64, f64, f64),
        flip: bool,
        vx: i32,
        vy: i32,
        vw: i32,
        vh: i32,
    ) {
        // Orientation from the DRAWN winding (`flip` undoes the pre-clip
        // canonicalization): CCW in y-up window coords is front-facing —
        // GL's default, and glTF's.
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area == 0.0 || !area.is_finite() {
            return;
        }
        let front = (area > 0.0) != flip;
        let inv_area = 1.0 / area;

        let min_x = (a.0.min(b.0).min(c.0).floor().max(vx as f64)) as i32;
        let max_x = (a.0.max(b.0).max(c.0).ceil().min((vx + vw) as f64)) as i32;
        let min_y = (a.1.min(b.1).min(c.1).floor().max(vy as f64)) as i32;
        let max_y = (a.1.max(b.1).max(c.1).ceil().min((vy + vh) as f64)) as i32;
        if min_x >= max_x || min_y >= max_y {
            return;
        }

        for py in min_y..max_y {
            let fy = py as f64 + 0.5;
            for px in min_x..max_x {
                let fx = px as f64 + 0.5;
                // Barycentric via edge functions (normalized by signed area,
                // so "inside" is all-nonnegative for either orientation).
                let l0 = ((b.0 - fx) * (c.1 - fy) - (b.1 - fy) * (c.0 - fx)) * inv_area;
                let l1 = ((c.0 - fx) * (a.1 - fy) - (c.1 - fy) * (a.0 - fx)) * inv_area;
                let l2 = ((a.0 - fx) * (b.1 - fy) - (a.1 - fy) * (b.0 - fx)) * inv_area;
                if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
                    continue;
                }
                // Window z is affine in screen space: plain barycentric.
                let z = (l0 * a.2 + l1 * b.2 + l2 * c.2) as f32;
                let i = py as usize * self.w + px as usize;
                if z < self.depth[i] {
                    self.depth[i] = z;
                    self.color[i] = 0.0; // geometry is black
                    self.valid[i] = if front { 1.0 } else { 0.0 };
                }
            }
        }
    }

    /// The header's two-shader weighted reduction, collapsed: the first pass
    /// multiplies each pixel by its weight pair and every later pass merely
    /// sums, so the whole chain is one weighted sum over the 3s x s layout.
    fn integrate(&self) -> (f32, f32) {
        let mut c = 0.0f32;
        let mut v = 0.0f32;
        for i in 0..self.w * self.h {
            let (wc, wv) = self.weights[i];
            c += self.color[i] * wc;
            v += self.valid[i] * wv;
        }
        (c, v)
    }
}

// ---------------------------------------------------------------------------
// The bake driver (lmBegin/lmEnd loop, restructured but order-preserving)
// ---------------------------------------------------------------------------

/// Bake sky-visibility AO for a mesh into a lightmap, ands/lightmapper
/// semantics.
///
/// * `positions` — world-space vertices (bake your transform in first).
/// * `normals` — optional; `None` = the header's `LM_NONE` (flat normals from
///   the winding), which is what example.c uses.
/// * `uvs` — lightmap coordinates in [0,1], e.g. the atlas uvs
///   [`crate::ao_atlas::bake_into`] emits.
/// * `indices` — triangle list.
///
/// Backface culling is disabled and backface fragments are marked invalid;
/// texels whose hemicube integrates to `validity <= 0.9` are rejected and
/// left at 0.0 for dilation, per the README's quality advice.
pub fn bake_ao(
    positions: &[Vec3f],
    normals: Option<&[Vec3f]>,
    uvs: &[[f32; 2]],
    indices: &[u32],
    width: usize,
    height: usize,
    params: &LightmapParams,
) -> LightmapBake {
    bake_ao_inner(positions, normals, uvs, indices, width, height, params, None)
}

pub fn bake_ao_with_pool(
    pool: &makepad_draw::makepad_platform::thread::TaskPool,
    positions: &[Vec3f],
    normals: Option<&[Vec3f]>,
    uvs: &[[f32; 2]],
    indices: &[u32],
    width: usize,
    height: usize,
    params: &LightmapParams,
) -> LightmapBake {
    bake_ao_inner(positions, normals, uvs, indices, width, height, params, Some(pool))
}

pub(crate) fn bake_ao_inner(
    positions: &[Vec3f],
    normals: Option<&[Vec3f]>,
    uvs: &[[f32; 2]],
    indices: &[u32],
    width: usize,
    height: usize,
    params: &LightmapParams,
    pool: Option<&makepad_draw::makepad_platform::thread::TaskPool>,
) -> LightmapBake {
    assert!(params.hemisphere_size.is_power_of_two() && (16..=512).contains(&params.hemisphere_size));
    assert!(params.z_near < params.z_far && params.z_near > 0.0);
    assert!(params.camera_to_surface_distance_modifier >= -1.0);
    assert!(params.interpolation_passes <= 8);
    assert!(params.interpolation_threshold >= 0.0);

    let tri_count = indices.len() / 3;
    let pass_count = 1 + 3 * params.interpolation_passes as i32;
    let mut data = vec![0.0f32; width * height];
    let mut debug = vec![0u8; width * height * 3];
    let mut stats = LightmapStats::default();
    // Texels that were rendered but whose every hemicube was rejected.
    let mut rejected = vec![false; width * height];

    // World-space triangle list for the renderer, in draw order.
    let tris: Vec<[Vec3f; 3]> = (0..tri_count)
        .map(|t| {
            [
                positions[indices[t * 3] as usize],
                positions[indices[t * 3 + 1] as usize],
                positions[indices[t * 3 + 2] as usize],
            ]
        })
        .collect();

    // Cosine kernel instead of the header's default 1.0 — horizon occluders
    // (parallel walls across gaps, distant clutter) lose influence, overhead
    // contacts keep it. This is the lightmapper-native dial toward "shade
    // only what presses on the surface".
    let cosine = params.cosine_weighted;
    let weights = hemisphere_weights(params.hemisphere_size, move |cos_theta| {
        if cosine { cos_theta } else { 1.0 }
    });
    let mut rng = Rng(0x2545_f491);

    for pass in 0..pass_count {
        // --- Walk: the conservative rasterizer over every triangle --------
        // (sequential and cheap; hemicube jobs are collected and rendered in
        // parallel below, then applied in this same order — see module docs
        // for why that is equivalent to the header's batch flow.)
        let step = pass_step_size(pass, pass_count);
        let (off_x, _off_y) = pass_offset(pass, pass_count);
        let mut queue: Vec<Job> = Vec::new();
        for tri in 0..tri_count {
            let Some(mut w) = TriangleWalk::new(
                tri, positions, normals, uvs, indices, width, height, pass, pass_count,
            ) else {
                continue;
            };
            // lm_findFirstConservativeTriangleRasterizerPosition + the
            // lmBegin loop: visit every cursor position of this pass's grid,
            // queueing a hemicube wherever try_sample says "render".
            loop {
                if try_sample(
                    &w, pass, step, params, &mut data, &mut debug, &mut stats, &mut rng, width,
                    &mut queue,
                ) {
                    // job queued (the header renders it here; we defer)
                }
                // move to next potential position
                w.x += step;
                while w.x >= w.maxx {
                    w.x = w.minx + off_x;
                    w.y += step;
                    if w.finished() {
                        break;
                    }
                }
                if w.finished() {
                    break;
                }
            }
        }

        // --- Render the queued hemicubes, in parallel ----------------------
        stats.hemicubes_rendered += queue.len();
        let results: Vec<(f32, f32)> = {
            let mut out = vec![(0.0f32, 0.0f32); queue.len()];
            let threads = pool
                .map_or(1, |pool| pool.heavy_workers().saturating_add(1))
                .min(queue.len().max(1));
            let queue = &queue;
            let tris = &tris;
            let weights = &weights;
            let (tx, rx) = std::sync::mpsc::channel();
            let work = |worker: usize| {
                let mut renderer = HemiRenderer::new(
                    params.hemisphere_size,
                    params.z_near,
                    params.z_far,
                    params.clear_color,
                    weights,
                    tris,
                );
                let mut i = worker;
                while i < queue.len() {
                    let _ = tx.send((i, renderer.render(&queue[i])));
                    i += threads;
                }
            };
            match pool {
                Some(pool) => pool.fan_out(
                    makepad_draw::makepad_platform::thread::Lane::Heavy,
                    threads,
                    work,
                ),
                None => (0..threads).for_each(work),
            }
            drop(tx);
            for (i, result) in rx {
                out[i] = result;
            }
            out
        };

        // --- lm_writeResultsToLightmap: apply in order, first write wins ---
        for (job, (c, validity)) in queue.iter().zip(&results) {
            let i = job.y as usize * width + job.x as usize;
            if data[i] == 0.0 && *validity > params.validity_min {
                let scale = 1.0 / validity;
                // channels == 1: (c0+c1+c2)*scale/3, all channels equal.
                data[i] = (c * scale).max(f32::MIN_POSITIVE);
                debug[i * 3] = 255; // red: rendered
                stats.texels_rendered += 1;
                rejected[i] = false;
            } else if *validity <= params.validity_min {
                stats.hemicubes_rejected += 1;
                if data[i] == 0.0 {
                    rejected[i] = true;
                }
            }
        }
    }

    // Final accounting.
    for i in 0..width * height {
        if rejected[i] && data[i] == 0.0 {
            stats.texels_rejected += 1;
            debug[i * 3 + 2] = 255; // blue: rendered but never valid
        } else if data[i] == 0.0 {
            stats.texels_wasted += 1;
        }
    }

    LightmapBake { width, height, data, debug, stats }
}

/// Port of `lm_trySamplingConservativeTriangleRasterizerPosition`.
///
/// Returns true when a hemicube job was queued for the current cursor —
/// interpolated, already-set, uncovered and degenerate positions all return
/// false, exactly like the header (which returns "render me" only).
#[allow(clippy::too_many_arguments)]
fn try_sample(
    w: &TriangleWalk,
    pass: i32,
    step: i32,
    params: &LightmapParams,
    data: &mut [f32],
    debug: &mut [u8],
    stats: &mut LightmapStats,
    rng: &mut Rng,
    width: usize,
    queue: &mut Vec<Job>,
) -> bool {
    if w.finished() {
        return false;
    }
    let idx = w.y as usize * width + w.x as usize;

    // Skip if the lightmap pixel was already set.
    if data[idx] != 0.0 {
        return false;
    }

    // Clip the pixel square against the triangle to find the covered area's
    // centroid.
    let mut pixel = [v2(0.0, 0.0); 16];
    pixel[0] = v2(w.x as f32, w.y as f32);
    pixel[1] = v2(w.x as f32 + 1.0, w.y as f32);
    pixel[2] = v2(w.x as f32 + 1.0, w.y as f32 + 1.0);
    pixel[3] = v2(w.x as f32, w.y as f32 + 1.0);
    let mut res = [v2(0.0, 0.0); 16];
    let n_res = convex_clip(&mut pixel, 4, &w.uv, &mut res);
    if n_res == 0 {
        return false; // nothing left
    }

    // Centroid position and area, the header's exact shoelace loop.
    let mut centroid = res[0];
    let mut area = res[n_res - 1].x * res[0].y - res[n_res - 1].y * res[0].x;
    for i in 1..n_res {
        centroid = add2(centroid, res[i]);
        area += res[i - 1].x * res[i].y - res[i - 1].y * res[i].x;
    }
    let centroid = v2(centroid.x / n_res as f32, centroid.y / n_res as f32);
    let area = (area / 2.0).abs();
    if area <= 0.0 {
        return false; // no area left
    }

    let uv = to_barycentric(w.uv[0], w.uv[1], w.uv[2], centroid);
    if !finite2(uv) {
        return false; // degenerate
    }

    // Hierarchical interpolation from neighbors, passes > 0 only.
    if pass > 0 {
        let d = step / 2;
        let dirs = ((pass - 1) % 3) + 1;
        let mut neighbors: [f32; 4] = [0.0; 4];
        let mut neighbor_count = 0usize;
        let mut neighbors_expected = 0usize;
        if dirs & 1 != 0 {
            neighbors_expected += 2;
            if w.x - d >= w.minx && w.x + d <= w.maxx {
                neighbors[neighbor_count] = data[w.y as usize * width + (w.x - d) as usize];
                neighbors[neighbor_count + 1] = data[w.y as usize * width + (w.x + d) as usize];
                neighbor_count += 2;
            }
        }
        if dirs & 2 != 0 {
            neighbors_expected += 2;
            if w.y - d >= w.miny && w.y + d <= w.maxy {
                neighbors[neighbor_count] = data[(w.y - d) as usize * width + w.x as usize];
                neighbors[neighbor_count + 1] = data[(w.y + d) as usize * width + w.x as usize];
                neighbor_count += 2;
            }
        }
        if neighbor_count == neighbors_expected {
            let avg = neighbors[..neighbor_count].iter().sum::<f32>() / neighbor_count as f32;
            let mut interpolate = true;
            for &nb in &neighbors[..neighbor_count] {
                if nb == 0.0 {
                    interpolate = false;
                }
                if (nb - avg).abs() > params.interpolation_threshold {
                    interpolate = false;
                }
                if !interpolate {
                    break;
                }
            }
            if interpolate {
                data[idx] = avg;
                debug[idx * 3 + 1] = 255; // green: interpolated
                stats.texels_interpolated += 1;
                return false;
            }
        }
    }

    // Could not interpolate: build the hemicube sample.
    //
    // NOTE the header's barycentric pairing: uv.x weights vertex 2, uv.y
    // weights vertex 1 (see `to_barycentric`).
    let v1 = sub3(w.p[1], w.p[0]);
    let v2_ = sub3(w.p[2], w.p[0]);
    let mut position = add3(w.p[0], add3(scale3(v2_, uv.x), scale3(v1, uv.y)));

    let nv1 = sub3(w.n[1], w.n[0]);
    let nv2 = sub3(w.n[2], w.n[0]);
    let direction = normalize3(add3(w.n[0], add3(scale3(nv2, uv.x), scale3(nv1, uv.y))));
    let direction = normalize3(direction); // the header normalizes twice
    let camera_to_surface_distance =
        (1.0 + params.camera_to_surface_distance_modifier) * params.z_near * 2.0f32.sqrt();
    position = add3(position, scale3(direction, camera_to_surface_distance));

    if !finite3(position) || !finite3(direction) || length3sq(direction) < 0.5 {
        return false;
    }

    let mut up = v3(0.0, 1.0, 0.0);
    if dot3(up, direction).abs() > 0.8 {
        up = v3(0.0, 0.0, 1.0);
    }
    // "randomized" rotation with pattern
    let side = normalize3(cross3(up, direction));
    let up = normalize3(cross3(side, direction));
    let rx = (w.x % 3) as usize;
    let ry = (w.y % 3) as usize;
    let phi = 2.0 * std::f32::consts::PI * BASE_ANGLES[ry][rx] + 0.1 * rng.next01();
    let sample_up = normalize3(add3(scale3(side, phi.cos()), scale3(up, phi.sin())));

    queue.push(Job { x: w.x, y: w.y, pos: position, dir: direction, up: sample_up });
    true
}

/// Render ONE hemicube for a hand-picked sample and integrate it — the
/// oracle the atlas mapping test compares texels against. Returns
/// `(sky_visibility, validity)`: visibility normalized so an open surface
/// reads 1.0 under the given params' weighting kernel (the same
/// `1/validity` texel scale and `1/open_sky` image scale the atlas bake
/// applies), validity in [0, 1] so callers can skip samples the bake's own
/// gate (`<= 0.9`) would have rejected.
#[cfg(test)]
pub(crate) fn sample_hemicube_ao(
    positions: &[Vec3f],
    indices: &[u32],
    pos: Vec3f,
    dir: Vec3f,
    params: &LightmapParams,
) -> (f32, f32) {
    let tris: Vec<[Vec3f; 3]> = (0..indices.len() / 3)
        .map(|t| {
            [
                positions[indices[t * 3] as usize],
                positions[indices[t * 3 + 1] as usize],
                positions[indices[t * 3 + 2] as usize],
            ]
        })
        .collect();
    let cosine = params.cosine_weighted;
    let weights = hemisphere_weights(params.hemisphere_size, move |c| if cosine { c } else { 1.0 });
    let open_sky: f32 = weights.iter().map(|w| w.0).sum();
    let mut r = HemiRenderer::new(
        params.hemisphere_size,
        params.z_near,
        params.z_far,
        params.clear_color,
        &weights,
        &tris,
    );
    // Camera basis exactly as `try_sample` builds it, minus the roll jitter —
    // the hemicube integral is roll-invariant up to raster discretization.
    let dir = normalize3(dir);
    let mut up = v3(0.0, 1.0, 0.0);
    if dot3(up, dir).abs() > 0.8 {
        up = v3(0.0, 0.0, 1.0);
    }
    let side = normalize3(cross3(up, dir));
    let up = normalize3(cross3(side, dir));
    let lift =
        (1.0 + params.camera_to_surface_distance_modifier) * params.z_near * 2.0f32.sqrt();
    let job = Job { x: 0, y: 0, pos: add3(pos, scale3(dir, lift)), dir, up };
    let (c, v) = r.render(&job);
    if v > 0.0 {
        ((c / v) / open_sky, v)
    } else {
        (0.0, 0.0)
    }
}

// ---------------------------------------------------------------------------
// Twin dedup + parity orientation (the port tool's preprocessing, verbatim)
// ---------------------------------------------------------------------------

/// Möller–Trumbore, hit at t > eps — the parity oracle's crossing test.
/// As `ray_hits`, but returns the hit DISTANCE (double-sided, t > eps).
fn ray_dist(o: Vec3f, d: Vec3f, a: Vec3f, b: Vec3f, c: Vec3f) -> Option<f32> {
    let e1 = sub3(b, a);
    let e2 = sub3(c, a);
    let p = cross3(d, e2);
    let det = dot3(e1, p);
    if det.abs() < 1.0e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let tv = sub3(o, a);
    let u = dot3(tv, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross3(tv, e1);
    let v = dot3(d, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot3(e2, q) * inv;
    if t > 1.0e-7 {
        Some(t)
    } else {
        None
    }
}

/// Mean clear distance (capped) over a small hemisphere fan about `n` —
/// "how much open space does this side see". Deterministic fan; reach is a
/// fraction of the model span so slots still differ from open sky.
/// Shared by all three engines' dedup passes: orientation is decided by the
/// bake's own question — which side of this face sees more open space —
/// because crossing parity needs watertight members and the kits do not
/// deliver them.
pub(crate) fn openness(
    centroid: Vec3f,
    n: Vec3f,
    positions: &[Vec3f],
    indices: &[u32],
    span: f32,
) -> f32 {
    let reach = (span * 0.35).max(1.0e-3);
    let eps = span * 1.0e-3;
    // Basis about n.
    let up = if n.y.abs() < 0.9 { v3(0.0, 1.0, 0.0) } else { v3(1.0, 0.0, 0.0) };
    let t = normalize3(cross3(up, n));
    let b = cross3(n, t);
    const FAN: [(f32, f32, f32); 9] = [
        (0.0, 0.0, 1.0),
        (0.66, 0.0, 0.75),
        (-0.66, 0.0, 0.75),
        (0.0, 0.66, 0.75),
        (0.0, -0.66, 0.75),
        (0.47, 0.47, 0.75),
        (-0.47, 0.47, 0.75),
        (0.47, -0.47, 0.75),
        (-0.47, -0.47, 0.75),
    ];
    let origin = v3(
        centroid.x + n.x * eps,
        centroid.y + n.y * eps,
        centroid.z + n.z * eps,
    );
    let mut total = 0.0f32;
    for (fx, fy, fz) in FAN {
        let d = v3(
            t.x * fx + b.x * fy + n.x * fz,
            t.y * fx + b.y * fy + n.y * fz,
            t.z * fx + b.z * fy + n.z * fz,
        );
        let mut nearest = reach;
        for tri in 0..indices.len() / 3 {
            let (a, b2, c) = (
                positions[indices[tri * 3] as usize],
                positions[indices[tri * 3 + 1] as usize],
                positions[indices[tri * 3 + 2] as usize],
            );
            if let Some(hit) = ray_dist(origin, d, a, b2, c) {
                if hit < nearest {
                    nearest = hit;
                }
            }
        }
        total += nearest / reach;
    }
    total / FAN.len() as f32
}

/// Drop coincident duplicate triangles (keep the FIRST of each pair — the
/// kits author outward faces first), then orient the kept set per connected
/// near-coplanar SURFACE GROUP: winding-parity propagation makes each group
/// internally coherent, the authored-normal agreement sum anchors its global
/// sign, and an area-weighted openness vote overrides the anchor only when
/// one side decisively stares into clear air. All-unpaired groups (authored
/// open sheets) keep their authored side untouched.
///
/// The lightmapper engine's OWN preprocessing (the other engines carry
/// theirs): baking the doubled mesh as-is wastes half the atlas on inward
/// twin charts whose every hemicube looks INTO its own member — all rejected
/// by the validity gate, then dilation-smeared.
pub(crate) fn dedup_twins(
    positions: &mut Vec<Vec3f>,
    authored_nrm: &[Vec3f],
    indices: &mut Vec<u32>,
    min: Vec3f,
    max: Vec3f,
) {
    let _ = &positions; // positions are read-only; &mut matches the engine hook shape
    let span = (max.x - min.x)
        .max(max.y - min.y)
        .max(max.z - min.z)
        .max(1.0e-5);
    let inv_eps = 1.0 / (span * 1.0e-5);
    let quant = |p: Vec3f| {
        (
            (p.x * inv_eps).round() as i64,
            (p.y * inv_eps).round() as i64,
            (p.z * inv_eps).round() as i64,
        )
    };
    let tri_count = indices.len() / 3;
    let mut first_of: std::collections::HashMap<[(i64, i64, i64); 3], usize> =
        std::collections::HashMap::with_capacity(tri_count);
    let mut kept: Vec<u32> = Vec::with_capacity(indices.len() / 2);
    let mut kept_paired: Vec<bool> = Vec::with_capacity(tri_count);
    for t in 0..tri_count {
        let mut sorted = [
            quant(positions[indices[t * 3] as usize]),
            quant(positions[indices[t * 3 + 1] as usize]),
            quant(positions[indices[t * 3 + 2] as usize]),
        ];
        sorted.sort();
        match first_of.entry(sorted) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(kept_paired.len());
                kept.extend_from_slice(&indices[t * 3..t * 3 + 3]);
                kept_paired.push(false);
            }
            std::collections::hash_map::Entry::Occupied(e) => {
                // Same cyclic order would be a plain duplicate, reversed the
                // twin — either way the first owner represents the pair, and
                // becomes eligible for the parity flip below.
                kept_paired[*e.get()] = true;
            }
        }
    }

    // Orientation against the deduped set — PAIR SURVIVORS only. A pair
    // survivor's winding is whichever twin was authored first, so it needs
    // the geometric side test; unpaired triangles (true open sheets) keep
    // the authored side, which artists get right.
    let kept_count = kept.len() / 3;
    // SURFACE-GROUP orientation (replaces per-triangle votes entirely).
    //
    // Per-triangle openness broke on the gate arches: hollow open segments
    // give near-tie answers that flip one triangle of a welded face; the
    // smooth-normal hemicubes then aim opposite ways across the edge and
    // the surface bakes with a hard bright/dark seam (edge_continuity's
    // WINDING-OPPOSED hits — 6/7 broken edges per gate, pergola 0).
    // Orientation is a property of a connected surface: (1) group kept
    // triangles by welded near-coplanar adjacency, (2) make each group
    // internally coherent by WINDING PARITY propagated across its welded
    // edges, (3) anchor the group's global sign on the authored-normal
    // agreement sum, (4) let ONE area-weighted openness vote over its
    // largest faces override the anchor when it is decisive.
    // All-unpaired groups (authored open sheets) keep their authored side.
    {
        let quantv = |p: Vec3f| {
            (
                (p.x * inv_eps).round() as i64,
                (p.y * inv_eps).round() as i64,
                (p.z * inv_eps).round() as i64,
            )
        };
        let mut tri_n = vec![v3(0.0, 0.0, 0.0); kept_count];
        let mut tri_area = vec![0.0f32; kept_count];
        for t in 0..kept_count {
            let (a, b, c) = (
                positions[kept[t * 3] as usize],
                positions[kept[t * 3 + 1] as usize],
                positions[kept[t * 3 + 2] as usize],
            );
            let n = cross3(sub3(b, a), sub3(c, a));
            let l = dot3(n, n).sqrt();
            if l > 1.0e-12 {
                tri_n[t] = v3(n.x / l, n.y / l, n.z / l);
                tri_area[t] = l * 0.5;
            }
        }
        let mut edges: std::collections::HashMap<
            ((i64, i64, i64), (i64, i64, i64)),
            Vec<usize>,
        > = std::collections::HashMap::new();
        for t in 0..kept_count {
            let (a, b, c) = (
                quantv(positions[kept[t * 3] as usize]),
                quantv(positions[kept[t * 3 + 1] as usize]),
                quantv(positions[kept[t * 3 + 2] as usize]),
            );
            for (u2, w2) in [(a, b), (b, c), (c, a)] {
                let key = if u2 <= w2 { (u2, w2) } else { (w2, u2) };
                edges.entry(key).or_default().push(t);
            }
        }
        let mut parent: Vec<usize> = (0..kept_count).collect();
        fn find(parent: &mut Vec<usize>, mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        for ts in edges.values() {
            if ts.len() != 2 {
                continue;
            }
            let (t0, t1) = (ts[0], ts[1]);
            if dot3(tri_n[t0], tri_n[t1]).abs() < 0.94 {
                continue;
            }
            let (r0, r1) = (find(&mut parent, t0), find(&mut parent, t1));
            if r0 != r1 {
                parent[r0] = r1;
            }
        }
        let mut groups: std::collections::HashMap<usize, Vec<usize>> =
            std::collections::HashMap::new();
        for t in 0..kept_count {
            let r = find(&mut parent, t);
            groups.entry(r).or_default().push(t);
        }
        // INTERNAL coherence by WINDING PARITY, not by normal sign. The old
        // rule — flip whoever disagrees with the largest face's normal — is
        // only right for near-flat groups. The near-coplanar weld chains
        // TRANSITIVELY, so a gently faceted band wraps as one group: the
        // gate arch tube's inner and outer rims run springer to crown to
        // springer, a 180-degree fan of normals. Sign-vs-lead forcibly
        // flipped the half of each ring facing away from its (arbitrary)
        // lead — mirror-identical halves, one baked soft, one solid black —
        // and the group's openness vote then saw a half-in/half-out set
        // whose front and back answers cancelled to an exact tie
        // (measured 0.435/0.435), so no vote could repair it.
        //
        // Two triangles of one orientable surface traverse a shared welded
        // edge in OPPOSITE directions; a kept twin of the wrong parity
        // traverses it the SAME way as its neighbour. That test is local
        // and curvature-independent, so propagating it (BFS over exactly
        // the edges the union-find merged over) orients a group of ANY
        // wrap as one surface, up to one global sign chosen below.
        let quant_tri = |t: usize| {
            (
                quantv(positions[kept[t * 3] as usize]),
                quantv(positions[kept[t * 3 + 1] as usize]),
                quantv(positions[kept[t * 3 + 2] as usize]),
            )
        };
        let mut flip_rel = vec![false; kept_count];
        {
            let mut visited = vec![false; kept_count];
            let mut queue = std::collections::VecDeque::new();
            for seed in 0..kept_count {
                if visited[seed] {
                    continue;
                }
                visited[seed] = true;
                queue.push_back(seed);
                while let Some(t) = queue.pop_front() {
                    let (a, b, c) = quant_tri(t);
                    for (u2, w2) in [(a, b), (b, c), (c, a)] {
                        let key = if u2 <= w2 { (u2, w2) } else { (w2, u2) };
                        let Some(ts) = edges.get(&key) else { continue };
                        if ts.len() != 2 {
                            continue;
                        }
                        let o = if ts[0] == t { ts[1] } else { ts[0] };
                        if visited[o] || dot3(tri_n[t], tri_n[o]).abs() < 0.94 {
                            continue;
                        }
                        let (oa, ob, oc) = quant_tri(o);
                        let same = [(oa, ob), (ob, oc), (oc, oa)]
                            .into_iter()
                            .any(|e| e == (u2, w2));
                        flip_rel[o] = flip_rel[t] ^ same;
                        visited[o] = true;
                        queue.push_back(o);
                    }
                }
            }
        }
        for members in groups.values() {
            let lead = *members
                .iter()
                .max_by(|x, y| {
                    tri_area[**x]
                        .partial_cmp(&tri_area[**y])
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap();
            // GLOBAL sign anchor: authored-normal agreement — the
            // area-weighted dot of each member's (parity-coherent) winding
            // normal against its authored vertex normals. Measured on the
            // kits (twin_authored_normals probe): every twin pair carries
            // OPPOSITE normals and winding-vs-authored agreement is total,
            // so authored normals carry no winding-independent signal there
            // and this anchor degenerates to the group's area-majority
            // authored winding — but voted across the WHOLE coherent
            // surface at once, which is what makes it symmetry-safe: a
            // mirrored half can no longer resolve apart from its twin,
            // because parity coherence welded them into one decision. On
            // kits whose artists DO repair normals after a baked mirror,
            // the same sum reads the artist's true facing.
            let mut agreement = 0.0f32;
            for &t in members {
                let an = add3(
                    add3(
                        authored_nrm[kept[t * 3] as usize],
                        authored_nrm[kept[t * 3 + 1] as usize],
                    ),
                    authored_nrm[kept[t * 3 + 2] as usize],
                );
                let d = dot3(tri_n[t], an) * tri_area[t] / 3.0;
                agreement += if flip_rel[t] { -d } else { d };
            }
            let invert = agreement < 0.0;
            for &t in members {
                if flip_rel[t] != invert {
                    kept.swap(t * 3 + 1, t * 3 + 2);
                    tri_n[t] = v3(-tri_n[t].x, -tri_n[t].y, -tri_n[t].z);
                }
            }
            let lead_n = tri_n[lead];
            if !members.iter().any(|&t| kept_paired[t]) {
                continue;
            }
            let mut sorted = members.clone();
            sorted.sort_by(|x, y| {
                tri_area[*y]
                    .partial_cmp(&tri_area[*x])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let (mut open_f, mut open_b, mut weight) = (0.0f32, 0.0f32, 0.0f32);
            for &t in sorted.iter().take(4) {
                let (a, b, c) = (
                    positions[kept[t * 3] as usize],
                    positions[kept[t * 3 + 1] as usize],
                    positions[kept[t * 3 + 2] as usize],
                );
                let centroid = v3(
                    (a.x + b.x + c.x) / 3.0,
                    (a.y + b.y + c.y) / 3.0,
                    (a.z + b.z + c.z) / 3.0,
                );
                let n = tri_n[t];
                let w = tri_area[t];
                open_f += openness(centroid, n, positions, &kept, span) * w;
                open_b += openness(centroid, v3(-n.x, -n.y, -n.z), positions, &kept, span) * w;
                weight += w;
            }
            // The vote must be DECISIVE to override the authored side. The
            // authored winding (first twin of each pair, which dedup keeps)
            // is right for everything except wholesale-mirrored parts — and
            // for those the true front stares into CLEAR AIR, so its
            // openness reads near 1.0 (measured: the mirrored floor-detail
            // sheet votes 1.000 against 0.514). Every measured WRONG flip
            // was a near-tie between two half-closed answers (winner 0.20 -
            // 0.51): narrow rim strips facing a flush bar assembly whose
            // back rays escape along the arch tube, cornice tops mostly
            // covered by the block above, rafter tops under a lattice. A
            // marginal "slightly more open" vote flipped those groups
            // interior-ward strip-wide — the hemicubes then integrated the
            // inside of the member and whole visible bands baked black.
            // Requiring the winning side to actually SEE clear space keeps
            // the vote for the mirrored cases it exists for and returns
            // every near-tie to the authored-normal anchor chosen above.
            let open_f_n = if weight > 0.0 { open_f / weight } else { 0.0 };
            let open_b_n = if weight > 0.0 { open_b / weight } else { 0.0 };
            let flip = weight > 0.0
                && open_b > open_f * 1.15 + 1.0e-3
                && open_b_n >= 0.65;
            if std::env::var_os("AO_ORIENT_DEBUG").is_some() {
                let (a, b, c) = (
                    positions[kept[lead * 3] as usize],
                    positions[kept[lead * 3 + 1] as usize],
                    positions[kept[lead * 3 + 2] as usize],
                );
                let lead_an = normalize3(add3(
                    add3(
                        authored_nrm[kept[lead * 3] as usize],
                        authored_nrm[kept[lead * 3 + 1] as usize],
                    ),
                    authored_nrm[kept[lead * 3 + 2] as usize],
                ));
                eprintln!(
                    "group lead t{lead} n ({:.2},{:.2},{:.2}) an ({:.2},{:.2},{:.2}) agree {agreement:.4} members {} area {:.3} open f/b {:.3}/{:.3}{} cen ({:.2},{:.2},{:.2})",
                    lead_n.x, lead_n.y, lead_n.z,
                    lead_an.x, lead_an.y, lead_an.z,
                    members.len(),
                    weight,
                    open_f_n,
                    open_b_n,
                    if flip { " FLIP" } else { "" },
                    (a.x + b.x + c.x) / 3.0,
                    (a.y + b.y + c.y) / 3.0,
                    (a.z + b.z + c.z) / 3.0,
                );
            }
            if flip {
                for &t in members {
                    kept.swap(t * 3 + 1, t * 3 + 2);
                    tri_n[t] = v3(-tri_n[t].x, -tri_n[t].y, -tri_n[t].z);
                }
            }
        }
    }
    *indices = kept;
}

// ---------------------------------------------------------------------------
// Image post-processing (lmImageDilate / lmImageSmooth / lmImagePower)
// ---------------------------------------------------------------------------

/// Port of `lmImageDilate`, single channel: fill zero pixels with the average
/// of their non-zero 4-neighbors.
pub fn image_dilate(image: &[f32], out: &mut [f32], w: usize, h: usize) {
    const DX: [isize; 4] = [-1, 0, 1, 0];
    const DY: [isize; 4] = [0, 1, 0, -1];
    for y in 0..h {
        for x in 0..w {
            let mut color = image[y * w + x];
            if !(color > 0.0) {
                let mut sum = 0.0f32;
                let mut n = 0u32;
                for d in 0..4 {
                    let cx = x as isize + DX[d];
                    let cy = y as isize + DY[d];
                    if cx >= 0 && cx < w as isize && cy >= 0 && cy < h as isize {
                        let dc = image[cy as usize * w + cx as usize];
                        if dc > 0.0 {
                            sum += dc;
                            n += 1;
                        }
                    }
                }
                if n > 0 {
                    color = sum / n as f32;
                }
            }
            out[y * w + x] = color;
        }
    }
}

/// Port of `lmImageSmooth`, single channel: 3x3 box over non-zero pixels.
pub fn image_smooth(image: &[f32], out: &mut [f32], w: usize, h: usize) {
    for y in 0..h {
        for x in 0..w {
            let mut color = 0.0f32;
            let mut n = 0u32;
            for dy in -1isize..=1 {
                let cy = y as isize + dy;
                for dx in -1isize..=1 {
                    let cx = x as isize + dx;
                    if cx >= 0 && cx < w as isize && cy >= 0 && cy < h as isize {
                        let v = image[cy as usize * w + cx as usize];
                        if v > 0.0 {
                            color += v;
                            n += 1;
                        }
                    }
                }
            }
            out[y * w + x] = if n > 0 { color / n as f32 } else { 0.0 };
        }
    }
}

/// Port of `lmImagePower` (gamma), single channel.
pub fn image_power(image: &mut [f32], exponent: f32) {
    for v in image {
        *v = v.powf(exponent);
    }
}

/// Port of `lmImageScale`, single channel.
pub fn image_scale(image: &mut [f32], factor: f32) {
    for v in image {
        *v *= factor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat quad floating alone in a white sky must integrate to ~1.0 on
    /// its face — the weight normalization is exactly what makes that true.
    #[test]
    fn open_quad_integrates_to_one() {
        let positions = vec![
            v3(-1.0, 0.0, -1.0),
            v3(1.0, 0.0, -1.0),
            v3(1.0, 0.0, 1.0),
            v3(-1.0, 0.0, 1.0),
        ];
        let uvs = vec![[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]];
        let indices = vec![0u32, 2, 1, 0, 3, 2]; // CCW seen from +y
        let params = LightmapParams { interpolation_passes: 0, ..Default::default() };
        let bake = bake_ao(&positions, None, &uvs, &indices, 32, 32, &params);
        let center = bake.data[16 * 32 + 16];
        assert!(
            (center - 1.0).abs() < 0.02,
            "open surface should see the whole sky, got {center}"
        );
        assert_eq!(bake.stats.texels_rejected, 0);
    }

    /// Two coincident quads with opposite winding, outward face drawn first —
    /// the double-sided pattern of the Kenney models. A sample from the
    /// outward side must stay valid: the depth tie between the twins must go
    /// to the first-drawn (outward) face.
    #[test]
    fn coplanar_twin_keeps_outward_face() {
        let positions = vec![
            v3(-1.0, 0.0, -1.0),
            v3(1.0, 0.0, -1.0),
            v3(1.0, 0.0, 1.0),
            v3(-1.0, 0.0, 1.0),
        ];
        let uvs = vec![[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]];
        // Outward (+y, CCW from above) first, twin (CW from above) after.
        let indices = vec![0u32, 2, 1, 0, 3, 2, 0, 1, 2, 0, 2, 3];
        let params = LightmapParams { interpolation_passes: 0, ..Default::default() };
        let bake = bake_ao(&positions, None, &uvs, &indices, 32, 32, &params);
        // The +y side texels must be valid and open. (Both windings bake:
        // the twin's texels rasterize over the same uv range but its samples
        // sit UNDER the quad looking down at open sky too, so everything
        // here should be valid; the point is that nothing got rejected by a
        // lost depth tie.)
        assert_eq!(
            bake.stats.texels_rejected, 0,
            "coplanar twins broke the depth tie: {:?}",
            bake.stats
        );
    }

    /// A sample INSIDE a closed double-sided box must be rejected by the
    /// validity gate — the README's whole reason for the alpha marker.
    #[test]
    fn sample_inside_geometry_is_rejected() {
        // A closed unit box made double-sided (outward faces first per quad),
        // plus a small quad trapped inside it whose texels should all reject.
        let mut positions: Vec<Vec3f> = Vec::new();
        let mut uvs: Vec<[f32; 2]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut quad = |a: Vec3f, b: Vec3f, c: Vec3f, d: Vec3f, uv: [[f32; 2]; 4]| {
            let base = positions.len() as u32;
            positions.extend_from_slice(&[a, b, c, d]);
            uvs.extend_from_slice(&uv);
            // outward winding then its twin, Kenney-style
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        };
        // Box faces, outward normals, corner uvs parked in a far texel.
        let park = [[0.02, 0.02]; 4];
        let s = 1.0;
        quad(v3(-s, -s, -s), v3(-s, s, -s), v3(s, s, -s), v3(s, -s, -s), park); // -z
        quad(v3(-s, -s, s), v3(s, -s, s), v3(s, s, s), v3(-s, s, s), park); // +z
        quad(v3(-s, -s, -s), v3(-s, -s, s), v3(-s, s, s), v3(-s, s, -s), park); // -x
        quad(v3(s, -s, -s), v3(s, s, -s), v3(s, s, s), v3(s, -s, s), park); // +x
        quad(v3(-s, -s, -s), v3(s, -s, -s), v3(s, -s, s), v3(-s, -s, s), park); // -y
        quad(v3(-s, s, -s), v3(-s, s, s), v3(s, s, s), v3(s, s, -s), park); // +y
        // Trapped quad in the middle, mapped to its own uv region.
        let base = positions.len() as u32;
        positions.extend_from_slice(&[
            v3(-0.3, 0.0, -0.3),
            v3(0.3, 0.0, -0.3),
            v3(0.3, 0.0, 0.3),
            v3(-0.3, 0.0, 0.3),
        ]);
        uvs.extend_from_slice(&[[0.3, 0.3], [0.7, 0.3], [0.7, 0.7], [0.3, 0.7]]);
        indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);

        let params = LightmapParams { interpolation_passes: 0, ..Default::default() };
        let bake = bake_ao(&positions, None, &uvs, &indices, 32, 32, &params);
        // The trapped quad's texel block is ~12x12; every one of its samples
        // sees only box interior (backfaces) and must have been rejected.
        assert!(
            bake.stats.texels_rejected > 50,
            "interior samples were not rejected: {:?}",
            bake.stats
        );
        let center = bake.data[16 * 32 + 16];
        assert_eq!(center, 0.0, "interior texel should be left 0 for dilation");
    }
}

#[cfg(test)]
mod orientation_forensics {
    use super::*;
    use crate::model::{StaticModel, MODEL_VERTEX_FLOATS};

    fn scan(name: &str) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit");
        let Ok(bytes) = std::fs::read(root.join(name)) else {
            eprintln!("{name}: asset absent — skipped");
            return;
        };
        let mut at = crate::ao_atlas::AoAtlas::new(512);
        at.fill = true;
        let m = StaticModel::parse_glb_baked(&bytes, &mut at).unwrap();
        let stride = MODEL_VERTEX_FLOATS;
        let vp = |i: u32| {
            let o = i as usize * stride;
            v3(m.vertices[o], m.vertices[o + 1], m.vertices[o + 2])
        };
        let uv = |i: u32| crate::model::unpack_ao_uv(m.vertices[i as usize * stride + 6]);
        let tri_count = m.indices.len() / 3;
        // Per-triangle atlas value at centroid uv.
        let mut val = vec![0.0f32; tri_count];
        let mut nrm = vec![v3(0.0, 0.0, 0.0); tri_count];
        let mut cen = vec![v3(0.0, 0.0, 0.0); tri_count];
        for t in 0..tri_count {
            let tri = &m.indices[t * 3..t * 3 + 3];
            let (ua, ub, uc) = (uv(tri[0]), uv(tri[1]), uv(tri[2]));
            let u = (ua[0] + ub[0] + uc[0]) / 3.0;
            let v_ = (ua[1] + ub[1] + uc[1]) / 3.0;
            let x = ((u * at.size as f32) as usize).min(at.size - 1);
            let y = ((v_ * at.size as f32) as usize).min(at.size - 1);
            val[t] = at.pixels[y * at.size + x] as f32 / 255.0;
            let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
            let n = cross3(sub3(b, a), sub3(c, a));
            let l = dot3(n, n).sqrt().max(1.0e-12);
            nrm[t] = v3(n.x / l, n.y / l, n.z / l);
            cen[t] = v3(
                (a.x + b.x + c.x) / 3.0,
                (a.y + b.y + c.y) / 3.0,
                (a.z + b.z + c.z) / 3.0,
            );
        }
        // Outliers: coplanar welded neighbors via position-key edges.
        let span = 8.0f32;
        let q = |p: Vec3f| {
            (
                (p.x * 2048.0 / span).round() as i64,
                (p.y * 2048.0 / span).round() as i64,
                (p.z * 2048.0 / span).round() as i64,
            )
        };
        let mut edges: std::collections::HashMap<((i64,i64,i64),(i64,i64,i64)), Vec<usize>> =
            std::collections::HashMap::new();
        for t in 0..tri_count {
            let tri = &m.indices[t * 3..t * 3 + 3];
            let (a, b, c) = (q(vp(tri[0])), q(vp(tri[1])), q(vp(tri[2])));
            for (u2, w2) in [(a, b), (b, c), (c, a)] {
                let key = if u2 <= w2 { (u2, w2) } else { (w2, u2) };
                edges.entry(key).or_default().push(t);
            }
        }
        let mut outliers = 0;
        for t in 0..tri_count {
            let tri = &m.indices[t * 3..t * 3 + 3];
            let (a, b, c) = (q(vp(tri[0])), q(vp(tri[1])), q(vp(tri[2])));
            let mut worst: Option<f32> = None;
            for (u2, w2) in [(a, b), (b, c), (c, a)] {
                let key = if u2 <= w2 { (u2, w2) } else { (w2, u2) };
                for &o in edges.get(&key).into_iter().flatten() {
                    if o == t {
                        continue;
                    }
                    if dot3(nrm[t], nrm[o]).abs() > 0.985 {
                        let d = (val[t] - val[o]).abs();
                        worst = Some(worst.map_or(d, |w: f32| w.max(d)));
                    }
                }
            }
            if let Some(w) = worst {
                if w > 0.35 {
                    outliers += 1;
                    if outliers <= 12 {
                        eprintln!(
                            "{name} OUTLIER tri {t}: val {:.2} cen ({:.2},{:.2},{:.2}) n ({:.2},{:.2},{:.2})",
                            val[t], cen[t].x, cen[t].y, cen[t].z,
                            nrm[t].x, nrm[t].y, nrm[t].z
                        );
                    }
                }
            }
        }
        eprintln!("{name}: {outliers} outlier triangles of {tri_count}");
    }

    #[test]
    fn gate_and_detail_outliers() {
        scan("gate.glb");
        scan("gate-metal-bars.glb");
        scan("template-floor-detail.glb");
        scan("template-floor-layer-hole.glb");
    }
}

#[cfg(test)]
mod gate_face_forensics {
    use super::*;
    use crate::model::{StaticModel, MODEL_VERTEX_FLOATS};

    /// Every dark triangle of gate.glb: its atlas value vs the openness of
    /// both its sides. flipped = dark but back side far more open;
    /// buried = both sides closed; honest = front closed-ish.
    #[test]
    fn gate_dark_faces() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit");
        let Ok(bytes) = std::fs::read(root.join("gate.glb")) else {
            eprintln!("asset absent — skipped");
            return;
        };
        let mut at = crate::ao_atlas::AoAtlas::new(512);
        at.fill = true;
        let m = StaticModel::parse_glb_baked(&bytes, &mut at).unwrap();
        let stride = MODEL_VERTEX_FLOATS;
        let vp = |i: u32| {
            let o = i as usize * stride;
            v3(m.vertices[o], m.vertices[o + 1], m.vertices[o + 2])
        };
        let uv = |i: u32| crate::model::unpack_ao_uv(m.vertices[i as usize * stride + 6]);
        let count = m.vertices.len() / stride;
        let pos: Vec<Vec3f> = (0..count as u32).map(vp).collect();
        let span = {
            let (mut lo, mut hi) = (v3(f32::MAX,f32::MAX,f32::MAX), v3(f32::MIN,f32::MIN,f32::MIN));
            for p in &pos {
                lo = v3(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
                hi = v3(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
            }
            (hi.x-lo.x).max(hi.y-lo.y).max(hi.z-lo.z)
        };
        let tri_count = m.indices.len() / 3;
        let mut dark = 0;
        for t in 0..tri_count {
            let tri = &m.indices[t * 3..t * 3 + 3];
            let (ua, ub, uc) = (uv(tri[0]), uv(tri[1]), uv(tri[2]));
            let u = (ua[0] + ub[0] + uc[0]) / 3.0;
            let v_ = (ua[1] + ub[1] + uc[1]) / 3.0;
            let x = ((u * at.size as f32) as usize).min(at.size - 1);
            let y = ((v_ * at.size as f32) as usize).min(at.size - 1);
            let val = at.pixels[y * at.size + x] as f32 / 255.0;
            if val > 0.45 {
                continue;
            }
            let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
            let n = cross3(sub3(b, a), sub3(c, a));
            let l = dot3(n, n).sqrt();
            if l < 1.0e-12 { continue; }
            let n = v3(n.x/l, n.y/l, n.z/l);
            let cen = v3((a.x+b.x+c.x)/3.0, (a.y+b.y+c.y)/3.0, (a.z+b.z+c.z)/3.0);
            let of = openness(cen, n, &pos, &m.indices, span);
            let ob = openness(cen, v3(-n.x,-n.y,-n.z), &pos, &m.indices, span);
            dark += 1;
            if dark <= 25 {
                let verdict = if ob > of * 1.5 { "FLIPPED?" } else if of < 0.25 && ob < 0.25 { "buried" } else { "honest-dark" };
                eprintln!(
                    "tri {t}: val {val:.2} open f/b {of:.2}/{ob:.2} {verdict} cen ({:.2},{:.2},{:.2}) n ({:.2},{:.2},{:.2})",
                    cen.x, cen.y, cen.z, n.x, n.y, n.z
                );
            }
        }
        eprintln!("dark faces: {dark} of {tri_count}");
    }

    /// ASSERTING: the pillar-slot strips of gate-metal-bars — the outermost
    /// bars' outward edge faces at x = +-1.0, staring at the pillar's inner
    /// wall across a 0.34 gap under the arch.
    ///
    /// Diagnosed 2026-08-12 (the "near-black strip" hunt): these faces bake
    /// ~0.45 mean, UNIFORM along their height, and that is the HONEST
    /// hemicube answer — validity ~1.0, texels RENDERED (not
    /// rejection-flooded), raw cosine-weighted sky visibility 0.15-0.21
    /// (the 1.26 m-wide, 4 m-tall wall 0.34 m away covers most of the
    /// weighted hemisphere; the arch closes the sky above; escape is only
    /// through the two narrow +-z gaps at grazing incidence), stored
    /// through example.c's display gamma as ~0.42-0.49. Every orientation
    /// stage was checked and chose correctly. The strip merely LOOKS like a
    /// defect in the saturating AO debug view.
    ///
    /// What a REGRESSION of the suspected classes would do instead:
    /// * orientation flip (parity or anchor picks the wrong sign) — the
    ///   hemicubes integrate the inside of the bar, the validity gate
    ///   rejects them, and the strip bakes ~0.0-0.2;
    /// * validity flood — the same texels stop being RENDERED and inherit a
    ///   min-fill;
    /// * a one-sided failure — the two strips are exact mirrors, so any
    ///   asymmetric decision (the class the parity BFS fixed on the arch)
    ///   breaks their equality first.
    /// Hence the three assertions: an honest-enclosure FLOOR, mirror
    /// SYMMETRY, and rendered-not-flooded texels. Pose-free.
    #[test]
    fn metal_bars_pillar_slot_strips() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit");
        let Ok(bytes) = std::fs::read(root.join("gate-metal-bars.glb")) else {
            eprintln!("asset absent — skipped");
            return;
        };
        let mut at = crate::ao_atlas::AoAtlas::new(512);
        at.fill = true;
        let m = StaticModel::parse_glb_baked(&bytes, &mut at).unwrap();
        let stride = MODEL_VERTEX_FLOATS;
        let vp = |i: u32| {
            let o = i as usize * stride;
            v3(m.vertices[o], m.vertices[o + 1], m.vertices[o + 2])
        };
        let uv = |i: u32| crate::model::unpack_ao_uv(m.vertices[i as usize * stride + 6]);
        let sample = |u: f32, v_: f32| -> f32 {
            let x = ((u * at.size as f32) as usize).min(at.size - 1);
            let y = ((v_ * at.size as f32) as usize).min(at.size - 1);
            at.pixels[y * at.size + x] as f32 / 255.0
        };
        // Select the strips GEOMETRICALLY, not by index: every triangle
        // lying wholly in the x = +-1.0 plane, winding outward (toward the
        // pillar), taller than a bar cap. Two triangles per side.
        let tri_count = m.indices.len() / 3;
        let mut sides: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
        for t in 0..tri_count {
            let tri = &m.indices[t * 3..t * 3 + 3];
            let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
            let n = cross3(sub3(b, a), sub3(c, a));
            let l = dot3(n, n).sqrt();
            if l * 0.5 < 0.3 {
                continue;
            }
            for (s, x0) in [(0usize, 1.0f32), (1, -1.0)] {
                if (a.x - x0).abs() < 1.0e-3
                    && (b.x - x0).abs() < 1.0e-3
                    && (c.x - x0).abs() < 1.0e-3
                    && n.x * x0 > 0.0
                {
                    sides[s].push(t);
                }
            }
        }
        assert_eq!(sides[0].len(), 2, "+x strip: expected the two quad halves");
        assert_eq!(sides[1].len(), 2, "-x strip: expected the two quad halves");
        // Interior-grid mean per side, and the rendered/interp fraction of
        // the same sample points (forensics stage classification).
        let mut means = [0.0f32; 2];
        for (s, ts) in sides.iter().enumerate() {
            let (mut sum, mut count) = (0.0f32, 0usize);
            let (mut live, mut total) = (0usize, 0usize);
            for &t in ts {
                let tri = &m.indices[t * 3..t * 3 + 3];
                let (ua, ub, uc) = (uv(tri[0]), uv(tri[1]), uv(tri[2]));
                for i in 1..6 {
                    for j in 1..(7 - i) {
                        let w1 = i as f32 / 7.0;
                        let w2 = j as f32 / 7.0;
                        let w0 = 1.0 - w1 - w2;
                        let u = ua[0] * w0 + ub[0] * w1 + uc[0] * w2;
                        let v_ = ua[1] * w0 + ub[1] * w1 + uc[1] * w2;
                        sum += sample(u, v_);
                        count += 1;
                        crate::ao_atlas::forensics::LAST.with(|lf| {
                            let bo = lf.borrow();
                            let Some(f) = bo.as_ref() else { return };
                            let x = ((u * f.size as f32) as usize).min(f.size - 1);
                            let y = ((v_ * f.size as f32) as usize).min(f.size - 1);
                            let dbg = &f.debug[(y * f.size + x) * 3..(y * f.size + x) * 3 + 3];
                            total += 1;
                            if dbg[0] != 0 || dbg[1] != 0 {
                                live += 1;
                            }
                        });
                    }
                }
            }
            means[s] = sum / count.max(1) as f32;
            eprintln!(
                "strip {}: mean {:.3}, rendered/interp {live} of {total} sample texels",
                if s == 0 { "+x" } else { "-x" },
                means[s],
            );
            if total > 0 {
                assert!(
                    live * 10 >= total * 6,
                    "strip {s}: {live}/{total} texels rendered/interp — validity flood"
                );
            }
        }
        for (s, mean) in means.iter().enumerate() {
            assert!(
                *mean >= 0.30,
                "strip {s}: mean {mean:.3} below the honest-enclosure floor — \
                 orientation or validity regression"
            );
        }
        assert!(
            (means[0] - means[1]).abs() <= 0.05,
            "mirrored strips diverge: +x {:.3} vs -x {:.3}",
            means[0],
            means[1]
        );
    }
}

#[cfg(test)]
mod gate_quad_forensics {
    use super::*;
    use crate::model::{StaticModel, MODEL_VERTEX_FLOATS};

    /// The tapered pillar-side quads: per-triangle mean atlas value + uv
    /// centroid, to catch the diagonal split numerically.
    #[test]
    fn pillar_quads() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit");
        let Ok(bytes) = std::fs::read(root.join("gate.glb")) else {
            eprintln!("asset absent — skipped");
            return;
        };
        let mut at = crate::ao_atlas::AoAtlas::new(512);
        at.fill = true;
        let m = StaticModel::parse_glb_baked(&bytes, &mut at).unwrap();
        let stride = MODEL_VERTEX_FLOATS;
        let vp = |i: u32| {
            let o = i as usize * stride;
            v3(m.vertices[o], m.vertices[o + 1], m.vertices[o + 2])
        };
        let uv = |i: u32| crate::model::unpack_ao_uv(m.vertices[i as usize * stride + 6]);
        let tri_count = m.indices.len() / 3;
        for t in 0..tri_count {
            let tri = &m.indices[t * 3..t * 3 + 3];
            let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
            let n = cross3(sub3(b, a), sub3(c, a));
            let l = dot3(n, n).sqrt();
            if l < 1.0e-9 { continue; }
            let nn = v3(n.x / l, n.y / l, n.z / l);
            let cen = v3((a.x+b.x+c.x)/3.0, (a.y+b.y+c.y)/3.0, (a.z+b.z+c.z)/3.0);
            // the +z pillar-side quads: |nz|>0.9, big, low
            if nn.z.abs() < 0.9 || l * 0.5 < 0.15 || cen.y > 1.2 || cen.x.abs() < 1.2 {
                continue;
            }
            // Sample a 4x4 grid of interior barycentric points.
            let (ua, ub, uc) = (uv(tri[0]), uv(tri[1]), uv(tri[2]));
            let mut sum = 0.0f32;
            let mut count = 0;
            for i in 1..4 {
                for j in 1..(5 - i) {
                    let w1 = i as f32 / 5.0;
                    let w2 = j as f32 / 5.0;
                    let w0 = 1.0 - w1 - w2;
                    let u = ua[0]*w0 + ub[0]*w1 + uc[0]*w2;
                    let v_ = ua[1]*w0 + ub[1]*w1 + uc[1]*w2;
                    let x = ((u * at.size as f32) as usize).min(at.size - 1);
                    let y = ((v_ * at.size as f32) as usize).min(at.size - 1);
                    sum += at.pixels[y * at.size + x] as f32 / 255.0;
                    count += 1;
                }
            }
            eprintln!(
                "tri {t}: mean {:.2} cen ({:.2},{:.2},{:.2}) n ({:.2},{:.2},{:.2}) uv0 ({:.3},{:.3})",
                sum / count as f32,
                cen.x, cen.y, cen.z, nn.x, nn.y, nn.z, ua[0], ua[1]
            );
        }
    }
}

#[cfg(test)]
mod edge_continuity {
    use super::*;
    use crate::model::{StaticModel, MODEL_VERTEX_FLOATS};

    /// THE instrument for "broken triangles", pose-free: for every welded
    /// near-coplanar edge, the atlas values sampled at MIRRORED points — the
    /// edge's uv midpoint pushed the same 1.5 texels into each side — must
    /// agree. Equal distances matter: an honest occlusion gradient arriving
    /// at the edge (a ledge's contact shadow) reads the same value a texel
    /// either side, while any fill or bake defect that steps AT the edge is
    /// caught exactly. (Sampling at the two centroids instead compares
    /// points at UNEQUAL depths into that gradient and flags smooth honest
    /// shading on any asymmetric split.)
    ///
    /// Which pairs count as "one visual surface" is decided by normal AND
    /// winding traversal of the shared edge:
    ///
    /// * `ndot >= 0.94`, opposite traversal — coplanar neighbours of a
    ///   consistently wound surface: must agree.
    /// * `ndot <= -0.99`, SAME traversal — a triangle of the surface wound
    ///   backwards (the flipped-half signature this instrument was built
    ///   to catch; a flipped half lies in its neighbour's own plane, so
    ///   the opposed case demands real coplanarity): must agree, and never
    ///   honestly can, so it reports.
    /// * other opposed pairs — folds between DISTINCT faces: a manifold
    ///   knife edge (opposite traversal — the arch band tapering to an
    ///   edge), or different members' faces welded at a 160-170 degree
    ///   dihedral (same traversal — the arch underside against the
    ///   keystone's seat at ndot -0.97). No eye reads across either;
    ///   skipped.
    pub(super) fn scan(name: &str) -> (usize, f32) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit");
        let Ok(bytes) = std::fs::read(root.join(name)) else {
            eprintln!("{name}: absent");
            return (0, 0.0);
        };
        let mut at = crate::ao_atlas::AoAtlas::new(512);
        at.fill = true;
        let m = StaticModel::parse_glb_baked(&bytes, &mut at).unwrap();
        let stride = MODEL_VERTEX_FLOATS;
        let vp = |i: u32| {
            let o = i as usize * stride;
            v3(m.vertices[o], m.vertices[o + 1], m.vertices[o + 2])
        };
        let uvf = |i: u32| crate::model::unpack_ao_uv(m.vertices[i as usize * stride + 6]);
        let sample = |u: f32, v_: f32| -> f32 {
            let x = ((u * at.size as f32) as usize).min(at.size - 1);
            let y = ((v_ * at.size as f32) as usize).min(at.size - 1);
            at.pixels[y * at.size + x] as f32 / 255.0
        };
        let tri_count = m.indices.len() / 3;
        let mut tri_n = vec![v3(0.0, 0.0, 0.0); tri_count];
        for t in 0..tri_count {
            let (a, b, c) = (vp(m.indices[t*3]), vp(m.indices[t*3+1]), vp(m.indices[t*3+2]));
            let n = cross3(sub3(b, a), sub3(c, a));
            let l = dot3(n, n).sqrt();
            if l > 1.0e-12 {
                tri_n[t] = v3(n.x / l, n.y / l, n.z / l);
            }
        }
        // welded edges: (triangle, edge slot, traversal direction vs key)
        let q = |p: Vec3f| ((p.x * 2048.0).round() as i64, (p.y * 2048.0).round() as i64, (p.z * 2048.0).round() as i64);
        let mut edges: std::collections::HashMap<
            ((i64,i64,i64),(i64,i64,i64)),
            Vec<(usize, usize, bool)>,
        > = std::collections::HashMap::new();
        for t in 0..tri_count {
            let (a, b, c) = (q(vp(m.indices[t*3])), q(vp(m.indices[t*3+1])), q(vp(m.indices[t*3+2])));
            for (e, (u2, w2)) in [(a,b),(b,c),(c,a)].into_iter().enumerate() {
                let fwd = u2 <= w2;
                let key = if fwd { (u2, w2) } else { (w2, u2) };
                edges.entry(key).or_default().push((t, e, fwd));
            }
        }
        // The mirrored near-edge sample: this side's uv at the edge
        // midpoint, pushed `dist` texels toward this side's uv centroid.
        let near_edge = |t: usize, e: usize, dist: f32| -> f32 {
            let (ia, ib, ic) = (m.indices[t*3], m.indices[t*3+1], m.indices[t*3+2]);
            let (es, ee) = (m.indices[t*3+e], m.indices[t*3+(e+1)%3]);
            let sz = at.size as f32;
            let (us, ue) = (uvf(es), uvf(ee));
            let mid = [(us[0]+ue[0])*0.5*sz, (us[1]+ue[1])*0.5*sz];
            let (ua, ub, uc) = (uvf(ia), uvf(ib), uvf(ic));
            let cen = [
                (ua[0]+ub[0]+uc[0])/3.0*sz,
                (ua[1]+ub[1]+uc[1])/3.0*sz,
            ];
            let d = ((cen[0]-mid[0]).powi(2) + (cen[1]-mid[1]).powi(2)).sqrt().max(1.0e-6);
            let k = dist.min(d) / d;
            sample(
                (mid[0] + (cen[0]-mid[0]) * k) / sz,
                (mid[1] + (cen[1]-mid[1]) * k) / sz,
            )
        };
        // Depth into the triangle from the edge midpoint, in texels.
        let depth = |t: usize, e: usize| -> f32 {
            let (ia, ib, ic) = (m.indices[t*3], m.indices[t*3+1], m.indices[t*3+2]);
            let (es, ee) = (m.indices[t*3+e], m.indices[t*3+(e+1)%3]);
            let sz = at.size as f32;
            let (us, ue) = (uvf(es), uvf(ee));
            let mid = [(us[0]+ue[0])*0.5*sz, (us[1]+ue[1])*0.5*sz];
            let (ua, ub, uc) = (uvf(ia), uvf(ib), uvf(ic));
            let cen = [
                (ua[0]+ub[0]+uc[0])/3.0*sz,
                (ua[1]+ub[1]+uc[1])/3.0*sz,
            ];
            ((cen[0]-mid[0]).powi(2) + (cen[1]-mid[1]).powi(2)).sqrt()
        };
        let mut bad = 0usize;
        let mut worst = 0.0f32;
        let mut shown = 0;
        for (_, ts) in edges.iter() {
            if ts.len() != 2 { continue; }
            let (t0, e0, f0) = ts[0];
            let (t1, e1, f1) = ts[1];
            let d = dot3(tri_n[t0], tri_n[t1]);
            let same_traversal = f0 == f1;
            let surface_pair = (d >= 0.94 && !same_traversal)
                || (d <= -0.99 && same_traversal);
            if !surface_pair {
                continue; // a crease/fold or degenerate authoring
            }
            // Mirrored samples: the SAME distance into each side.
            let dist = 1.5f32.min(depth(t0, e0)).min(depth(t1, e1));
            let vals = [near_edge(t0, e0, dist), near_edge(t1, e1, dist)];
            let delta = (vals[0] - vals[1]).abs();
            if delta > worst { worst = delta; }
            if delta > 0.25 {
                bad += 1;
                if shown < 10 {
                    shown += 1;
                    let cen = {
                        let c0 = vp(m.indices[t0*3]);
                        c0
                    };
                    eprintln!(
                        "{name} EDGE t{}/t{} vals {:.2}/{:.2} ndot {:.2}{} at ({:.2},{:.2},{:.2})",
                        t0, t1, vals[0], vals[1], d,
                        if d < 0.0 { " WINDING-OPPOSED" } else { "" },
                        cen.x, cen.y, cen.z
                    );
                    // Which stage put each side's value there?
                    crate::ao_atlas::forensics::LAST.with(|l| {
                        let b = l.borrow();
                        let Some(f) = b.as_ref() else { return };
                        for &t in &[t0, t1] {
                            let (ua, ub, uc) = (
                                uvf(m.indices[t * 3]),
                                uvf(m.indices[t * 3 + 1]),
                                uvf(m.indices[t * 3 + 2]),
                            );
                            let u = (ua[0] + ub[0] + uc[0]) / 3.0;
                            let v_ = (ua[1] + ub[1] + uc[1]) / 3.0;
                            let x = ((u * f.size as f32) as usize).min(f.size - 1);
                            let y = ((v_ * f.size as f32) as usize).min(f.size - 1);
                            let i = y * f.size + x;
                            let dbg = &f.debug[i * 3..i * 3 + 3];
                            let class = match (dbg[0] != 0, dbg[1] != 0, dbg[2] != 0) {
                                (true, _, _) => "RENDERED",
                                (_, true, _) => "interp",
                                (_, _, true) => "REJECTED(min-filled)",
                                _ => "untouched(dilated)",
                            };
                            let ci = f.tri_chart.get(t).copied().unwrap_or(usize::MAX);
                            eprintln!(
                                "    t{t}: chart {ci} texel ({x},{y}) {class} raw {:.3} fill {:.3} cen-n ({:.2},{:.2},{:.2})",
                                f.data_raw.get(i).copied().unwrap_or(-1.0),
                                f.data_fill.get(i).copied().unwrap_or(-1.0),
                                tri_n[t].x, tri_n[t].y, tri_n[t].z,
                            );
                            // Deep probe: a hemicube at this centroid, aimed
                            // along the same smooth field the bake used —
                            // whose validity tells us if the sample is buried.
                            let (i0, i1, i2) = (
                                f.out_idx[t * 3] as usize,
                                f.out_idx[t * 3 + 1] as usize,
                                f.out_idx[t * 3 + 2] as usize,
                            );
                            let cen = scale3(
                                add3(add3(f.out_pos[i0], f.out_pos[i1]), f.out_pos[i2]),
                                1.0 / 3.0,
                            );
                            let sdir = normalize3(add3(
                                add3(f.lm_nrm[i0], f.lm_nrm[i1]),
                                f.lm_nrm[i2],
                            ));
                            let params = LightmapParams::atlas();
                            let (vis_s, val_s) = sample_hemicube_ao(
                                &f.out_pos, &f.out_idx, cen, sdir, &params,
                            );
                            let (vis_f, val_f) = sample_hemicube_ao(
                                &f.out_pos, &f.out_idx, cen, tri_n[t], &params,
                            );
                            // Nearest surface along +/- the flat normal.
                            let hit = |d: Vec3f| -> f32 {
                                let mut best = f32::INFINITY;
                                for tt in 0..f.out_idx.len() / 3 {
                                    let (a, b, c) = (
                                        f.out_pos[f.out_idx[tt * 3] as usize],
                                        f.out_pos[f.out_idx[tt * 3 + 1] as usize],
                                        f.out_pos[f.out_idx[tt * 3 + 2] as usize],
                                    );
                                    if let Some(h) = ray_dist(cen, d, a, b, c) {
                                        if h < best {
                                            best = h;
                                        }
                                    }
                                }
                                best
                            };
                            eprintln!(
                                "      probe cen ({:.3},{:.3},{:.3}) smooth ({:.2},{:.2},{:.2}) vis/val {vis_s:.3}/{val_s:.4} | flat vis/val {vis_f:.3}/{val_f:.4} | fwd hit {:.4} back hit {:.4}",
                                cen.x, cen.y, cen.z, sdir.x, sdir.y, sdir.z,
                                hit(tri_n[t]), hit(neg3(tri_n[t])),
                            );
                        }
                    });
                }
            }
        }
        eprintln!("{name}: {bad} broken edges, worst delta {worst:.2}");
        (bad, worst)
    }

    /// The FACE-level instrument, complement of the edge scan: a connected
    /// same-facing coplanar group that geometrically SEES OUT of the model
    /// must not bake dark as a whole. The edge scan is blind to a coherent
    /// wrong band whose borders are creases (the gate rims' orientation
    /// flips: value ~0 across a whole strip, hard crease boundaries,
    /// neighbours grading gently) — this catches exactly that.
    ///
    /// "Sees out" is the ESCAPE fraction of a hemisphere ray fan — rays
    /// that leave the model without hitting anything — evaluated on BOTH
    /// sides of the group, NOT just along the baked winding. Trusting the
    /// baked side was this instrument's blind spot: a group flipped
    /// interior-ward by the orientation pass reports zero escape on its
    /// (wrong) front and classifies as "interior, honestly black" — the
    /// gate arch rims' black half-bands sailed through. A surface half of
    /// whose sides stares into clear air is open no matter which way the
    /// bake wound it; if EITHER side escapes decisively, the baked mean
    /// must not be dark.
    ///
    /// Clear-distance openness is the wrong measure here — the hollow
    /// pillars' sealed cavity walls have half a metre of air in front of
    /// them (openness 0.32) yet zero sky, and their honest bake IS black;
    /// their backs are buried in the pillar wall, so both sides read
    /// no-escape and no assertion fires.
    /// Violations = (max(escape_front, escape_back) >= 0.15, mean < 0.30).
    pub(super) fn scan_open_groups(name: &str) -> usize {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit");
        let Ok(bytes) = std::fs::read(root.join(name)) else {
            eprintln!("{name}: absent");
            return 0;
        };
        let mut at = crate::ao_atlas::AoAtlas::new(512);
        at.fill = true;
        let m = StaticModel::parse_glb_baked(&bytes, &mut at).unwrap();
        let stride = MODEL_VERTEX_FLOATS;
        let vp = |i: u32| {
            let o = i as usize * stride;
            v3(m.vertices[o], m.vertices[o + 1], m.vertices[o + 2])
        };
        let uvf = |i: u32| crate::model::unpack_ao_uv(m.vertices[i as usize * stride + 6]);
        let tri_count = m.indices.len() / 3;
        let span = (m.max.x - m.min.x)
            .max(m.max.y - m.min.y)
            .max(m.max.z - m.min.z)
            .max(1.0e-5);
        let mut tri_n = vec![v3(0.0, 0.0, 0.0); tri_count];
        let mut tri_area = vec![0.0f32; tri_count];
        for t in 0..tri_count {
            let (a, b, c) = (vp(m.indices[t*3]), vp(m.indices[t*3+1]), vp(m.indices[t*3+2]));
            let n = cross3(sub3(b, a), sub3(c, a));
            let l = dot3(n, n).sqrt();
            if l > 1.0e-12 {
                tri_n[t] = v3(n.x / l, n.y / l, n.z / l);
                tri_area[t] = l * 0.5;
            }
        }
        // Union same-facing welded neighbours (the edge scan's weld, the
        // dedup pass's grouping — but signed: a flipped strip must stay its
        // own group to be judged alone).
        let q = |p: Vec3f| ((p.x * 2048.0).round() as i64, (p.y * 2048.0).round() as i64, (p.z * 2048.0).round() as i64);
        let mut edges: std::collections::HashMap<((i64,i64,i64),(i64,i64,i64)), Vec<usize>> =
            std::collections::HashMap::new();
        for t in 0..tri_count {
            let (a, b, c) = (q(vp(m.indices[t*3])), q(vp(m.indices[t*3+1])), q(vp(m.indices[t*3+2])));
            for (u2, w2) in [(a,b),(b,c),(c,a)] {
                let key = if u2 <= w2 { (u2, w2) } else { (w2, u2) };
                edges.entry(key).or_default().push(t);
            }
        }
        let mut parent: Vec<usize> = (0..tri_count).collect();
        fn find(parent: &mut Vec<usize>, mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        for ts in edges.values() {
            for i in 0..ts.len() {
                for j in i + 1..ts.len() {
                    if dot3(tri_n[ts[i]], tri_n[ts[j]]) < 0.94 {
                        continue;
                    }
                    let (r0, r1) = (find(&mut parent, ts[i]), find(&mut parent, ts[j]));
                    if r0 != r1 {
                        parent[r0] = r1;
                    }
                }
            }
        }
        let mut groups: std::collections::HashMap<usize, Vec<usize>> =
            std::collections::HashMap::new();
        for t in 0..tri_count {
            if tri_area[t] > 1.0e-9 {
                let r = find(&mut parent, t);
                groups.entry(r).or_default().push(t);
            }
        }
        let sample = |u: f32, v_: f32| -> f32 {
            let x = ((u * at.size as f32) as usize).min(at.size - 1);
            let y = ((v_ * at.size as f32) as usize).min(at.size - 1);
            at.pixels[y * at.size + x] as f32 / 255.0
        };
        let debug = std::env::var_os("AO_GROUP_DEBUG").is_some();
        let mut violations = 0usize;
        for members in groups.values() {
            let mut area = 0.0f32;
            let mut mean = 0.0f32;
            for &t in members {
                let (ua, ub, uc) = (uvf(m.indices[t*3]), uvf(m.indices[t*3+1]), uvf(m.indices[t*3+2]));
                mean += sample(
                    (ua[0]+ub[0]+uc[0])/3.0,
                    (ua[1]+ub[1]+uc[1])/3.0,
                ) * tri_area[t];
                area += tri_area[t];
            }
            if area < 1.0e-3 {
                continue; // slivers: their centroid texel is gutter noise
            }
            mean /= area;
            // Escape fraction along the baked winding, over the largest
            // faces: the dedup vote's hemisphere fan, uncapped — a ray
            // either hits geometry or leaves the model.
            let escape_frac = |cen: Vec3f, n: Vec3f| -> f32 {
                let eps = span * 1.0e-3;
                let up = if n.y.abs() < 0.9 { v3(0.0, 1.0, 0.0) } else { v3(1.0, 0.0, 0.0) };
                let tb = normalize3(cross3(up, n));
                let bb = cross3(n, tb);
                const FAN: [(f32, f32, f32); 9] = [
                    (0.0, 0.0, 1.0),
                    (0.66, 0.0, 0.75),
                    (-0.66, 0.0, 0.75),
                    (0.0, 0.66, 0.75),
                    (0.0, -0.66, 0.75),
                    (0.47, 0.47, 0.75),
                    (-0.47, 0.47, 0.75),
                    (0.47, -0.47, 0.75),
                    (-0.47, -0.47, 0.75),
                ];
                let origin = add3(cen, scale3(n, eps));
                let mut escaped = 0usize;
                for (fx, fy, fz) in FAN {
                    let d = v3(
                        tb.x * fx + bb.x * fy + n.x * fz,
                        tb.y * fx + bb.y * fy + n.y * fz,
                        tb.z * fx + bb.z * fy + n.z * fz,
                    );
                    let mut hit = false;
                    for tt in 0..tri_count {
                        let (a, b, c) = (
                            vp(m.indices[tt * 3]),
                            vp(m.indices[tt * 3 + 1]),
                            vp(m.indices[tt * 3 + 2]),
                        );
                        if ray_dist(origin, d, a, b, c).is_some() {
                            hit = true;
                            break;
                        }
                    }
                    if !hit {
                        escaped += 1;
                    }
                }
                escaped as f32 / FAN.len() as f32
            };
            let mut sorted = members.clone();
            sorted.sort_by(|x, y| {
                tri_area[*y].partial_cmp(&tri_area[*x]).unwrap_or(std::cmp::Ordering::Equal)
            });
            let (mut open_f, mut open_b, mut ow) = (0.0f32, 0.0f32, 0.0f32);
            for &t in sorted.iter().take(4) {
                let (a, b, c) = (vp(m.indices[t*3]), vp(m.indices[t*3+1]), vp(m.indices[t*3+2]));
                let cen = v3((a.x+b.x+c.x)/3.0, (a.y+b.y+c.y)/3.0, (a.z+b.z+c.z)/3.0);
                open_f += escape_frac(cen, tri_n[t]) * tri_area[t];
                open_b += escape_frac(cen, neg3(tri_n[t])) * tri_area[t];
                ow += tri_area[t];
            }
            let open_f = if ow > 0.0 { open_f / ow } else { 0.0 };
            let open_b = if ow > 0.0 { open_b / ow } else { 0.0 };
            let open = open_f.max(open_b);
            let flagged = open >= 0.15 && mean < 0.30;
            if flagged {
                violations += 1;
            }
            if flagged || (debug && (open >= 0.10 || mean < 0.45)) {
                let lead = sorted[0];
                let (a, b, c) = (vp(m.indices[lead*3]), vp(m.indices[lead*3+1]), vp(m.indices[lead*3+2]));
                eprintln!(
                    "{name} GROUP{} lead t{lead} n ({:.2},{:.2},{:.2}) members {} area {:.3} open f/b {:.3}/{:.3} mean {:.2} cen ({:.2},{:.2},{:.2})",
                    if flagged { " DARK-OPEN" } else { "" },
                    tri_n[lead].x, tri_n[lead].y, tri_n[lead].z,
                    members.len(), area, open_f, open_b, mean,
                    (a.x+b.x+c.x)/3.0, (a.y+b.y+c.y)/3.0, (a.z+b.z+c.z)/3.0,
                );
                if flagged {
                    crate::ao_atlas::forensics::LAST.with(|l| {
                        let bo = l.borrow();
                        let Some(f) = bo.as_ref() else { return };
                        let (ua, ub, uc) = (
                            uvf(m.indices[lead * 3]),
                            uvf(m.indices[lead * 3 + 1]),
                            uvf(m.indices[lead * 3 + 2]),
                        );
                        let u = (ua[0] + ub[0] + uc[0]) / 3.0;
                        let v_ = (ua[1] + ub[1] + uc[1]) / 3.0;
                        let x = ((u * f.size as f32) as usize).min(f.size - 1);
                        let y = ((v_ * f.size as f32) as usize).min(f.size - 1);
                        let i = y * f.size + x;
                        let dbg = &f.debug[i * 3..i * 3 + 3];
                        eprintln!(
                            "    lead texel ({x},{y}) dbg r{} g{} b{} raw {:.3} fill {:.3} chart {:?}",
                            dbg[0], dbg[1], dbg[2],
                            f.data_raw.get(i).copied().unwrap_or(-1.0),
                            f.data_fill.get(i).copied().unwrap_or(-1.0),
                            f.tri_chart.get(lead),
                        );
                        let (i0, i1, i2) = (
                            f.out_idx[lead * 3] as usize,
                            f.out_idx[lead * 3 + 1] as usize,
                            f.out_idx[lead * 3 + 2] as usize,
                        );
                        let cen = scale3(
                            add3(add3(f.out_pos[i0], f.out_pos[i1]), f.out_pos[i2]),
                            1.0 / 3.0,
                        );
                        let sdir = normalize3(add3(
                            add3(f.lm_nrm[i0], f.lm_nrm[i1]),
                            f.lm_nrm[i2],
                        ));
                        let (vis, val) = sample_hemicube_ao(
                            &f.out_pos, &f.out_idx, cen, sdir, &LightmapParams::atlas(),
                        );
                        eprintln!("    probe vis/validity {vis:.3}/{val:.4}");
                    });
                }
            }
        }
        eprintln!("{name}: {violations} dark-open groups");
        violations
    }

    /// ASSERTING, face level: no visibly open surface group may bake dark
    /// on any of the four campaign models.
    #[test]
    fn gates_open_groups_bake_bright() {
        for name in [
            "gate.glb",
            "gate-metal-bars.glb",
            "template-floor-layer-raised.glb",
            "template-floor-detail.glb",
        ] {
            let bad = scan_open_groups(name);
            assert_eq!(bad, 0, "{name}: {bad} open surface groups baked dark");
        }
    }

    /// FORENSIC (non-asserting): do coincident twins carry OPPOSITE
    /// authored normals (normals follow winding) or the SAME outward
    /// normals (independent artist signal)? Measured on the gates: every
    /// pair is opposite and winding-vs-authored agreement is total — the
    /// fact that makes dedup's authored-normal anchor degenerate to the
    /// group's area-majority authored winding on these kits (see the
    /// anchor comment in `dedup_twins`).
    #[test]
    fn twin_authored_normals() {
        use crate::model::{StaticModel, MODEL_VERTEX_FLOATS};
        for name in ["gate.glb", "gate-metal-bars.glb"] {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../apps/sandbox/resources/models/kenney/modular-dungeon-kit");
            let Ok(bytes) = std::fs::read(root.join(name)) else {
                eprintln!("{name}: absent");
                continue;
            };
            let m = StaticModel::parse_glb(&bytes).unwrap();
            let stride = MODEL_VERTEX_FLOATS;
            let vp = |i: u32| {
                let o = i as usize * stride;
                v3(m.vertices[o], m.vertices[o + 1], m.vertices[o + 2])
            };
            let vn = |i: u32| {
                let (ox, oy) = crate::skin::unpack2f16_pub(m.vertices[i as usize * stride + 3]);
                crate::skin::oct_decode_pub(ox, oy)
            };
            let span = (m.max.x - m.min.x).max(m.max.y - m.min.y).max(m.max.z - m.min.z).max(1e-5);
            let inv_eps = 1.0 / (span * 1.0e-5);
            let q = |p: Vec3f| ((p.x*inv_eps).round() as i64, (p.y*inv_eps).round() as i64, (p.z*inv_eps).round() as i64);
            let tri_count = m.indices.len() / 3;
            let mut first: std::collections::HashMap<[(i64,i64,i64);3], usize> = std::collections::HashMap::new();
            let (mut opposite, mut same, mut unpaired) = (0usize, 0usize, 0usize);
            let mut w_agree = 0usize; // authored normal agrees with own winding
            let mut w_disagree = 0usize;
            let mut ring = (0usize, 0usize); // arch-band tris: agree/disagree
            for t in 0..tri_count {
                let tri = [m.indices[t*3], m.indices[t*3+1], m.indices[t*3+2]];
                let (a, b, c) = (vp(tri[0]), vp(tri[1]), vp(tri[2]));
                let w = cross3(sub3(b, a), sub3(c, a));
                let wl = dot3(w, w).sqrt();
                let an = normalize3(add3(add3(vn(tri[0]), vn(tri[1])), vn(tri[2])));
                if wl > 1e-12 {
                    let wn = v3(w.x/wl, w.y/wl, w.z/wl);
                    let d = dot3(wn, an);
                    if d >= 0.0 { w_agree += 1 } else { w_disagree += 1 }
                    let ceny = (a.y + b.y + c.y) / 3.0;
                    if an.z.abs() < 0.3 && ceny > 1.4 && ceny < 3.6 {
                        if d >= 0.0 { ring.0 += 1 } else { ring.1 += 1 }
                    }
                }
                let mut key = [q(a), q(b), q(c)];
                key.sort();
                match first.entry(key) {
                    std::collections::hash_map::Entry::Vacant(e) => { e.insert(t); unpaired += 1; }
                    std::collections::hash_map::Entry::Occupied(e) => {
                        unpaired -= 1;
                        let o = *e.get();
                        let otri = [m.indices[o*3], m.indices[o*3+1], m.indices[o*3+2]];
                        let on = normalize3(add3(add3(vn(otri[0]), vn(otri[1])), vn(otri[2])));
                        if dot3(an, on) < 0.0 { opposite += 1 } else { same += 1 }
                    }
                }
            }
            eprintln!("{name}: {tri_count} tris; twin pairs: {opposite} opposite-normals, {same} same-normals, {unpaired} unpaired; winding-vs-authored: {w_agree} agree / {w_disagree} disagree; arch-band agree/disagree {}/{}", ring.0, ring.1);
        }
    }

    /// ASSERTING: the models that carried the broken-triangle campaign, and
    /// the two flat templates as the no-regression floor. Every welded
    /// surface pair's atlas values must agree across the edge.
    #[test]
    fn gates_edge_continuity() {
        let mut totals = Vec::new();
        for name in [
            "gate.glb",
            "gate-metal-bars.glb",
            "template-floor-layer-raised.glb",
            "template-floor-detail.glb",
        ] {
            let (bad, worst) = scan(name);
            totals.push((name, bad, worst));
        }
        eprintln!("totals: {totals:?}");
        for (name, bad, _) in totals {
            assert_eq!(bad, 0, "{name}: {bad} broken welded edges (delta > 0.25)");
        }
    }
}
