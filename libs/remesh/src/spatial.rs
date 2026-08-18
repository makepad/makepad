//! Uniform 64^3 bin grid over [-1,1]^3 — the same broadphase the validated
//! CPU oracle (cpu_polyfill.py FakeBVH) uses, so nearest-hit / closest-point
//! results are identical by construction (lexicographic (value, tri_id) mins).
//!
//! Triangle AABBs are dilated by 1e-4 at binning time, which makes bin lookup
//! a provable superset for: SAT queries dilated up to 1e-4, ray/segment hits
//! (MT eps_edge tolerance << 1e-4), and closest-point shells.

use crate::math::*;

pub const GRID_LEVEL: u32 = 6;
pub const GRID_RES: i64 = 1 << GRID_LEVEL;
pub const BIN_SIZE: f64 = 2.0 / GRID_RES as f64;
pub const BIN_DILATE: f64 = 1e-4;

pub struct BinGrid {
    starts: Vec<u32>,
    counts: Vec<u32>,
    /// triangle ids, ascending within each bin
    tris: Vec<u32>,
    /// per-triangle AABBs (exact f32 min/max of the vertices)
    pub tmin: Vec<V3>,
    pub tmax: Vec<V3>,
}

#[inline]
fn bin_of(x: f64) -> i64 {
    (((x + 1.0) / BIN_SIZE).floor() as i64).clamp(0, GRID_RES - 1)
}

#[inline]
fn bin_id(b: [i64; 3]) -> usize {
    ((b[0] * GRID_RES + b[1]) * GRID_RES + b[2]) as usize
}

impl BinGrid {
    pub fn build(tris: &[[V3; 3]]) -> BinGrid {
        let n_bins = (GRID_RES * GRID_RES * GRID_RES) as usize;
        let mut counts = vec![0u32; n_bins];
        let mut tmin = Vec::with_capacity(tris.len());
        let mut tmax = Vec::with_capacity(tris.len());
        let spans: Vec<([i64; 3], [i64; 3])> = tris
            .iter()
            .map(|t| {
                let mn = min3(min3(t[0], t[1]), t[2]);
                let mx = max3(max3(t[0], t[1]), t[2]);
                tmin.push(mn);
                tmax.push(mx);
                let lo = [
                    bin_of(mn[0] as f64 - BIN_DILATE),
                    bin_of(mn[1] as f64 - BIN_DILATE),
                    bin_of(mn[2] as f64 - BIN_DILATE),
                ];
                let hi = [
                    bin_of(mx[0] as f64 + BIN_DILATE),
                    bin_of(mx[1] as f64 + BIN_DILATE),
                    bin_of(mx[2] as f64 + BIN_DILATE),
                ];
                (lo, hi)
            })
            .collect();
        for &(lo, hi) in &spans {
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        counts[bin_id([x, y, z])] += 1;
                    }
                }
            }
        }
        let mut starts = vec![0u32; n_bins];
        let mut acc = 0u32;
        for i in 0..n_bins {
            starts[i] = acc;
            acc += counts[i];
        }
        let mut fill = starts.clone();
        let mut tris_out = vec![0u32; acc as usize];
        // iterate tris in ascending id: per-bin lists come out ascending
        for (tid, &(lo, hi)) in spans.iter().enumerate() {
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        let b = bin_id([x, y, z]);
                        tris_out[fill[b] as usize] = tid as u32;
                        fill[b] += 1;
                    }
                }
            }
        }
        BinGrid {
            starts,
            counts,
            tris: tris_out,
            tmin,
            tmax,
        }
    }

    #[inline]
    pub fn bin_tris(&self, b: [i64; 3]) -> &[u32] {
        let id = bin_id(b);
        let s = self.starts[id] as usize;
        &self.tris[s..s + self.counts[id] as usize]
    }
}

/// Möller-Trumbore, kernel-exact (bvh_kernels.cu Triangle::ray_intersect):
/// f32 compute, u/v edge tests in FLOAT64 vs FLT_EPSILON; returns 1e10 on miss.
#[inline]
pub fn ray_tri_intersect(ro: V3, rd: V3, tri: &[V3; 3]) -> f32 {
    const EPS: f32 = 1e-8;
    const EPS_EDGE: f64 = 1.192_092_90e-7;
    let e1 = sub3(tri[1], tri[0]);
    let e2 = sub3(tri[2], tri[0]);
    let h = cross3(rd, e2);
    let det = dot3(e1, h);
    if det.abs() < EPS {
        return 1e10;
    }
    let f = 1.0f32 / det;
    let s = sub3(ro, tri[0]);
    let u = f as f64 * dot3(s, h) as f64;
    if u < -EPS_EDGE || u > 1.0 + EPS_EDGE {
        return 1e10;
    }
    let q = cross3(s, e1);
    let v = f as f64 * dot3(rd, q) as f64;
    if v < -EPS_EDGE || u + v > 1.0 + EPS_EDGE {
        return 1e10;
    }
    let t = f * dot3(e2, q);
    if t > EPS {
        t
    } else {
        1e10
    }
}

/// Nearest hit for a segment-as-ray: mint starts at max_t (strict <),
/// ties at equal fp32 t break toward the LOWEST triangle id (candidates are
/// visited ascending). Returns (t, face_id) with face_id = -1 on miss.
pub fn segment_nearest_hit(
    grid: &BinGrid,
    tris: &[[V3; 3]],
    s0: V3,
    s1: V3,
    ro: V3,
    rd: V3,
    max_t: f32,
) -> (f32, i64) {
    let mn = [
        s0[0].min(s1[0]) as f64 - BIN_DILATE,
        s0[1].min(s1[1]) as f64 - BIN_DILATE,
        s0[2].min(s1[2]) as f64 - BIN_DILATE,
    ];
    let mx = [
        s0[0].max(s1[0]) as f64 + BIN_DILATE,
        s0[1].max(s1[1]) as f64 + BIN_DILATE,
        s0[2].max(s1[2]) as f64 + BIN_DILATE,
    ];
    let lo = [bin_of(mn[0]), bin_of(mn[1]), bin_of(mn[2])];
    let hi = [bin_of(mx[0]), bin_of(mx[1]), bin_of(mx[2])];
    let mut best_t = max_t;
    let mut best_face = -1i64;
    // Prefilter: an MT-accepted hit lies on the segment span and within
    // ~2e-7 of the triangle, so its aabb must overlap the segment aabb
    // dilated by far less than 1e-5 — skipping non-overlapping triangles
    // cannot change the result (the CUDA segment kernel has the same test).
    let pmn = [
        (mn[0] + (BIN_DILATE - 1e-5)) as f32,
        (mn[1] + (BIN_DILATE - 1e-5)) as f32,
        (mn[2] + (BIN_DILATE - 1e-5)) as f32,
    ];
    let pmx = [
        (mx[0] - (BIN_DILATE - 1e-5)) as f32,
        (mx[1] - (BIN_DILATE - 1e-5)) as f32,
        (mx[2] - (BIN_DILATE - 1e-5)) as f32,
    ];
    // Lexicographic (t, tri_id) minimum: identical to the oracle's
    // "visit ascending id, strict < on t" (lowest id wins fp32-equal-t ties)
    // but independent of visit order, so duplicate candidates from multiple
    // bins are harmless and no per-query sort is needed.
    let mut visit = |tid: u32| {
        let ti = tid as usize;
        let tmn = grid.tmin[ti];
        let tmx = grid.tmax[ti];
        if tmn[0] > pmx[0]
            || tmx[0] < pmn[0]
            || tmn[1] > pmx[1]
            || tmx[1] < pmn[1]
            || tmn[2] > pmx[2]
            || tmx[2] < pmn[2]
        {
            return;
        }
        let t = ray_tri_intersect(ro, rd, &tris[ti]);
        if t < best_t || (t == best_t && best_face >= 0 && (tid as i64) < best_face) {
            best_t = t;
            best_face = tid as i64;
        }
    };
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                for &tid in grid.bin_tris([x, y, z]) {
                    visit(tid);
                }
            }
        }
    }
    (best_t, best_face)
}

/// Kernel Triangle::distance_sq (bvh_kernels.cu): sign_f(x) = x >= 0 ? 1 : -1.
pub fn tri_distance_sq(pos: V3, tri: &[V3; 3]) -> f32 {
    #[inline(always)]
    fn sign_f(x: f32) -> f32 {
        if x >= 0.0 {
            1.0
        } else {
            -1.0
        }
    }
    let (a, b, c) = (tri[0], tri[1], tri[2]);
    let v21 = sub3(b, a);
    let p1 = sub3(pos, a);
    let v32 = sub3(c, b);
    let p2 = sub3(pos, b);
    let v13 = sub3(a, c);
    let p3 = sub3(pos, c);
    let nor = cross3(v21, v13);
    let nor_sq = dot3(nor, nor);
    let degen = nor_sq < 1e-12;
    let outside = if degen {
        true
    } else {
        let st = sign_f(dot3(cross3(v21, nor), p1))
            + sign_f(dot3(cross3(v32, nor), p2))
            + sign_f(dot3(cross3(v13, nor), p3));
        st < 2.0
    };
    if outside {
        #[inline(always)]
        fn edge_dist(v: V3, p: V3) -> f32 {
            let mut d = dot3(v, p) / dot3(v, v).max(1e-12);
            d = d.min(1.0).max(0.0); // clamp_f(x, 0, 1) = fmaxf(0, fminf(1, x))
            let cv = sub3(scale3(v, d), p);
            dot3(cv, cv)
        }
        edge_dist(v21, p1).min(edge_dist(v32, p2).min(edge_dist(v13, p3)))
    } else {
        let d = dot3(nor, p1);
        d * d / nor_sq.max(1e-12)
    }
}

/// Kernel Triangle::closest_point (mirrors distance_sq branch logic).
pub fn tri_closest_point(pos: V3, tri: &[V3; 3]) -> V3 {
    #[inline(always)]
    fn sign_f(x: f32) -> f32 {
        if x >= 0.0 {
            1.0
        } else {
            -1.0
        }
    }
    let (a, b, c) = (tri[0], tri[1], tri[2]);
    let v21 = sub3(b, a);
    let p1 = sub3(pos, a);
    let v32 = sub3(c, b);
    let p2 = sub3(pos, b);
    let v13 = sub3(a, c);
    let p3 = sub3(pos, c);
    let nor = cross3(v21, v13);
    let nor_sq = dot3(nor, nor);
    let degen = nor_sq < 1e-12;
    let outside = if degen {
        true
    } else {
        let st = sign_f(dot3(cross3(v21, nor), p1))
            + sign_f(dot3(cross3(v32, nor), p2))
            + sign_f(dot3(cross3(v13, nor), p3));
        st < 2.0
    };
    if outside {
        #[inline(always)]
        fn edge_closest(v: V3, p: V3, origin: V3) -> V3 {
            let mut d = dot3(v, p) / dot3(v, v).max(1e-12);
            d = d.min(1.0).max(0.0);
            add3(origin, scale3(v, d))
        }
        let c1 = edge_closest(v21, p1, a);
        let c2 = edge_closest(v32, p2, b);
        let c3 = edge_closest(v13, p3, c);
        let d1 = dot3(sub3(c1, pos), sub3(c1, pos));
        let d2 = dot3(sub3(c2, pos), sub3(c2, pos));
        let d3 = dot3(sub3(c3, pos), sub3(c3, pos));
        if d1 < d2 && d1 < d3 {
            c1
        } else if d2 < d3 {
            c2
        } else {
            c3
        }
    } else {
        let d = dot3(nor, p1);
        let proj = scale3(nor, d / nor_sq.max(1e-12));
        sub3(pos, proj)
    }
}

/// Kernel Triangle::barycentric (denom clamped via fmax(denom, 1e-10)).
pub fn tri_barycentric(p: V3, tri: &[V3; 3]) -> V3 {
    let (a, b, c) = (tri[0], tri[1], tri[2]);
    let v0 = sub3(b, a);
    let v1 = sub3(c, a);
    let v2 = sub3(p, a);
    let d00 = dot3(v0, v0);
    let d01 = dot3(v0, v1);
    let d11 = dot3(v1, v1);
    let d20 = dot3(v2, v0);
    let d21 = dot3(v2, v1);
    let denom = (d00 * d11 - d01 * d01).max(1e-10);
    let v = (d11 * d20 - d01 * d21) / denom;
    let w = (d00 * d21 - d01 * d20) / denom;
    [1.0f32 - v - w, v, w]
}

/// Point-to-AABB squared distance (fp32).
#[inline]
fn point_aabb_dist_sq(p: V3, mn: V3, mx: V3) -> f32 {
    let mut d = 0.0f32;
    for a in 0..3 {
        let da = (mn[a] - p[a]).max(0.0).max(p[a] - mx[a]);
        d += da * da;
    }
    d
}

/// Closest triangle to a point: expanding shell search over the bin grid.
/// Lexicographic (dist, tri_id) minimum == the oracle's "ascending id,
/// strict <" tie rule, independent of visit order (duplicates harmless).
/// Candidates are pruned by a point-to-tri-AABB lower bound with fp slack:
/// a pruned triangle provably cannot beat or tie the current best.
pub fn closest_tri(grid: &BinGrid, tris: &[[V3; 3]], p: V3) -> (f32, i64) {
    let pb = [bin_of(p[0] as f64), bin_of(p[1] as f64), bin_of(p[2] as f64)];
    let mut best_d = f32::INFINITY;
    let mut best_id = -1i64;
    let mut r: i64 = 1;
    while r <= GRID_RES {
        let lo = [
            (pb[0] - r).clamp(0, GRID_RES - 1),
            (pb[1] - r).clamp(0, GRID_RES - 1),
            (pb[2] - r).clamp(0, GRID_RES - 1),
        ];
        let hi = [
            (pb[0] + r).clamp(0, GRID_RES - 1),
            (pb[1] + r).clamp(0, GRID_RES - 1),
            (pb[2] + r).clamp(0, GRID_RES - 1),
        ];
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    for &tid in grid.bin_tris([x, y, z]) {
                        let ti = tid as usize;
                        // relative + absolute slack covers all fp error in
                        // the bound vs the kernel distance formula
                        // (rel ~1e-6 << 1e-5; abs ~1e-14 << 1e-12)
                        let bound = point_aabb_dist_sq(p, grid.tmin[ti], grid.tmax[ti]);
                        if best_id >= 0 && bound > best_d * 1.00001 + 1e-12 {
                            continue;
                        }
                        let d = tri_distance_sq(p, &tris[ti]);
                        if d < best_d || (d == best_d && best_id >= 0 && (tid as i64) < best_id) {
                            best_d = d;
                            best_id = tid as i64;
                        }
                    }
                }
            }
        }
        // provable termination: the searched block covers every triangle
        // whose DILATED aabb touches it; any unseen triangle is farther from
        // p than the distance to the block boundary minus the dilation.
        // Domain-clamped sides cover everything beyond them (all binned
        // geometry clamps into bins 0..GRID_RES-1), so they bound nothing.
        let mut boundary = f64::INFINITY;
        for a in 0..3 {
            if lo[a] > 0 {
                boundary = boundary.min(p[a] as f64 - (-1.0 + lo[a] as f64 * BIN_SIZE));
            }
            if hi[a] < GRID_RES - 1 {
                boundary = boundary.min((-1.0 + (hi[a] + 1) as f64 * BIN_SIZE) - p[a] as f64);
            }
        }
        let guaranteed = boundary - BIN_DILATE;
        if best_id >= 0 && (best_d as f64).sqrt() <= guaranteed.max(0.0) {
            break;
        }
        // full-grid block searched and still nothing better possible
        if lo == [0, 0, 0] && hi == [GRID_RES - 1, GRID_RES - 1, GRID_RES - 1] {
            break;
        }
        r += 1;
    }
    (best_d, best_id)
}
