//! Ported from prideout/aobaker (<https://github.com/prideout/aobaker>),
//! public domain — the ambient-occlusion evaluator, selected by
//! `AO_BAKER=aobaker` (see [`crate::ao_atlas::AoBakerKind`]).
//!
//! # What aobaker does (aobaker.cpp, raytrace.cpp, thekla/thekla_atlas.cpp)
//!
//! 1. Thekla parameterises the mesh into charts, and `atlas_dump` rasterises
//!    every output triangle into the atlas, storing one interpolated
//!    object-space POSITION (`coordsdata`) and one facet NORMAL (`normsdata`)
//!    per covered texel.
//! 2. `raytrace.cpp` builds an embree scene over the SAME mesh and, for every
//!    texel whose stored normal is non-zero, shoots `nsamples` rays: a random
//!    point on the unit SPHERE (`random_direction`), flipped into the
//!    hemisphere when its dot with the normal is negative; `tnear` a fixed
//!    epsilon, `tfar = FLT_MAX`; `rtcOccluded`, so ANY hit at ANY distance
//!    occludes — double-sided, no falloff, no cosine weighting. Then
//!    `ao = multiply * (1 - hits / nsamples)`, stored as `min(255, 255*ao)`
//!    in an 8-bit image.
//! 3. The image is dilated by 2 texels — an uncovered texel copies its first
//!    covered neighbour — for bilinear safety at chart seams.
//!
//! # What this port maps onto
//!
//! Chart growth, packing, texel COVERAGE, uv emission and gutter dilation are
//! the repo's own ([`crate::ao_atlas::bake_into`]), so the result stays
//! texel-comparable with the production and reference bakes; this module
//! replaces only the per-texel occlusion answer. Taken verbatim from aobaker:
//!
//! * ONE sample per covered texel — a position and an interpolated authored
//!   normal (aobaker itself stores the facet normal; on the flat-shaded kit
//!   pieces this bakes, authored per-corner normals ARE the facet normals);
//! * uniform-SPHERE directions folded into the hemisphere by the normal —
//!   NOT cosine-weighted, the fold is the only hemisphere shaping;
//! * ray origin advanced `1e-3 x model span` ALONG the ray (aobaker's
//!   `tnear = 0.001`, on meshes it has just written out at roughly unit
//!   scale);
//! * `tfar` unbounded — the occluder set's diameter caps the march below
//!   only because a grid walk needs a finite length, nothing can be hit
//!   beyond it;
//! * ANY hit occludes; `texel = min(255, 255 * multiply * (1 - hits/N))`,
//!   truncated to u8 exactly as the C float-to-uchar store does.
//!
//! Deliberate deviations, each forced by determinism or the shared machinery:
//!
//! * aobaker draws directions from per-thread-seeded `rand()`, so its bakes
//!   are not even self-reproducible; this port uses a fixed Fibonacci
//!   lattice over the sphere — deterministic, same uniform-area
//!   distribution (and no 3.14-for-pi truncation).
//! * dilation is the machinery's chart-local covered-neighbour AVERAGE
//!   rather than aobaker's whole-image first-non-zero copy; both exist for
//!   the same bilinear-support reason and fill the same texels.
//! * the machinery's virtual ground quad is EXCLUDED from the occluder set:
//!   aobaker's scene is the mesh alone, and the quad is this repo's
//!   addition.
//!
//! On top of the faithful port sits ONE switch aobaker itself does not have
//! but embree deployments commonly enable: `cull_backfaces` — the standard
//! "ignore backfaces" intersection filter (what
//! `rtcSetGeometryEnableFilter`-style backface culling gives embree users).
//! A hit whose triangle winds AWAY from the ray does not occlude.
//!
//! Knobs: `AO_AOBAKER_MULTIPLY` (aobaker's `--multiply`, default 1.0) and
//! `AO_AOBAKER_CULL=1` for the backface-culled variant. The twin dedup runs
//! unconditionally — the comparison bake the user picked was the deduped
//! variant, and single-sided packs make it a no-op.

use crate::ao::AoSampler;
use crate::ao_atlas::{project, Chart, GUTTER, TEXEL_SUBSAMPLES, TEXEL_SUB_OFFSETS};
use makepad_draw::makepad_math::Vec3f;

/// Rays per texel. aobaker's CLI default is 128; the port contract pins 256,
/// which quantises occlusion to 1/256 — below one 8-bit step.
pub(crate) const AOBAKER_RAYS: usize = 256;

/// Golden angle for the Fibonacci sphere; same constant as the production
/// hemisphere and the reference evaluator.
const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// The aobaker evaluator's knobs, one bake at a time. (The twin dedup —
/// [`dedup_double_sided`] — is not a knob: for fully double-sided kits the
/// modular dungeon kit measures 100%, and without it HALF the charts are
/// interior-facing twins that bake to ~0 and drown the atlas dark; aobaker
/// was designed for single-sided meshes.)
#[derive(Clone, Copy)]
pub(crate) struct AobakerParams {
    /// aobaker's `--multiply`: scales the stored AO value. `< 1` darkens.
    pub multiply: f32,
    /// The "ignore backfaces" variant: only front-facing hits occlude.
    pub cull_backfaces: bool,
}

impl AobakerParams {
    pub(crate) fn from_env() -> Self {
        Self {
            multiply: std::env::var("AO_AOBAKER_MULTIPLY")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(1.0),
            cull_backfaces: std::env::var_os("AO_AOBAKER_CULL").is_some_and(|v| v == "1"),
        }
    }
}

// Small vector helpers for the dedup pass; the evaluator below stays
// componentwise like its neighbours.
fn vsub(a: Vec3f, b: Vec3f) -> Vec3f {
    Vec3f { x: a.x - b.x, y: a.y - b.y, z: a.z - b.z }
}
fn vdot(a: Vec3f, b: Vec3f) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}
fn vcross(a: Vec3f, b: Vec3f) -> Vec3f {
    Vec3f {
        x: a.y * b.z - a.z * b.y,
        y: a.z * b.x - a.x * b.z,
        z: a.x * b.y - a.y * b.x,
    }
}
fn vnorm(a: Vec3f) -> Vec3f {
    let l = vdot(a, a).sqrt().max(1.0e-12);
    Vec3f { x: a.x / l, y: a.y / l, z: a.z / l }
}

/// Collapse a fully or partially double-sided mesh to single-sided: detect
/// coincident opposite-winding twins and keep ONE per pair — for the sample
/// set (charts, texels, uvs) AND the occluder scene alike, since both are
/// built from these indices downstream.
///
/// Which twin survives is decided by crossing PARITY against the deduped
/// set (sound once twins are gone: an even count from just off a side means
/// that side faces open space): probe both sides of the shared plane, keep
/// the twin whose winding faces the outside. The twins carry honest
/// per-side authored normals, so keeping the outward twin orients the
/// evaluator's hemisphere without mutating any vertex data. Ties — both
/// sides inside (a member end embedded in a neighbouring member) or both
/// outside (a free-standing fin) — keep the first-authored twin. Unpaired
/// triangles keep their authored orientation untouched.
///
/// Geometry note: which twin is kept never changes the occluder SHAPE (the
/// twins coincide, and the evaluator intersects double-sided), so the
/// provisional occluder set built before the parity pass is already the
/// final one.
pub(crate) fn dedup_double_sided(
    positions: &[Vec3f],
    indices: &mut Vec<u32>,
    min: Vec3f,
    max: Vec3f,
) {
    let tri_count = indices.len() / 3;
    if tri_count == 0 {
        return;
    }
    // Same position-welding quantisation as `bake_into`'s adjacency pass.
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
    let mut canon_of: std::collections::HashMap<(i64, i64, i64), u32> =
        std::collections::HashMap::with_capacity(positions.len());
    let canon: Vec<u32> = positions
        .iter()
        .enumerate()
        .map(|(i, p)| *canon_of.entry(quant(*p)).or_insert(i as u32))
        .collect();

    let fnorm = |t: usize| {
        let (a, b, c) = (
            positions[indices[t * 3] as usize],
            positions[indices[t * 3 + 1] as usize],
            positions[indices[t * 3 + 2] as usize],
        );
        vnorm(vcross(vsub(b, a), vsub(c, a)))
    };

    // Group triangles on their SORTED welded-vertex triple: twins share the
    // set of positions whatever their winding or vertex duplication.
    let mut groups: std::collections::HashMap<[u32; 3], Vec<usize>> =
        std::collections::HashMap::with_capacity(tri_count);
    for t in 0..tri_count {
        let mut key = [
            canon[indices[t * 3] as usize],
            canon[indices[t * 3 + 1] as usize],
            canon[indices[t * 3 + 2] as usize],
        ];
        key.sort_unstable();
        groups.entry(key).or_default().push(t);
    }

    // Pair opposite windings greedily within each group.
    let mut twin = vec![usize::MAX; tri_count];
    for g in groups.values() {
        let mut used = vec![false; g.len()];
        for i in 0..g.len() {
            if used[i] {
                continue;
            }
            for j in i + 1..g.len() {
                if !used[j] && vdot(fnorm(g[i]), fnorm(g[j])) < -0.5 {
                    twin[g[i]] = g[j];
                    twin[g[j]] = g[i];
                    used[i] = true;
                    used[j] = true;
                    break;
                }
            }
        }
    }

    // Provisional single-sided occluder set: the first twin of each pair
    // plus everything unpaired. Final by construction — see above.
    let mut drop = vec![false; tri_count];
    for t in 0..tri_count {
        if twin[t] != usize::MAX && twin[t] < t {
            drop[t] = true;
        }
    }
    let occ_idx: Vec<u32> = (0..tri_count)
        .filter(|t| !drop[*t])
        .flat_map(|t| indices[t * 3..t * 3 + 3].to_vec())
        .collect();

    // OPENNESS vote, not inside/outside parity (the same oracle the
    // lightmapper engine's dedup uses, for the same reason): parity needs
    // watertight members and the kits do not deliver them — the gate arch's
    // segmented tube misvoted whole quads, a 113-level per-triangle seam on
    // a flat face. Keep the twin whose winding side sees more open space;
    // near-ties keep the first-authored twin.
    for t in 0..tri_count {
        let other = twin[t];
        if other == usize::MAX || other < t {
            continue; // unpaired, or the pair was handled from its lower id
        }
        let n = fnorm(t);
        let (a, b, c) = (
            positions[indices[t * 3] as usize],
            positions[indices[t * 3 + 1] as usize],
            positions[indices[t * 3 + 2] as usize],
        );
        let centroid = Vec3f {
            x: (a.x + b.x + c.x) / 3.0,
            y: (a.y + b.y + c.y) / 3.0,
            z: (a.z + b.z + c.z) / 3.0,
        };
        let open_front =
            crate::ao_lightmapper::openness(centroid, n, positions, &occ_idx, span);
        let open_back = crate::ao_lightmapper::openness(
            centroid,
            Vec3f { x: -n.x, y: -n.y, z: -n.z },
            positions,
            &occ_idx,
            span,
        );
        let keep_t = open_back <= open_front * 1.15 + 1.0e-3;
        drop[t] = !keep_t;
        drop[other] = keep_t;
    }

    let mut out = Vec::with_capacity(indices.len() / 2);
    for t in 0..tri_count {
        if !drop[t] {
            out.extend_from_slice(&indices[t * 3..t * 3 + 3]);
        }
    }
    *indices = out;
}

/// A fixed, uniform lattice over the unit SPHERE — the deterministic stand-in
/// for aobaker's `random_direction`. Uniform in AREA (even steps in z),
/// deliberately not cosine-weighted: the per-texel fold against the normal is
/// aobaker's only hemisphere shaping.
fn sphere_dirs() -> [Vec3f; AOBAKER_RAYS] {
    let mut out = [Vec3f { x: 0.0, y: 1.0, z: 0.0 }; AOBAKER_RAYS];
    for (i, slot) in out.iter_mut().enumerate() {
        let z = 1.0 - (2.0 * i as f32 + 1.0) / AOBAKER_RAYS as f32;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let phi = i as f32 * GOLDEN_ANGLE;
        *slot = Vec3f { x: r * phi.cos(), y: r * phi.sin(), z };
    }
    out
}

/// Rasterise one chart with aobaker's evaluator. Drop-in for
/// `rasterise_chart` and [`crate::ao_reference::bake_reference`]: same
/// inputs, same coverage, same dilation — aobaker's occlusion answer.
///
/// Coverage accumulates each subsample's POSITION and NORMAL rather than an
/// AO value, and the rays are shot once per covered texel at the averaged
/// point — aobaker's own model of one sample position and one interpolated
/// normal per texel.
#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_aobaker_chart(
    c: &Chart,
    sampler: &AoSampler,
    occ_pos: &[Vec3f],
    occ_idx: &[u32],
    positions: &[Vec3f],
    normals: &[Vec3f],
    indices: &[u32],
    params: &AobakerParams,
) -> Vec<u8> {
    // aobaker's tfar is FLT_MAX; bounded here by the occluder set's own
    // diagonal because the grid march needs a finite length to walk, and
    // nothing sits further apart than the set's corners.
    let (mut lo, mut hi) = (
        Vec3f { x: f32::MAX, y: f32::MAX, z: f32::MAX },
        Vec3f { x: f32::MIN, y: f32::MIN, z: f32::MIN },
    );
    for p in occ_pos {
        lo.x = lo.x.min(p.x);
        lo.y = lo.y.min(p.y);
        lo.z = lo.z.min(p.z);
        hi.x = hi.x.max(p.x);
        hi.y = hi.y.max(p.y);
        hi.z = hi.z.max(p.z);
    }
    let (dx, dy, dz) = (hi.x - lo.x, hi.y - lo.y, hi.z - lo.z);
    let tfar = (dx * dx + dy * dy + dz * dz).sqrt().max(1.0e-3) * 1.01;
    // 1e-3 of the model span, applied ALONG the ray — the sampler computed
    // exactly this epsilon from the model bounds at construction.
    let tnear = sampler.epsilon();
    let dirs = sphere_dirs();

    // --- Coverage: byte-for-byte the loops of `rasterise_chart` ------------
    // Any drift here and the atlases stop being comparable, so this mirrors
    // the production rasteriser exactly — same subsample offsets, same
    // clamped barycentrics, same texel-distance acceptance — with the single
    // change that it banks positions and normals instead of AO.
    let texels = c.w * c.h;
    let mut pos_acc = vec![Vec3f { x: 0.0, y: 0.0, z: 0.0 }; texels];
    let mut nrm_acc = vec![Vec3f { x: 0.0, y: 0.0, z: 0.0 }; texels];
    let mut cnt = vec![0.0f32; texels];
    let fallback = Vec3f { x: 0.0, y: 1.0, z: 0.0 };
    const SUB: [(f32, f32); TEXEL_SUBSAMPLES] = TEXEL_SUB_OFFSETS;

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
                    pos_acc[i].x += pa.x * w0 + pb.x * w1 + pc.x * w2;
                    pos_acc[i].y += pa.y * w0 + pb.y * w1 + pc.y * w2;
                    pos_acc[i].z += pa.z * w0 + pb.z * w1 + pc.z * w2;
                    nrm_acc[i].x += na.x * w0 + nb.x * w1 + nc.x * w2;
                    nrm_acc[i].y += na.y * w0 + nb.y * w1 + nc.y * w2;
                    nrm_acc[i].z += na.z * w0 + nb.z * w1 + nc.z * w2;
                    cnt[i] += 1.0;
                }
            }
        }
    }

    // --- Evaluate: aobaker's raytrace.cpp, once per covered texel ----------
    let mut px = vec![255u8; texels];
    let mut covered = vec![false; texels];
    for i in 0..texels {
        if cnt[i] <= 0.0 {
            continue;
        }
        let inv = 1.0 / cnt[i];
        let p = Vec3f {
            x: pos_acc[i].x * inv,
            y: pos_acc[i].y * inv,
            z: pos_acc[i].z * inv,
        };
        // Normalised average of the subsample normals; a texel whose votes
        // cancel has no facing and falls back to up, the machinery's own
        // missing-normal fallback. (aobaker instead SKIPS zero-normal texels
        // — but its zero normal means "uncovered", which the coverage mask
        // here answers.)
        let n = {
            let l = (nrm_acc[i].x * nrm_acc[i].x
                + nrm_acc[i].y * nrm_acc[i].y
                + nrm_acc[i].z * nrm_acc[i].z)
                .sqrt();
            if l < 1.0e-8 {
                fallback
            } else {
                Vec3f {
                    x: nrm_acc[i].x / l,
                    y: nrm_acc[i].y / l,
                    z: nrm_acc[i].z / l,
                }
            }
        };
        let mut hits = 0usize;
        for d in &dirs {
            // aobaker: flip a below-horizon sphere direction into the
            // hemisphere — `if (dotp < 0) dir = -dir`.
            let mut d = *d;
            if d.x * n.x + d.y * n.y + d.z * n.z < 0.0 {
                d = Vec3f { x: -d.x, y: -d.y, z: -d.z };
            }
            // tnear along the RAY: identical to starting the ray at the
            // surface and rejecting hits nearer than the epsilon.
            let origin = Vec3f {
                x: p.x + d.x * tnear,
                y: p.y + d.y * tnear,
                z: p.z + d.z * tnear,
            };
            if sampler.any_hit_aobaker(occ_pos, occ_idx, origin, d, tfar, params.cull_backfaces) {
                hits += 1;
            }
        }
        // raytrace.cpp verbatim: ao = multiply * (1 - hits/N);
        // results[i] = min(255, 255 * ao). No floor, no remap.
        let ao = params.multiply * (1.0 - hits as f32 / AOBAKER_RAYS as f32);
        px[i] = (255.0 * ao).min(255.0) as u8;
        covered[i] = true;
    }

    // --- Dilation: identical to `rasterise_chart`'s -----------------------
    for _ in 0..GUTTER + 1 {
        let src = px.clone();
        let mask = covered.clone();
        for ty in 0..c.h {
            for tx in 0..c.w {
                if mask[ty * c.w + tx] {
                    continue;
                }
                let (mut sum, mut n) = (0u32, 0u32);
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        let (nx, ny) = (tx as isize + dx, ty as isize + dy);
                        if nx < 0 || ny < 0 || nx >= c.w as isize || ny >= c.h as isize {
                            continue;
                        }
                        let ni = ny as usize * c.w + nx as usize;
                        if mask[ni] {
                            sum += src[ni] as u32;
                            n += 1;
                        }
                    }
                }
                if n > 0 {
                    px[ty * c.w + tx] = (sum / n) as u8;
                    covered[ty * c.w + tx] = true;
                }
            }
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::dedup_double_sided;
    use crate::ao::AoSampler;
    use makepad_draw::makepad_math::Vec3f;

    fn v(x: f32, y: f32, z: f32) -> Vec3f {
        Vec3f { x, y, z }
    }
    fn sub(a: Vec3f, b: Vec3f) -> Vec3f {
        v(a.x - b.x, a.y - b.y, a.z - b.z)
    }
    fn dot(a: Vec3f, b: Vec3f) -> f32 {
        a.x * b.x + a.y * b.y + a.z * b.z
    }
    fn cross(a: Vec3f, b: Vec3f) -> Vec3f {
        v(
            a.y * b.z - a.z * b.y,
            a.z * b.x - a.x * b.z,
            a.x * b.y - a.y * b.x,
        )
    }

    /// The two intersection switches the port depends on, proven on a mesh
    /// where they are DISTINGUISHABLE - one single-sided triangle plus one
    /// "virtual ground" triangle.
    #[test]
    fn cull_and_ground_switches_are_live() {
        // Triangle 0: the "model", a roof over the origin, wound to face +y.
        // Triangle 1: the "ground" below, also facing +y.
        let pos = vec![
            v(-5.0, 1.0, -5.0),
            v(5.0, 1.0, -5.0),
            v(0.0, 1.0, 5.0),
            v(-5.0, -1.0, -5.0),
            v(5.0, -1.0, -5.0),
            v(0.0, -1.0, 5.0),
        ];
        let idx = vec![0u32, 2, 1, 3, 5, 4];
        let lo = v(-5.0, -1.0, -5.0);
        let hi = v(5.0, 1.0, 5.0);
        let s = AoSampler::with_reach(&pos, &idx, lo, hi, lo, hi, 32, 1);
        let o = v(0.0, 0.0, 0.0);
        let up = v(0.0, 1.0, 0.0);
        let down = v(0.0, -1.0, 0.0);
        // Upward from below: the roof's BACK. Double-sided blocks, culled not.
        assert!(s.any_hit_aobaker(&pos, &idx, o, up, 10.0, false));
        assert!(
            !s.any_hit_aobaker(&pos, &idx, o, up, 10.0, true),
            "a backface hit survived the cull"
        );
        // Downward from above the roof: its FRONT. Blocks either way.
        assert!(s.any_hit_aobaker(&pos, &idx, v(0.0, 2.0, 0.0), down, 10.0, true));
        // Downward from the origin: only the ground triangle is in the way.
        // The port must not see it; the machinery's nearest-hit must.
        assert!(
            !s.any_hit_aobaker(&pos, &idx, o, down, 10.0, false),
            "the virtual ground leaked into the aobaker scene"
        );
        assert!(s.nearest_hit(&pos, &idx, o, down, 10.0).is_finite());
    }

    /// The dedup pass on a doubled sheet next to a solid, twins' authoring
    /// order deliberately scrambled: exactly one triangle per pair must
    /// survive, and every survivor must face AWAY from the solid — the
    /// openness vote working, not the authoring order leaking through.
    /// (The vote's contract is the kits': a face's closed side is within the
    /// openness reach. A lone hollless-than-reach box reads open on both
    /// sides and deliberately keeps the authored winding.)
    #[test]
    fn dedup_keeps_one_outward_twin_per_pair() {
        // A large single-sided ground quad (unpaired: must pass untouched),
        // with a small DOUBLED quad 5cm above it. The doubled quad's down
        // side stares at the ground; its up side sees open sky.
        let positions = vec![
            v(-5.0, 0.0, -5.0),
            v(5.0, 0.0, -5.0),
            v(5.0, 0.0, 5.0),
            v(-5.0, 0.0, 5.0),
            v(-0.5, 0.05, -0.5),
            v(0.5, 0.05, -0.5),
            v(0.5, 0.05, 0.5),
            v(-0.5, 0.05, 0.5),
        ];
        let up_tris: [[u32; 3]; 2] = [[4, 6, 5], [4, 7, 6]]; // wind +y
        let mut indices: Vec<u32> = vec![0, 2, 1, 0, 3, 2]; // ground, up
        // Scramble: first pair authors the DOWN twin first, second the UP.
        for (i, tri) in up_tris.iter().enumerate() {
            let down = [tri[0], tri[2], tri[1]];
            if i == 0 {
                indices.extend_from_slice(&down);
                indices.extend_from_slice(tri);
            } else {
                indices.extend_from_slice(tri);
                indices.extend_from_slice(&down);
            }
        }
        dedup_double_sided(&positions, &mut indices, v(-5.0, 0.0, -5.0), v(5.0, 0.05, 5.0));
        assert_eq!(indices.len(), (2 + 2) * 3, "one survivor per twin pair");
        // Survivors of the doubled quad (the last two triangles) wind UP.
        for t in 2..4 {
            let (a, b, c) = (
                positions[indices[t * 3] as usize],
                positions[indices[t * 3 + 1] as usize],
                positions[indices[t * 3 + 2] as usize],
            );
            let n = cross(sub(b, a), sub(c, a));
            assert!(
                n.y > 0.0,
                "survivor {t} faces the solid it rests on — the openness vote \
                 picked the wrong twin (n.y {})",
                n.y
            );
        }
    }
}
