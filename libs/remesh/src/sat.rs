//! Triangle-box SAT + Sutherland-Hodgman clip, kernel-exact fp32 transcription.
//!
//! SAT: atom3d bvh_kernels.cu `triangle_aabb_sat` (13 axes, eps-dilated,
//! plane normal = cross(e0, v2-v0) — the flavor the real BVH path runs).
//! Clip: atom3d cumtv_kernels.cu `sat_clip_polygon_kernel` + `clip_with_plane`
//! (eps=1e-8, plane order +x,-x,+y,-y,+z,-z; centroid = mean -> clamp to box
//! -> project to triangle plane with clamped barycentrics; fan area about the
//! CLAMPED pre-projection mean; area<eps -> 0 with hit kept; degenerate
//! projection denom -> centroid = tri v0, area stays 0).

use crate::math::*;

/// SAT epsilon the encoder path actually uses (library default; the comments
/// in the reference recommend 1e-5 but the encoder never overrides it).
pub const SAT_EPS: f32 = 1e-6;
/// Superset-narrowing epsilon for the fused octree recursion. Any triangle
/// that fp-SAT(1e-6)-hits a descendant box provably fp-SAT(1e-4)-hits every
/// ancestor box (interval nesting with >25x fp-error headroom).
pub const NARROW_EPS: f32 = 1e-4;

pub const CLIP_EPS: f32 = 1e-8;

/// Akenine-Möller 13-axis SAT with eps dilation, bit-exact vs the oracle.
pub fn tri_box_sat(tri: &[V3; 3], bmin: V3, bmax: V3, eps: f32) -> bool {
    let c = [
        (bmin[0] + bmax[0]) * 0.5,
        (bmin[1] + bmax[1]) * 0.5,
        (bmin[2] + bmax[2]) * 0.5,
    ];
    let h = [
        (bmax[0] - bmin[0]) * 0.5,
        (bmax[1] - bmin[1]) * 0.5,
        (bmax[2] - bmin[2]) * 0.5,
    ];
    let v0 = sub3(tri[0], c);
    let v1 = sub3(tri[1], c);
    let v2 = sub3(tri[2], c);

    // slab tests
    for a in 0..3 {
        let mn = v0[a].min(v1[a]).min(v2[a]);
        let mx = v0[a].max(v1[a]).max(v2[a]);
        if mn > h[a] + eps || mx < -h[a] - eps {
            return false;
        }
    }

    let e0 = sub3(v1, v0);
    let e1 = sub3(v2, v1);
    let e2 = sub3(v0, v2);

    #[inline(always)]
    fn axis_test(v0: V3, v1: V3, v2: V3, h: V3, edge: V3, axis: usize, eps: f32) -> bool {
        let a: V3 = match axis {
            0 => [0.0, edge[2], -edge[1]],
            1 => [-edge[2], 0.0, edge[0]],
            _ => [edge[1], -edge[0], 0.0],
        };
        let p0 = dot3(v0, a);
        let p1 = dot3(v1, a);
        let p2 = dot3(v2, a);
        let mn = p0.min(p1).min(p2);
        let mx = p0.max(p1).max(p2);
        let rad = h[0] * a[0].abs() + h[1] * a[1].abs() + h[2] * a[2].abs()
            + eps * dot3(a, a).sqrt();
        !(mn > rad || mx < -rad)
    }

    for edge in [e0, e1, e2] {
        for axis in 0..3 {
            if !axis_test(v0, v1, v2, h, edge, axis, eps) {
                return false;
            }
        }
    }

    // plane test (bvh flavor: cross(e0, v2 - v0))
    let n = cross3(e0, sub3(v2, v0));
    let d = -dot3(n, v0);
    let r = h[0] * n[0].abs() + h[1] * n[1].abs() + h[2] * n[2].abs() + eps * dot3(n, n).sqrt();
    if d.abs() > r {
        return false;
    }
    true
}

pub struct ClipResult {
    pub hit: bool,
    pub centroid: V3,
    pub area: f32,
}

/// Sutherland-Hodgman clip of a triangle against a box (kernel-exact).
pub fn clip_tri_box(tri: &[V3; 3], bmin: V3, bmax: V3) -> ClipResult {
    let eps = CLIP_EPS;
    let miss = ClipResult { hit: false, centroid: [0.0; 3], area: 0.0 };

    let mut poly_a = [[0.0f32; 3]; 10];
    let mut poly_b = [[0.0f32; 3]; 10];
    poly_a[..3].copy_from_slice(tri);
    let mut cnt: usize = 3;

    // plane order: +x(d=bmax), -x(d=-bmin), +y, -y, +z, -z
    // dist = sign * p[axis] - d
    let planes: [(usize, f32, f32); 6] = [
        (0, 1.0, bmax[0]),
        (0, -1.0, -bmin[0]),
        (1, 1.0, bmax[1]),
        (1, -1.0, -bmin[1]),
        (2, 1.0, bmax[2]),
        (2, -1.0, -bmin[2]),
    ];

    let mut cur = &mut poly_a;
    let mut next = &mut poly_b;
    for &(axis, sgn, d) in planes.iter() {
        let mut dist = [0.0f32; 10];
        for i in 0..cnt {
            dist[i] = sgn * cur[i][axis] - d;
        }
        let mut out_cnt: usize = 0;
        for i in 0..cnt {
            let j = if i + 1 == cnt { 0 } else { i + 1 };
            let p = cur[i];
            let q = cur[j];
            let dp = dist[i];
            let dq = dist[j];
            let in_p = dp <= eps;
            let in_q = dq <= eps;
            if in_p && in_q {
                next[out_cnt] = q;
                out_cnt += 1;
            } else if in_p && !in_q {
                let denom = dp - dq;
                if denom.abs() > eps {
                    let t = dp / denom;
                    next[out_cnt] = [
                        p[0] + t * (q[0] - p[0]),
                        p[1] + t * (q[1] - p[1]),
                        p[2] + t * (q[2] - p[2]),
                    ];
                    out_cnt += 1;
                }
            } else if !in_p && in_q {
                let denom = dp - dq;
                if denom.abs() > eps {
                    let t = dp / denom;
                    next[out_cnt] = [
                        p[0] + t * (q[0] - p[0]),
                        p[1] + t * (q[1] - p[1]),
                        p[2] + t * (q[2] - p[2]),
                    ];
                    out_cnt += 1;
                }
                next[out_cnt] = q;
                out_cnt += 1;
            }
        }
        core::mem::swap(&mut cur, &mut next);
        cnt = out_cnt;
        if cnt == 0 {
            return miss;
        }
    }

    // centroid: sequential mean over cnt verts
    let mut cx = 0.0f32;
    let mut cy = 0.0f32;
    let mut cz = 0.0f32;
    for v in cur.iter().take(cnt) {
        cx += v[0];
        cy += v[1];
        cz += v[2];
    }
    cx /= cnt as f32;
    cy /= cnt as f32;
    cz /= cnt as f32;

    // clamp to box: fminf(fmaxf(c, bmin), bmax)
    cx = cx.max(bmin[0]).min(bmax[0]);
    cy = cy.max(bmin[1]).min(bmax[1]);
    cz = cz.max(bmin[2]).min(bmax[2]);

    // project onto triangle plane with barycentric clamping
    let a = tri[0];
    let b = tri[1];
    let cvert = tri[2];
    let e1 = sub3(b, a);
    let e2 = sub3(cvert, a);
    let v = [cx - a[0], cy - a[1], cz - a[2]];
    let d00 = dot3(e1, e1);
    let d01 = dot3(e1, e2);
    let d11 = dot3(e2, e2);
    let d20 = dot3(v, e1);
    let d21 = dot3(v, e2);
    let denom = d00 * d11 - d01 * d01;
    if denom.abs() < eps * eps {
        // degenerate triangle: centroid = v0, area stays 0, hit stays true
        return ClipResult { hit: true, centroid: a, area: 0.0 };
    }
    let mut v_bc = (d11 * d20 - d01 * d21) / denom;
    let mut w_bc = (d00 * d21 - d01 * d20) / denom;
    let mut u_bc = 1.0f32 - v_bc - w_bc;
    if u_bc < 0.0 {
        u_bc = 0.0;
    }
    if v_bc < 0.0 {
        v_bc = 0.0;
    }
    if w_bc < 0.0 {
        w_bc = 0.0;
    }
    let mut norm = u_bc + v_bc + w_bc;
    if norm <= eps {
        u_bc = 1.0;
        v_bc = 0.0;
        w_bc = 0.0;
        norm = 1.0;
    }
    u_bc /= norm;
    v_bc /= norm;
    w_bc /= norm;
    let centroid = [
        u_bc * a[0] + v_bc * b[0] + w_bc * cvert[0],
        u_bc * a[1] + v_bc * b[1] + w_bc * cvert[1],
        u_bc * a[2] + v_bc * b[2] + w_bc * cvert[2],
    ];

    // fan area about the CLAMPED pre-projection mean (sequential)
    let cmean = [cx, cy, cz];
    let mut area = 0.0f32;
    for i in 0..cnt {
        let j = (i + 1) % cnt;
        let f1 = sub3(cur[i], cmean);
        let f2 = sub3(cur[j], cmean);
        let cr = cross3(f1, f2);
        area += 0.5 * (cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt();
    }
    if area < eps {
        area = 0.0; // contact: hit kept, weight zero
    }
    ClipResult { hit: true, centroid, area }
}
