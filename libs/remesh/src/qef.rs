//! Weighted QEF solve per voxel — exact port of faithcontour/qef_solver.py
//! solve_qef (fp32, voxel-local coordinates, area weights).
//!
//! Per-voxel accumulation happens in sample order (= ascending triangle id,
//! the same order the oracle's scatter_add sees), so fp32 sums match.

use crate::math::*;

/// One (voxel, triangle) clip sample.
#[derive(Clone, Copy)]
pub struct QefSample {
    pub point: V3,  // projected clip centroid
    pub normal: V3, // encoder face normal (cross/clamp_min(1e-8))
    pub area: f32,  // clip area (weight)
}

pub struct QefResult {
    pub anchor: V3,
    pub normal: V3,
}

/// 3x3 linear solve with partial pivoting, LAPACK getf2-style
/// (reciprocal-of-pivot multiplies, first-maximum pivot selection).
fn solve3(a_in: [[f32; 3]; 3], b_in: V3) -> V3 {
    let mut a = a_in;
    let mut b = b_in;
    for k in 0..3 {
        // pivot: first row with max |a[i][k]|, i >= k
        let mut piv = k;
        let mut mx = a[k][k].abs();
        for i in k + 1..3 {
            let v = a[i][k].abs();
            if v > mx {
                mx = v;
                piv = i;
            }
        }
        if piv != k {
            a.swap(piv, k);
            b.swap(piv, k);
        }
        let r = 1.0f32 / a[k][k];
        for i in k + 1..3 {
            let l = a[i][k] * r;
            a[i][k] = l;
            for j in k + 1..3 {
                a[i][j] -= l * a[k][j];
            }
            b[i] -= l * b[k];
        }
    }
    // back substitution
    let mut x = [0.0f32; 3];
    for k in (0..3).rev() {
        let mut s = b[k];
        for j in k + 1..3 {
            s -= a[k][j] * x[j];
        }
        x[k] = s / a[k][k];
    }
    x
}

/// solve_qef for one voxel's samples. lambda/weight config mirrors the
/// reference defaults used by the demo: lambda_n=1.0, lambda_d=1e-3,
/// weight_power=1 (pow(1.0) == identity in torch).
pub fn solve_qef_voxel(
    samples: &[QefSample],
    cell: f32,
    lambda_n: f32,
    lambda_d: f32,
) -> QefResult {
    const EPS: f32 = 1e-12;
    let k = samples.len();

    // normalized weights: raw = clamp_min(area, 0); norm = raw/sum or 1/count
    let mut wsum = 0.0f32;
    for s in samples {
        wsum += s.area.max(0.0);
    }
    let uniform_w = (1.0f64 / k as f64) as f32;

    // group centroid: UNWEIGHTED mean of sample points (scatter_mean)
    let mut csum = [0.0f32; 3];
    for s in samples {
        csum = add3(csum, s.point);
    }
    let centroid = [csum[0] / k as f32, csum[1] / k as f32, csum[2] / k as f32];

    let inv_cell = (1.0f64 / cell as f64) as f32;

    // accumulate A = sum_i w_i (lambda_d I + lambda_n n n^T) (+ eps I), b = A_i @ local_i
    let mut a_mat = [[0.0f32; 3]; 3];
    let mut b_vec = [0.0f32; 3];
    let mut nsum = [0.0f32; 3];
    for s in samples {
        let raw = s.area.max(0.0);
        let w = if wsum > EPS { raw / wsum } else { uniform_w };
        let n = s.normal;
        let local = [
            (s.point[0] - centroid[0]) * inv_cell,
            (s.point[1] - centroid[1]) * inv_cell,
            (s.point[2] - centroid[2]) * inv_cell,
        ];
        // A_i[r][c] = w * (lambda_d * I + lambda_n * (n_r * n_c))
        let mut ai = [[0.0f32; 3]; 3];
        for r in 0..3 {
            for c in 0..3 {
                let base = if r == c { lambda_d } else { 0.0 } + lambda_n * (n[r] * n[c]);
                ai[r][c] = w * base;
            }
        }
        for r in 0..3 {
            let bi = ai[r][0] * local[0] + ai[r][1] * local[1] + ai[r][2] * local[2];
            b_vec[r] += bi;
            for c in 0..3 {
                a_mat[r][c] += ai[r][c];
            }
        }
        nsum[0] += w * n[0];
        nsum[1] += w * n[1];
        nsum[2] += w * n[2];
    }
    for d in 0..3 {
        a_mat[d][d] += EPS;
    }
    let x = solve3(a_mat, b_vec);
    let anchor = [
        x[0] * cell + centroid[0],
        x[1] * cell + centroid[1],
        x[2] * cell + centroid[2],
    ];
    let nn = norm3(nsum).max(EPS);
    let normal = [nsum[0] / nn, nsum[1] / nn, nsum[2] / nn];
    QefResult { anchor, normal }
}
