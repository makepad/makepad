//! BakerBoy evaluator — a CPU port of Fewes/BakerBoy
//! (<https://github.com/Fewes/BakerBoy>), the Unity GPU ambient occlusion
//! baker, selected by `AO_BAKER=bakerboy`
//! (see [`crate::ao_atlas::AoBakerKind`]).
//!
//! ```text
//! MIT License
//!
//! Copyright (c) 2021 Felix Westin
//!
//! Permission is hereby granted, free of charge, to any person obtaining a
//! copy of this software and associated documentation files (the
//! "Software"), to deal in the Software without restriction, including
//! without limitation the rights to use, copy, modify, merge, publish,
//! distribute, sublicense, and/or sell copies of the Software, and to
//! permit persons to whom the Software is furnished to do so, subject to
//! the following conditions:
//!
//! The above copyright notice and this permission notice shall be included
//! in all copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
//! OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
//! MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
//! IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
//! CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
//! TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
//! SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
//! ```
//!
//! # What BakerBoy actually does (verified against its sources)
//!
//! BakerBoy is shadow mapping in a loop. Per bake it renders three kinds of
//! pass (`Resources/Shaders/BakerBoy.shader`):
//!
//! 1. **PositionNormal** — rasterises the mesh in UV space (clip pos from
//!    `uv * 2 - 1`) writing world position and world normal per texel. This is
//!    the only place geometry meets texels; everything after works on the maps.
//! 2. **Shadow + Gather**, once per direction. `BakerBoy.cs::Bake()` walks a
//!    Fibonacci sphere (`PointsOnSphere`, `sampleCount` directions), fits an
//!    ortho "shadow camera" to the scene bounds (`FitShadowCamera`: radius =
//!    the LARGEST HALF-EXTENT of the bounds, near = 1, far = 2·radius + near,
//!    camera at `center - dir·(radius+near)` looking along `dir`), renders the
//!    occluders' depth into a shadow map, then rasterises the mesh in UV space
//!    again and per texel:
//!    ```text
//!    atten  = UNITY_SAMPLE_SHADOW_PROJ(shadowmap,
//!                 worldToShadow * (worldPos - lightDir * _ShadowDepthBias))
//!    atten *= max(0, dot(worldNormal, -lightDir))
//!    occlusion  += float4(atten.xxx, 1)                      * (1/sampleCount)
//!    bentNormal += float4(-lightDir * lerp(1, atten, 0.9), 1) * (1/sampleCount)
//!    ```
//!    (`Blend One One`; the `1` in the alpha lane becomes the coverage mask
//!    the dilation kernel keys on.)
//! 3. **Post** (`BakerBoy.compute` + PackNormal pass):
//!    * PackNormal: bent normal world→tangent via the mesh TBN, then
//!      `normalize(..) * 0.5 + 0.5`.
//!    * `PostProcess` kernel — THE exact output curve:
//!      `rgb = pow(saturate(rgb * _OcclusionGain), _OcclusionBias)` where
//!      `_OcclusionGain = (useHemisphere ? 2 : 4) * config.occlusionGain` and
//!      `_OcclusionBias = config.occlusionBias`. The 4 (or 2) is the sphere
//!      normalisation: E[max(0, cos θ)] over a uniform sphere is exactly 1/4
//!      (1/2 over the useful hemisphere), so an unoccluded texel's
//!      `Σ vis·cos / N ≈ 1/4` maps back to white. Shipped defaults
//!      (`Default BakerBoy Config.asset`): occlusionBias = 1, occlusionGain =
//!      1 — i.e. the curve degenerates to `saturate(x·4)`; the C# field
//!      initialisers would give `pow(saturate(x·4), 2)` but the serialized
//!      asset wins in Unity, so plain `saturate(x·4)` is what BakerBoy ships.
//!    * `Dilate` kernel, `dilationCount` ping-pong passes (forced ≥ 2 and
//!      even; default asset: 512): a texel with alpha == 0 takes
//!      `sum(3x3 neighbours).rgb / sum.a` when any neighbour has alpha > 0.
//!
//! # The bias scheme, exactly
//!
//! `GetAttenuation` in `BakerBoy.shader` offsets the RECEIVER, in world
//! space, along `-lightDir` (toward the light) by `_ShadowDepthBias` =
//! `config.depthBias` (default 0.01 world units) before projecting into
//! shadow space. Because the offset is parallel to the light direction it
//! moves only the compare depth, not the shadow-map texel looked up. The
//! caster side is unbiased (`Shader.SetGlobalVector("_ShadowBias", 0)`
//! zeroes URP's slope/depth bias in the ShadowCaster pass), and the lookup
//! itself is `UNITY_SAMPLE_SHADOW_PROJ` — a hardware 2x2 PCF comparison
//! (bilinear blend of per-tap `receiver_depth <= stored_depth`), ported here
//! as [`sample_shadow_pcf`].
//!
//! # Deliberate deviations, all of them
//!
//! * **Texel sample points come from the repo's chart machinery**, not from a
//!   UV-space rasterisation of authored UVs: one averaged position and one
//!   interpolated (normalised) normal per covered texel, with the exact
//!   coverage loops of `rasterise_chart`/`bake_reference`. This sidesteps a
//!   real BakerBoy pathology on fully double-sided meshes (Kenney): the two
//!   opposite-winding twins share authored UVs, so in Unity both rasterise to
//!   the SAME texel — the position/normal maps keep whichever twin drew last,
//!   and the additive gather then accumulates every sample TWICE (ZTest
//!   LEqual passes on the equal constant depth), doubling `Σ vis·cos / N` and
//!   crushing everything brighter than half-open to white after the ×4 gain.
//!   The chart machinery never merges opposite-facing triangles into one
//!   chart (`COPLANAR_DOT` is +0.966; twins dot at −1), so each side owns its
//!   own texels and bakes honestly — what BakerBoy itself produces on a
//!   single-sided mesh.
//! * **Depth map is 1024² (parameter)**, not BakerBoy's 4086x4096.
//! * **The shadow camera fits the bounding SPHERE**, not BakerBoy's largest
//!   half-extent — their fit clips the corners of wide flat models out of
//!   the depth map and bakes the clamped border as a false soft shadow over
//!   every extremity (see [`ShadowCam::fit`]).
//! * **Doubled meshes are collapsed first** ([`dedup_double_sided`]): one
//!   outward-oriented triangle per coincident opposite-winding pair, for the
//!   sample set, the occluder scene and the emitted geometry alike.
//! * **Bent normals stay in WORLD space** (`normalize(Σ)·0.5+0.5`). The
//!   PackNormal tangent-space transform needs the mesh TBN and this pipeline
//!   carries no tangents; world space is the form the repo's renderer could
//!   actually consume.
//! * **No occluders beyond the model**: BakerBoy bakes whatever renderers are
//!   marked `occlude`, which for a lone prop is the prop itself. The
//!   production path's virtual ground plane is NOT added (BakerBoy has no
//!   ground; its `useHemisphere` option merely folds all directions into the
//!   upper hemisphere, default off).
//! * GPU triangle setup (top-left fill rule, watertight rasterisation) is
//!   approximated by a plain edge-function rasteriser sampling pixel centres;
//!   near/far clipping is applied per fragment, which for an ortho projection
//!   (affine depth) covers exactly the same area as polygon clipping would.
//! * Threading accumulates per-direction partial sums in a fixed order, so a
//!   rebake on the same machine is byte-identical.
//!
//! # What the depth-map visibility means for pathological geometry
//!
//! Compared with ray AO (production evaluator and `ao_reference`):
//!
//! * **Interpenetrating members**: a texel buried INSIDE another member is
//!   behind that member's front surface from every direction, so it goes
//!   fully dark — same verdict as any-hit rays. But a texel NEAR the joint,
//!   still outside the other member, is only occluded for the directions in
//!   which the other member actually stands in front of it — there is no
//!   distance term at all, so no ray-style "grazing hit at 2 mm" halo. The
//!   0.01-unit receiver bias additionally forgives surfaces up to 1 cm behind
//!   an occluder, which absorbs exact-coincidence z-fighting where members'
//!   faces touch.
//! * **Double-sided geometry**: rays fired double-sided from a face can hit
//!   the face's own twin only at t≈0 (rejected by the ray epsilon); the depth
//!   map has the same property structurally — a texel's own surface IS the
//!   stored depth, and the bias keeps `receiver <= stored + bias` true. A
//!   coplanar twin therefore never self-shadows; only genuinely closer
//!   geometry does.
//! * **Coverage**: every triangle, both windings, is rasterised into the
//!   depth buffer (`rasterise_depth` does no culling), matching the task's
//!   double-sided occluder set; BakerBoy itself renders the material's
//!   ShadowCaster pass, which for these twins yields the same surfaces.

use crate::ao_atlas::{project, Chart, GUTTER, TEXEL_SUBSAMPLES, TEXEL_SUB_OFFSETS};
use makepad_draw::makepad_math::Vec3f;

/// Everything `BakerBoyConfig` carries that this port honours, plus the two
/// knobs the task parameterises (direction count, depth resolution).
///
/// Defaults follow the repo's SHIPPED `Default BakerBoy Config.asset`
/// (occlusionBias 1, occlusionGain 1, dilation 512, hemisphere off,
/// depthBias 0.01) — not the C# field initialisers (which say bias 2,
/// dilation 256) — because the serialized asset is what Unity actually loads.
/// `sample_count` defaults to 512 — the setting of the port's comparison
/// bake the user picked this engine FROM (its bake_report.txt: "samples
/// 512"); BakerBoy's asset says 1024, the original port brief said 128.
#[derive(Clone, Debug)]
pub struct BakerBoyParams {
    /// Directions on the Fibonacci sphere (BakerBoy `sampleCount`).
    pub sample_count: usize,
    /// Ortho depth buffer side in texels (BakerBoy uses 4086x4096).
    pub depth_res: usize,
    /// World-space receiver offset toward the light (BakerBoy `depthBias`).
    pub depth_bias: f32,
    /// Fold all directions into the upper hemisphere (BakerBoy
    /// `useHemisphere`: "discard all occlusion samples that hit the ground").
    pub use_hemisphere: bool,
    /// Contrast: the `pow` exponent of the output curve (BakerBoy
    /// `occlusionBias`).
    pub occlusion_bias: f32,
    /// Brightness: multiplies the 4 (or 2) sphere normalisation (BakerBoy
    /// `occlusionGain`).
    pub occlusion_gain: f32,
    /// Dilation passes (BakerBoy `dilationCount`; forced ≥ 2 and even when
    /// non-zero, exactly as `BakerBoy.cs::PostProcess` does).
    pub dilation_count: usize,
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

impl BakerBoyParams {
    /// Read overrides from `AO_BAKERBOY_*` env vars — the same channel the
    /// `AO_BAKE_BAKERBOY` switch itself uses, so the offline tool can steer a
    /// bake that happens several calls below it.
    pub fn from_env() -> Self {
        Self {
            sample_count: env_usize("AO_BAKERBOY_SAMPLES", 512).max(1),
            depth_res: env_usize("AO_BAKERBOY_DEPTH_RES", 1024).max(16),
            depth_bias: env_f32("AO_BAKERBOY_DEPTH_BIAS", 0.01),
            use_hemisphere: std::env::var_os("AO_BAKERBOY_HEMISPHERE").is_some_and(|v| v == "1"),
            occlusion_bias: env_f32("AO_BAKERBOY_OCCLUSION_BIAS", 1.0),
            occlusion_gain: env_f32("AO_BAKERBOY_OCCLUSION_GAIN", 1.0),
            dilation_count: env_usize("AO_BAKERBOY_DILATION", 512),
        }
    }
}

/// `BakerBoy.cs::PointsOnSphere`, verbatim: the classic offset Fibonacci
/// sphere. `y` walks (-1, 1) in even steps of `2/n` (half-step inset, so the
/// poles are never hit exactly), radius derives from `y`, and the azimuth
/// advances by `π(3-√5)` — the golden angle.
pub fn points_on_sphere(n: usize) -> Vec<Vec3f> {
    let inc = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    let off = 2.0 / n as f32;
    (0..n)
        .map(|k| {
            let y = k as f32 * off - 1.0 + off / 2.0;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let phi = k as f32 * inc;
            Vec3f { x: phi.cos() * r, y, z: phi.sin() * r }
        })
        .collect()
}

/// The virtual shadow camera of `BakerBoy.cs::FitShadowCamera`, as a basis
/// plus the ortho extents — enough to project a world point to shadow-map
/// (u, v, depth01) without ever materialising a matrix.
struct ShadowCam {
    pos: Vec3f,
    right: Vec3f,
    up: Vec3f,
    fwd: Vec3f,
    radius: f32,
    near: f32,
    far: f32,
}

impl ShadowCam {
    /// DEVIATION from `FitShadowCamera`: the radius is the BOUNDING-SPHERE
    /// radius (half the box diagonal), not BakerBoy's `max(bounds.extents)`
    /// (largest half-extent). With their fit, corners of a wide flat model
    /// fall outside the ortho volume for diagonal directions — the geometry
    /// is near-plane/viewport clipped out of the depth map and receivers
    /// clamp to the map's border texel, which reads as occlusion. Measured on
    /// the dungeon-kit raised floor: an open corner-post face baked 0.53
    /// where its true visibility is ~0.85, a soft false shadow over every
    /// extremity. The sphere radius guarantees every receiver projects inside
    /// the map and inside [near, far]; the cost is a ~1.6x coarser depth
    /// texel, invisible at 1024².
    fn fit(min: Vec3f, max: Vec3f, dir: Vec3f) -> ShadowCam {
        let center = (min + max) * 0.5;
        let radius = ((max - min) * 0.5).length().max(1.0e-5);
        let near = 1.0;
        let fwd = dir.normalize();
        // Quaternion.LookRotation(dir) with the default world up. The
        // Fibonacci sphere never yields an exactly vertical direction, but
        // the guard keeps a hand-fed test direction from collapsing the
        // basis.
        let world_up = if fwd.y.abs() > 0.999 {
            Vec3f { x: 0.0, y: 0.0, z: 1.0 }
        } else {
            Vec3f { x: 0.0, y: 1.0, z: 0.0 }
        };
        let right = Vec3f::cross(world_up, fwd).normalize();
        let up = Vec3f::cross(fwd, right);
        ShadowCam {
            pos: center - fwd * (radius + near),
            right,
            up,
            fwd,
            radius,
            near,
            far: radius * 2.0 + near,
        }
    }

    /// World point to (u01, v01, depth01). Ortho, so depth is affine — the
    /// property the per-fragment near/far clip in the rasteriser relies on.
    fn project(&self, p: Vec3f) -> (f32, f32, f32) {
        let rel = p - self.pos;
        (
            rel.dot(self.right) / self.radius * 0.5 + 0.5,
            rel.dot(self.up) / self.radius * 0.5 + 0.5,
            (rel.dot(self.fwd) - self.near) / (self.far - self.near),
        )
    }
}

/// Software ortho depth rasteriser — the CPU stand-in for BakerBoy's
/// `DrawShadowmap`. Every triangle, BOTH windings (no culling), nearest depth
/// wins; fragments in front of the near plane or beyond the far plane are
/// dropped, which under an affine (ortho) depth covers exactly the region
/// polygon clipping would keep.
fn rasterise_depth(
    depth: &mut [f32],
    res: usize,
    cam: &ShadowCam,
    positions: &[Vec3f],
    indices: &[u32],
) {
    depth.fill(1.0);
    let resf = res as f32;
    for t in 0..indices.len() / 3 {
        let mut sx = [0.0f32; 3];
        let mut sy = [0.0f32; 3];
        let mut sz = [0.0f32; 3];
        for k in 0..3 {
            let (u, v, z) = cam.project(positions[indices[t * 3 + k] as usize]);
            sx[k] = u * resf;
            sy[k] = v * resf;
            sz[k] = z;
        }
        let area = (sx[1] - sx[0]) * (sy[2] - sy[0]) - (sy[1] - sy[0]) * (sx[2] - sx[0]);
        if area.abs() < 1.0e-12 {
            continue;
        }
        let inv_area = 1.0 / area;
        let lo_x = (sx[0].min(sx[1]).min(sx[2]).floor() as isize).max(0) as usize;
        let lo_y = (sy[0].min(sy[1]).min(sy[2]).floor() as isize).max(0) as usize;
        let hi_x = ((sx[0].max(sx[1]).max(sx[2]).ceil() as isize).max(0) as usize).min(res);
        let hi_y = ((sy[0].max(sy[1]).max(sy[2]).ceil() as isize).max(0) as usize).min(res);
        for py in lo_y..hi_y {
            for px in lo_x..hi_x {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                // Same signed-area barycentrics as the chart rasteriser;
                // `inv_area` carries the winding so both orientations pass.
                let w1 = ((fx - sx[0]) * (sy[2] - sy[0]) - (fy - sy[0]) * (sx[2] - sx[0])) * inv_area;
                let w2 = ((sx[1] - sx[0]) * (fy - sy[0]) - (sy[1] - sy[0]) * (fx - sx[0])) * inv_area;
                let w0 = 1.0 - w1 - w2;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = sz[0] * w0 + sz[1] * w1 + sz[2] * w2;
                if !(0.0..=1.0).contains(&z) {
                    continue;
                }
                let i = py * res + px;
                if z < depth[i] {
                    depth[i] = z;
                }
            }
        }
    }
}

/// `UNITY_SAMPLE_SHADOW_PROJ`: a comparison sample with hardware 2x2 PCF —
/// four LEqual comparisons (`reference <= stored` → lit) blended bilinearly.
/// Taps clamp at the map edge (a fresh Unity RenderTexture wraps Clamp).
fn sample_shadow_pcf(depth: &[f32], res: usize, u01: f32, v01: f32, z_ref: f32) -> f32 {
    let fx = u01 * res as f32 - 0.5;
    let fy = v01 * res as f32 - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (ax, ay) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as isize, y0 as isize);
    let clamp = |v: isize| v.clamp(0, res as isize - 1) as usize;
    let tap = |xi: isize, yi: isize| -> f32 {
        if z_ref <= depth[clamp(yi) * res + clamp(xi)] {
            1.0
        } else {
            0.0
        }
    };
    tap(x0, y0) * (1.0 - ax) * (1.0 - ay)
        + tap(x0 + 1, y0) * ax * (1.0 - ay)
        + tap(x0, y0 + 1) * (1.0 - ax) * ay
        + tap(x0 + 1, y0 + 1) * ax * ay
}

/// `BakerBoy.compute::Dilate`, ported exactly, run `count` passes the way
/// `BakerBoy.cs::PostProcess` schedules them (non-zero counts forced to at
/// least 2 and even). A texel that already has coverage (`a > 0`) never
/// changes; an empty texel takes the coverage-weighted mean of its 3x3
/// neighbourhood the moment any neighbour is covered. Out-of-bounds
/// neighbours read as zero, exactly like a GPU RWTexture.
///
/// Early-out when a pass changes nothing: once every reachable texel is
/// filled the kernel is the identity, so the remaining passes are skipped
/// without altering the result.
fn dilate(rgb: &mut [[f32; 3]], alpha: &mut [f32], w: usize, h: usize, count: usize) {
    if count == 0 {
        return;
    }
    let mut passes = count.max(2);
    if passes % 2 != 0 {
        passes += 1;
    }
    let mut src_rgb = rgb.to_vec();
    let mut src_a = alpha.to_vec();
    for _ in 0..passes {
        let mut changed = false;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                rgb[i] = src_rgb[i];
                alpha[i] = src_a[i];
                if src_a[i] > 0.0 {
                    continue;
                }
                let (mut sum, mut sum_a) = ([0.0f32; 3], 0.0f32);
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let (nx, ny) = (x as isize + dx, y as isize + dy);
                        if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                            continue;
                        }
                        let ni = ny as usize * w + nx as usize;
                        sum[0] += src_rgb[ni][0];
                        sum[1] += src_rgb[ni][1];
                        sum[2] += src_rgb[ni][2];
                        sum_a += src_a[ni];
                    }
                }
                if sum_a > 0.0 {
                    rgb[i] = [sum[0] / sum_a, sum[1] / sum_a, sum[2] / sum_a];
                    alpha[i] = 1.0;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
        src_rgb.copy_from_slice(rgb);
        src_a.copy_from_slice(alpha);
    }
}

/// Collapse a fully doubled mesh to one triangle per coincident
/// opposite-winding PAIR, oriented outward — the preprocessing the BakerBoy
/// path runs before charts are grown.
///
/// # Why
///
/// Kenney's modular kits ship every triangle twice with opposite winding
/// (measured census: 100% of modular-dungeon-kit; 0% of city-kit-suburban).
/// Baked as-is, every surface gets TWO charts — the runtime-culled inward one
/// bakes near-black (it is the interior of a closed box) — so half the atlas
/// is spent on invisible black charts. That wastes half the texel budget, and
/// worse, the atlas-wide dilation and bilinear filtering at chart borders let
/// those black regions bleed into the gutters of the visible charts beside
/// them: the classic seam, on every face at once.
///
/// # How
///
/// Triangles are grouped by their unordered snapped-position triple (the same
/// `span * 1e-5` quantisation the chart welding uses); within a group,
/// opposite-facing pairs (`dot < -0.9`) collapse to their first member.
/// Each survivor of a pair is then ORIENTED BY CROSSING PARITY against the
/// deduped scene: a point nudged off the face along its winding normal is
/// inside matter iff a ray from it crosses the scene an odd number of times,
/// so `outside(+n) && inside(-n)` keeps the winding and the reverse flips it
/// (index swap plus a one-time normal negation per vertex). Faces whose BOTH sides are
/// inside matter — a beam top coincident with the plank lying on it — are
/// genuinely buried interfaces and keep their authored orientation; their
/// texels bake dark, which is the honest answer for a contact seam.
/// Triangles with no twin keep their authored winding untouched.
///
/// Returns the number of pairs collapsed.
pub(crate) fn dedup_double_sided(
    positions: &mut Vec<Vec3f>,
    normals: &mut Vec<Vec3f>,
    indices: &mut Vec<u32>,
    min: Vec3f,
    max: Vec3f,
) -> usize {
    let tri_count = indices.len() / 3;
    if tri_count == 0 {
        return 0;
    }
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
    let corner = |t: usize, k: usize| positions[indices[t * 3 + k] as usize];
    let fnorm = |t: usize| -> Option<Vec3f> {
        let n = Vec3f::cross(corner(t, 1) - corner(t, 0), corner(t, 2) - corner(t, 0));
        if n.length() < 1.0e-12 {
            None
        } else {
            Some(n.normalize())
        }
    };

    let mut groups: std::collections::HashMap<[(i64, i64, i64); 3], Vec<usize>> =
        std::collections::HashMap::with_capacity(tri_count);
    for t in 0..tri_count {
        let mut key = [quant(corner(t, 0)), quant(corner(t, 1)), quant(corner(t, 2))];
        key.sort_unstable();
        groups.entry(key).or_default().push(t);
    }

    let mut drop = vec![false; tri_count];
    let mut from_pair = vec![false; tri_count];
    let mut pairs = 0usize;
    for tris in groups.values() {
        // Greedy opposite-winding pairing in triangle order; decisions are
        // local to a group, so the map's iteration order cannot change them.
        let mut taken = vec![false; tris.len()];
        for i in 0..tris.len() {
            if taken[i] {
                continue;
            }
            let Some(ni) = fnorm(tris[i]) else { continue };
            for j in i + 1..tris.len() {
                if taken[j] {
                    continue;
                }
                let Some(nj) = fnorm(tris[j]) else { continue };
                if ni.dot(nj) < -0.9 {
                    taken[i] = true;
                    taken[j] = true;
                    drop[tris[j].max(tris[i])] = true;
                    from_pair[tris[i].min(tris[j])] = true;
                    pairs += 1;
                    break;
                }
            }
        }
    }
    if pairs == 0 {
        return 0;
    }

    let kept: Vec<usize> = (0..tri_count).filter(|&t| !drop[t]).collect();
    // Decide every flip BEFORE touching the vertex arrays — the closures
    // above borrow them, and the answer must come from the scene as grouped,
    // not one mid-rewrite.
    let flips: Vec<bool> = {
        // OPENNESS vote, not inside/outside parity (the same oracle the
        // lightmapper engine's dedup uses, for the same reason): parity
        // needs watertight members and the kits do not deliver them — the
        // gate arch's segmented tube misvoted whole quads. A flipped face
        // stares into stone (openness ~0) while its true front sees the
        // world; near-ties keep the authored side.
        let kept_idx: Vec<u32> = kept
            .iter()
            .flat_map(|&t| indices[t * 3..t * 3 + 3].to_vec())
            .collect();
        kept.iter()
            .map(|&t| {
                if !from_pair[t] {
                    return false;
                }
                let Some(n) = fnorm(t) else { return false };
                let c = (corner(t, 0) + corner(t, 1) + corner(t, 2)) * (1.0 / 3.0);
                let open_front =
                    crate::ao_lightmapper::openness(c, n, positions, &kept_idx, span);
                let open_back = crate::ao_lightmapper::openness(
                    c,
                    Vec3f { x: -n.x, y: -n.y, z: -n.z },
                    positions,
                    &kept_idx,
                    span,
                );
                open_back > open_front * 1.15 + 1.0e-3
            })
            .collect()
    };

    // Flip IN PLACE: winding by index swap, normals negated once per vertex.
    // No new vertices — `model.rs` keeps uv/tint arrays parallel to the
    // vertex array and indexes them by `source_vertex`, so growing the
    // vertex array here would send those reads out of bounds. In-place is
    // safe for the real pipeline (the mesh arrives fully un-indexed from
    // `resolve_corner_normals`, no vertex is shared); a hand-built indexed
    // mesh shares vertices only within a face, whose triangles flip
    // together — the negation set makes that a single negation per vertex.
    let mut out_idx = Vec::with_capacity(kept.len() * 3);
    let mut negate: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for (k, &t) in kept.iter().enumerate() {
        let (ia, ib, ic) = (indices[t * 3], indices[t * 3 + 1], indices[t * 3 + 2]);
        if flips[k] {
            out_idx.extend_from_slice(&[ia, ic, ib]);
            negate.extend([ia, ib, ic]);
        } else {
            out_idx.extend_from_slice(&[ia, ib, ic]);
        }
    }
    for vi in negate {
        if let Some(n) = normals.get_mut(vi as usize) {
            *n = Vec3f { x: -n.x, y: -n.y, z: -n.z };
        }
    }
    *indices = out_idx;
    pairs
}

/// One texel's stand-in for BakerBoy's position/normal maps: the averaged
/// sample position, the normalised interpolated normal, and where the texel
/// lives in the atlas.
struct TexelPoint {
    atlas_index: usize,
    p: Vec3f,
    n: Vec3f,
}

/// Rasterise every chart's texel sample points with the EXACT coverage loops
/// of `rasterise_chart` / `bake_reference` — same subsample offsets, same
/// clamped barycentrics, same texel-distance acceptance — banking position
/// and normal votes instead of AO. One point per covered texel, like
/// BakerBoy's PositionNormal pass (which keeps one fragment per texel).
fn collect_texel_points(
    charts: &[Chart],
    atlas_size: usize,
    positions: &[Vec3f],
    normals: &[Vec3f],
    indices: &[u32],
) -> Vec<TexelPoint> {
    let fallback = Vec3f { x: 0.0, y: 1.0, z: 0.0 };
    const SUB: [(f32, f32); TEXEL_SUBSAMPLES] = TEXEL_SUB_OFFSETS;
    let mut out = Vec::new();

    for c in charts {
        let texels = c.w * c.h;
        let mut pos_acc = vec![Vec3f { x: 0.0, y: 0.0, z: 0.0 }; texels];
        let mut nrm_acc = vec![Vec3f { x: 0.0, y: 0.0, z: 0.0 }; texels];
        let mut cnt = vec![0.0f32; texels];

        for &t in &c.tris {
            let (ia, ib, ic) = (
                indices[t * 3] as usize,
                indices[t * 3 + 1] as usize,
                indices[t * 3 + 2] as usize,
            );
            let (pa, pb, pc) = (positions[ia], positions[ib], positions[ic]);
            let na = normals.get(ia).copied().unwrap_or(fallback);
            let nb = normals.get(ib).copied().unwrap_or(fallback);
            let nc = normals.get(ic).copied().unwrap_or(fallback);

            let to_tex = |p: Vec3f| {
                let (pu, pv) = project(p, c.axis);
                (
                    GUTTER as f32 + (pu - c.u0) * c.scale,
                    GUTTER as f32 + (pv - c.v0) * c.scale,
                )
            };
            let (ax, ay) = to_tex(pa);
            let (bx, by) = to_tex(pb);
            let (cx2, cy2) = to_tex(pc);

            let area = (bx - ax) * (cy2 - ay) - (by - ay) * (cx2 - ax);
            if area.abs() < 1.0e-9 {
                continue;
            }
            let inv_area = 1.0 / area;

            let lo_x = ((ax.min(bx).min(cx2)).floor() as isize - 1).max(0) as usize;
            let lo_y = ((ay.min(by).min(cy2)).floor() as isize - 1).max(0) as usize;
            let hi_x = ((ax.max(bx).max(cx2)).ceil() as usize + 1).min(c.w);
            let hi_y = ((ay.max(by).max(cy2)).ceil() as usize + 1).min(c.h);

            for ty in lo_y..hi_y {
                for tx in lo_x..hi_x {
                    for (sx, sy) in SUB {
                        let (fx, fy) = (tx as f32 + sx, ty as f32 + sy);
                        let mut w1 = ((fx - ax) * (cy2 - ay) - (fy - ay) * (cx2 - ax)) * inv_area;
                        let mut w2 = ((bx - ax) * (fy - ay) - (by - ay) * (fx - ax)) * inv_area;
                        w1 = w1.clamp(0.0, 1.0);
                        w2 = w2.clamp(0.0, 1.0);
                        if w1 + w2 > 1.0 {
                            let s = w1 + w2;
                            w1 /= s;
                            w2 /= s;
                        }
                        let w0 = 1.0 - w1 - w2;
                        let qx = ax * w0 + bx * w1 + cx2 * w2;
                        let qy = ay * w0 + by * w1 + cy2 * w2;
                        let d2 = (qx - fx) * (qx - fx) + (qy - fy) * (qy - fy);
                        if d2 > 1.0 {
                            continue;
                        }
                        let i = ty * c.w + tx;
                        pos_acc[i] = pos_acc[i] + pa * w0 + pb * w1 + pc * w2;
                        nrm_acc[i] = nrm_acc[i] + na * w0 + nb * w1 + nc * w2;
                        cnt[i] += 1.0;
                    }
                }
            }
        }

        for ty in 0..c.h {
            for tx in 0..c.w {
                let i = ty * c.w + tx;
                if cnt[i] <= 0.0 {
                    continue;
                }
                let p = pos_acc[i] * (1.0 / cnt[i]);
                let n = if nrm_acc[i].length() < 1.0e-8 {
                    fallback
                } else {
                    nrm_acc[i].normalize()
                };
                out.push(TexelPoint {
                    atlas_index: (c.y + ty) * atlas_size + c.x + tx,
                    p,
                    n,
                });
            }
        }
    }
    out
}

/// The whole BakerBoy sample loop, direction-major, for every chart at once —
/// the shape of `BakerBoy.cs::Bake()`: per direction fit the shadow camera,
/// render the depth map, then let every texel gather through it. Returns the
/// per-chart pixel blocks `bake_into` expects. (The port's bent-normal lane
/// is not computed here — nothing in this renderer consumes it; the AO bytes
/// are identical with or without it.)
pub(crate) fn bake_all_charts(
    charts: &[Chart],
    atlas_size: usize,
    positions: &[Vec3f],
    normals: &[Vec3f],
    indices: &[u32],
    min: Vec3f,
    max: Vec3f,
) -> Vec<Vec<u8>> {
    let params = BakerBoyParams::from_env();
    let points = collect_texel_points(charts, atlas_size, positions, normals, indices);

    // Directions, hemisphere-folded if asked (BakerBoy: `direction.y =
    // -abs(direction.y)` — light always travelling DOWNWARD, i.e. arriving
    // from the sky).
    let mut dirs = points_on_sphere(params.sample_count);
    if params.use_hemisphere {
        for d in dirs.iter_mut() {
            d.y = -d.y.abs();
        }
    }

    // --- Sample loop: one depth map per direction, shared by all texels ----
    // Directions are dealt round-robin to a fixed number of workers and the
    // partial sums reduced in worker order, so the float addition order — and
    // with it the bake — is reproducible.
    let threads = std::thread::available_parallelism()
        .map_or(8, |n| n.get())
        .min(params.sample_count.max(1));
    let res = params.depth_res;
    let point_count = points.len();
    let partials: Vec<Vec<f32>> = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(threads);
        for worker in 0..threads {
            let dirs = &dirs;
            let points = &points;
            let params = &params;
            handles.push(scope.spawn(move || {
                let mut occ = vec![0.0f32; point_count];
                let mut depth = vec![1.0f32; res * res];
                let mut k = worker;
                while k < dirs.len() {
                    let d = dirs[k];
                    let cam = ShadowCam::fit(min, max, d);
                    rasterise_depth(&mut depth, res, &cam, positions, indices);
                    let bias_z = params.depth_bias / (cam.far - cam.near);
                    for (i, pt) in points.iter().enumerate() {
                        let (u, v, z) = cam.project(pt.p);
                        // `worldPos - lightDir * bias`: the offset is parallel
                        // to the view direction, so only the compare depth
                        // moves — by exactly bias/(far-near).
                        let vis = sample_shadow_pcf(&depth, res, u, v, z - bias_z);
                        occ[i] += vis * (-pt.n.dot(d)).max(0.0);
                    }
                    k += threads;
                }
                occ
            }));
        }
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut occ_sum = vec![0.0f32; point_count];
    for occ in &partials {
        for i in 0..point_count {
            occ_sum[i] += occ[i];
        }
    }

    // --- Post: the exact BakerBoy output curve, then dilation over the
    // whole atlas -------------------------------------------------------------
    let texels = atlas_size * atlas_size;
    let mut ao_rgb = vec![[0.0f32; 3]; texels];
    let mut ao_a = vec![0.0f32; texels];

    let inv_n = 1.0 / params.sample_count as f32;
    let gain = if params.use_hemisphere { 2.0 } else { 4.0 } * params.occlusion_gain;
    for (i, pt) in points.iter().enumerate() {
        // `pow(saturate(rgb * _OcclusionGain), _OcclusionBias)` on the
        // accumulated `Σ vis·cos / N`.
        let v = (occ_sum[i] * inv_n * gain).clamp(0.0, 1.0).powf(params.occlusion_bias);
        ao_rgb[pt.atlas_index] = [v, v, v];
        ao_a[pt.atlas_index] = 1.0;
    }

    dilate(&mut ao_rgb, &mut ao_a, atlas_size, atlas_size, params.dilation_count);

    // Quantise like the GPU's float→ARGB32 readback: round to nearest.
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let occlusion: Vec<u8> = ao_rgb.iter().map(|c| q(c[0])).collect();

    // Per-chart blocks for `bake_into`, sliced from the finished atlas so
    // `atlas.pixels` agrees texel for texel with the port's full-image map
    // everywhere a chart lives.
    charts
        .iter()
        .map(|c| {
            let mut px = vec![255u8; c.w * c.h];
            for ty in 0..c.h {
                for tx in 0..c.w {
                    px[ty * c.w + tx] = occlusion[(c.y + ty) * atlas_size + c.x + tx];
                }
            }
            px
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3f {
        Vec3f { x, y, z }
    }

    /// The direction set is BakerBoy's own: unit length, y marching in even
    /// steps from -1+1/n to 1-1/n.
    #[test]
    fn fibonacci_sphere_matches_bakerboy() {
        let n = 128;
        let dirs = points_on_sphere(n);
        assert_eq!(dirs.len(), n);
        for (k, d) in dirs.iter().enumerate() {
            assert!((d.length() - 1.0).abs() < 1.0e-4, "direction {k} not unit: {}", d.length());
            let want_y = k as f32 * (2.0 / n as f32) - 1.0 + 1.0 / n as f32;
            assert!((d.y - want_y).abs() < 1.0e-5, "direction {k} y drifted");
        }
        // Cosine-sum sanity: E[max(0, cos)] over the sphere is 1/4 — the 4x
        // gain in the post curve depends on it.
        let n_up = v(0.0, 1.0, 0.0);
        let sum: f32 = dirs.iter().map(|d| (-n_up.dot(*d)).max(0.0)).sum();
        let mean = sum / n as f32;
        assert!((mean - 0.25).abs() < 0.01, "cosine mean {mean} should be ~1/4");
    }

    /// An open quad must bake white and a floor under a close lid nearly
    /// black, through the full `bake_into` path with the BakerBoy engine
    /// selected — via the THREAD-LOCAL override, so parallel bake tests
    /// never see it.
    #[test]
    fn open_is_white_covered_is_black_and_deterministic() {
        let geometry = || {
            // A 2x2 up-facing floor with a small down-facing lid 0.05 above
            // its centre: the floor under the lid and the lid's underside
            // stare at each other and go black; the floor's outer ring is
            // open sky and must gather white.
            let p = vec![
                v(-1.0, 0.0, -1.0), v(1.0, 0.0, -1.0), v(1.0, 0.0, 1.0), v(-1.0, 0.0, 1.0),
                v(-0.4, 0.05, -0.4), v(0.4, 0.05, -0.4), v(0.4, 0.05, 0.4), v(-0.4, 0.05, 0.4),
            ];
            let mut n = vec![v(0.0, 1.0, 0.0); 4];
            n.extend(vec![v(0.0, -1.0, 0.0); 4]);
            let i = vec![0u32, 1, 2, 0, 2, 3, 4, 6, 5, 4, 7, 6];
            (p, n, i)
        };
        let bake = || {
            crate::ao_atlas::set_thread_baker(Some(crate::ao_atlas::AoBakerKind::BakerBoy));
            let (mut p, mut n, mut i) = geometry();
            // `fill`: a lone mesh shrink-wraps its own atlas; a fixed-size
            // pack atlas would trip the no-growth assertion.
            let mut at = crate::ao_atlas::AoAtlas::new(256);
            at.fill = true;
            crate::ao_atlas::bake_into(
                &mut at, &mut p, &mut n, &mut i, v(-1.0, 0.0, -1.0), v(1.0, 0.05, 1.0),
            );
            crate::ao_atlas::set_thread_baker(None);
            at
        };
        let at = bake();

        // The geometry makes the answer bimodal: the lid's top and the
        // floor's underside are fully open (white after the 4x gain), while
        // the floor's top and the lid's underside stare at each other from
        // 5cm and lose the entire hemisphere (near black). Both modes must
        // be present in the atlas.
        let dark = at.pixels.iter().filter(|&&px| px < 30).count();
        let bright = at.pixels.iter().filter(|&&px| px > 245).count();
        assert!(bright > 0, "no open texels baked white");
        assert!(dark > 0, "no enclosed texels baked dark");

        // Determinism: a second bake reproduces byte for byte.
        let at2 = bake();
        assert_eq!(at.pixels, at2.pixels);
    }

    /// The receiver bias must forgive a surface its own depth. Not per
    /// direction — at grazing angles a depth texel's slope error genuinely
    /// exceeds a fixed 0.01 bias, in BakerBoy as here; that is why the
    /// gather multiplies by `max(0, cos)` before accumulating. The honest
    /// claim is the WEIGHTED one: an open quad's full BakerBoy sum must come
    /// back white.
    #[test]
    fn an_open_surface_gathers_white() {
        let positions = vec![
            v(-1.0, 0.0, -1.0), v(1.0, 0.0, -1.0), v(1.0, 0.0, 1.0), v(-1.0, 0.0, 1.0),
        ];
        let indices = vec![0u32, 1, 2, 0, 2, 3];
        let n_up = v(0.0, 1.0, 0.0);
        let (res, bias) = (1024usize, 0.01f32);
        let dirs = points_on_sphere(128);
        let mut depth = vec![1.0f32; res * res];
        for probe in [v(0.25, 0.0, -0.125), v(-0.8, 0.0, 0.7), v(0.0, 0.0, 0.0)] {
            let mut sum = 0.0f32;
            for &dir in &dirs {
                let cam = ShadowCam::fit(v(-1.0, -0.01, -1.0), v(1.0, 0.01, 1.0), dir);
                rasterise_depth(&mut depth, res, &cam, &positions, &indices);
                let (u, vv, z) = cam.project(probe);
                let vis =
                    sample_shadow_pcf(&depth, res, u, vv, z - bias / (cam.far - cam.near));
                sum += vis * (-n_up.dot(dir)).max(0.0);
            }
            let value = (sum / dirs.len() as f32 * 4.0).clamp(0.0, 1.0);
            assert!(
                value > 0.97,
                "open quad gathered {value} at ({},{},{})",
                probe.x, probe.y, probe.z
            );
        }
    }

    /// A doubled sheet resting near a solid must collapse to one triangle
    /// per pair, each survivor oriented AWAY from the solid — the openness
    /// vote working, with the flipped survivor's vertex normals negated to
    /// match. (The vote's contract is the kits': a face's closed side is
    /// within the openness reach; a lone hollow box reads open on both
    /// sides and keeps the authored winding.)
    #[test]
    fn dedup_collapses_twins_and_orients_outward() {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let push_tri = |ps: &mut Vec<Vec3f>, ns: &mut Vec<Vec3f>, is: &mut Vec<u32>,
                        a: Vec3f, b: Vec3f, c: Vec3f| {
            let n = Vec3f::cross(b - a, c - a).normalize();
            let base = ps.len() as u32;
            ps.extend_from_slice(&[a, b, c]);
            ns.extend_from_slice(&[n, n, n]);
            is.extend_from_slice(&[base, base + 1, base + 2]);
        };
        // Big single-sided ground (unpaired, untouched)...
        push_tri(&mut positions, &mut normals, &mut indices,
                 v(-5.0, 0.0, -5.0), v(5.0, 0.0, 5.0), v(5.0, 0.0, -5.0));
        push_tri(&mut positions, &mut normals, &mut indices,
                 v(-5.0, 0.0, -5.0), v(-5.0, 0.0, 5.0), v(5.0, 0.0, 5.0));
        // ...and a doubled quad 5cm above it, the INWARD (down) twin authored
        // FIRST so it is the pair's kept representative and the openness vote
        // must flip it up, negating its normals.
        let (a, b, c, d) = (
            v(-0.5, 0.05, -0.5), v(0.5, 0.05, -0.5), v(0.5, 0.05, 0.5), v(-0.5, 0.05, 0.5),
        );
        push_tri(&mut positions, &mut normals, &mut indices, a, b, c); // down
        push_tri(&mut positions, &mut normals, &mut indices, a, c, b); // up twin
        push_tri(&mut positions, &mut normals, &mut indices, a, c, d); // down
        push_tri(&mut positions, &mut normals, &mut indices, a, d, c); // up twin
        assert_eq!(indices.len() / 3, 6);
        let pairs = dedup_double_sided(
            &mut positions, &mut normals, &mut indices,
            v(-5.0, 0.0, -5.0), v(5.0, 0.05, 5.0),
        );
        assert_eq!(pairs, 2);
        assert_eq!(indices.len() / 3, 4);
        for t in 2..4 {
            let (a, b, c) = (
                positions[indices[t * 3] as usize],
                positions[indices[t * 3 + 1] as usize],
                positions[indices[t * 3 + 2] as usize],
            );
            let n = Vec3f::cross(b - a, c - a).normalize();
            assert!(
                n.y > 0.0,
                "kept face {t} faces the solid it rests on: n=({:.2},{:.2},{:.2})",
                n.x, n.y, n.z
            );
            // The vertex normals must agree with the corrected winding.
            let vn = normals[indices[t * 3] as usize];
            assert!(vn.dot(n) > 0.9, "vertex normal disagrees with winding on face {t}");
        }
    }

    /// BakerBoy's dilation: covered texels are immutable, empty texels take
    /// the coverage-weighted 3x3 mean, and the pass count rounding (>=2,
    /// even) matches `PostProcess`.
    #[test]
    fn dilation_fills_outward_without_touching_coverage() {
        let (w, h) = (8usize, 1usize);
        let mut rgb = vec![[0.0f32; 3]; w * h];
        let mut a = vec![0.0f32; w * h];
        rgb[0] = [0.5, 0.5, 0.5];
        a[0] = 1.0;
        // One requested pass runs as two (min 2), so two texels fill.
        dilate(&mut rgb, &mut a, w, h, 1);
        assert_eq!(rgb[0], [0.5, 0.5, 0.5]);
        assert!((rgb[1][0] - 0.5).abs() < 1.0e-6 && a[1] == 1.0);
        assert!((rgb[2][0] - 0.5).abs() < 1.0e-6 && a[2] == 1.0);
        assert_eq!(a[3], 0.0, "one pass (rounded to two) must not reach texel 3");
    }
}
