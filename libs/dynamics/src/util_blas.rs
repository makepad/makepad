//! Small vector and dense matrix operations.
//!
//! Translated from `src/engine/engine_util_blas.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

// Scalar branches only; these retain the C AVX reduction order.

#[inline]
pub fn zero3(res: &mut [f64; 3]) {
    res[0] = 0.0;
    res[1] = 0.0;
    res[2] = 0.0;
}

#[inline]
pub fn equal3(vec1: &[f64; 3], vec2: &[f64; 3]) -> i32 {
    return ((vec1[0] - vec2[0]).abs() < crate::types::MINVAL
        && (vec1[1] - vec2[1]).abs() < crate::types::MINVAL
        && (vec1[2] - vec2[2]).abs() < crate::types::MINVAL) as i32;
}

#[inline]
pub fn copy3(res: &mut [f64; 3], vec: &[f64; 3]) {
    res[0] = vec[0];
    res[1] = vec[1];
    res[2] = vec[2];
}

#[inline]
pub fn copy9(res: &mut [f64; 9], mat: &[f64; 9]) {
    res[0] = mat[0];
    res[1] = mat[1];
    res[2] = mat[2];
    res[3] = mat[3];
    res[4] = mat[4];
    res[5] = mat[5];
    res[6] = mat[6];
    res[7] = mat[7];
    res[8] = mat[8];
}

#[inline]
pub fn scl3(res: &mut [f64; 3], vec: &[f64; 3], scl: f64) {
    res[0] = vec[0] * scl;
    res[1] = vec[1] * scl;
    res[2] = vec[2] * scl;
}

#[inline]
pub fn add3(res: &mut [f64; 3], vec1: &[f64; 3], vec2: &[f64; 3]) {
    res[0] = vec1[0] + vec2[0];
    res[1] = vec1[1] + vec2[1];
    res[2] = vec1[2] + vec2[2];
}

#[inline]
pub fn sub3(res: &mut [f64; 3], vec1: &[f64; 3], vec2: &[f64; 3]) {
    res[0] = vec1[0] - vec2[0];
    res[1] = vec1[1] - vec2[1];
    res[2] = vec1[2] - vec2[2];
}

#[inline]
pub fn add_to3(res: &mut [f64; 3], vec: &[f64; 3]) {
    res[0] += vec[0];
    res[1] += vec[1];
    res[2] += vec[2];
}

#[inline]
pub fn sub_from3(res: &mut [f64; 3], vec: &[f64; 3]) {
    res[0] -= vec[0];
    res[1] -= vec[1];
    res[2] -= vec[2];
}

#[inline]
pub fn add_to_scl3(res: &mut [f64; 3], vec: &[f64; 3], scl: f64) {
    res[0] += vec[0] * scl;
    res[1] += vec[1] * scl;
    res[2] += vec[2] * scl;
}

#[inline]
pub fn add_scl3(res: &mut [f64; 3], vec1: &[f64; 3], vec2: &[f64; 3], scl: f64) {
    res[0] = vec1[0] + scl * vec2[0];
    res[1] = vec1[1] + scl * vec2[1];
    res[2] = vec1[2] + scl * vec2[2];
}

#[inline]
pub fn normalize3(vec: &mut [f64; 3]) -> f64 {
    let norm: f64 = (vec[0] * vec[0] + vec[1] * vec[1] + vec[2] * vec[2]).sqrt();

    if norm < crate::types::MINVAL {
        vec[0] = 1.0;
        vec[1] = 0.0;
        vec[2] = 0.0;
    } else {
        let norm_inv: f64 = 1.0 / norm;
        vec[0] *= norm_inv;
        vec[1] *= norm_inv;
        vec[2] *= norm_inv;
    }

    return norm;
}

#[inline]
pub fn norm3(vec: &[f64; 3]) -> f64 {
    return (vec[0] * vec[0] + vec[1] * vec[1] + vec[2] * vec[2]).sqrt();
}

#[inline]
pub fn dot3(vec1: &[f64; 3], vec2: &[f64; 3]) -> f64 {
    return vec1[0] * vec2[0] + vec1[1] * vec2[1] + vec1[2] * vec2[2];
}

#[inline]
pub fn dist3(pos1: &[f64; 3], pos2: &[f64; 3]) -> f64 {
    let dif: [f64; 3] = [pos1[0] - pos2[0], pos1[1] - pos2[1], pos1[2] - pos2[2]];
    return (dif[0] * dif[0] + dif[1] * dif[1] + dif[2] * dif[2]).sqrt();
}

#[inline]
pub fn mul_mat_vec3(res: &mut [f64; 3], mat: &[f64; 9], vec: &[f64; 3]) {
    let tmp: [f64; 3] = [
        mat[0] * vec[0] + mat[1] * vec[1] + mat[2] * vec[2],
        mat[3] * vec[0] + mat[4] * vec[1] + mat[5] * vec[2],
        mat[6] * vec[0] + mat[7] * vec[1] + mat[8] * vec[2],
    ];
    res[0] = tmp[0];
    res[1] = tmp[1];
    res[2] = tmp[2];
}

#[inline]
pub fn mul_mat_tvec3(res: &mut [f64; 3], mat: &[f64; 9], vec: &[f64; 3]) {
    let tmp: [f64; 3] = [
        mat[0] * vec[0] + mat[3] * vec[1] + mat[6] * vec[2],
        mat[1] * vec[0] + mat[4] * vec[1] + mat[7] * vec[2],
        mat[2] * vec[0] + mat[5] * vec[1] + mat[8] * vec[2],
    ];
    res[0] = tmp[0];
    res[1] = tmp[1];
    res[2] = tmp[2];
}

#[inline]
pub fn mul_mat_mat3(res: &mut [f64; 9], mat1: &[f64; 9], mat2: &[f64; 9]) {
    res[0] = mat1[0] * mat2[0] + mat1[1] * mat2[3] + mat1[2] * mat2[6];
    res[1] = mat1[0] * mat2[1] + mat1[1] * mat2[4] + mat1[2] * mat2[7];
    res[2] = mat1[0] * mat2[2] + mat1[1] * mat2[5] + mat1[2] * mat2[8];
    res[3] = mat1[3] * mat2[0] + mat1[4] * mat2[3] + mat1[5] * mat2[6];
    res[4] = mat1[3] * mat2[1] + mat1[4] * mat2[4] + mat1[5] * mat2[7];
    res[5] = mat1[3] * mat2[2] + mat1[4] * mat2[5] + mat1[5] * mat2[8];
    res[6] = mat1[6] * mat2[0] + mat1[7] * mat2[3] + mat1[8] * mat2[6];
    res[7] = mat1[6] * mat2[1] + mat1[7] * mat2[4] + mat1[8] * mat2[7];
    res[8] = mat1[6] * mat2[2] + mat1[7] * mat2[5] + mat1[8] * mat2[8];
}

#[inline]
pub fn mul_mat_tmat3(res: &mut [f64; 9], mat1: &[f64; 9], mat2: &[f64; 9]) {
    res[0] = mat1[0] * mat2[0] + mat1[3] * mat2[3] + mat1[6] * mat2[6];
    res[1] = mat1[0] * mat2[1] + mat1[3] * mat2[4] + mat1[6] * mat2[7];
    res[2] = mat1[0] * mat2[2] + mat1[3] * mat2[5] + mat1[6] * mat2[8];
    res[3] = mat1[1] * mat2[0] + mat1[4] * mat2[3] + mat1[7] * mat2[6];
    res[4] = mat1[1] * mat2[1] + mat1[4] * mat2[4] + mat1[7] * mat2[7];
    res[5] = mat1[1] * mat2[2] + mat1[4] * mat2[5] + mat1[7] * mat2[8];
    res[6] = mat1[2] * mat2[0] + mat1[5] * mat2[3] + mat1[8] * mat2[6];
    res[7] = mat1[2] * mat2[1] + mat1[5] * mat2[4] + mat1[8] * mat2[7];
    res[8] = mat1[2] * mat2[2] + mat1[5] * mat2[5] + mat1[8] * mat2[8];
}

#[inline]
pub fn mul_mat_mat_t3(res: &mut [f64; 9], mat1: &[f64; 9], mat2: &[f64; 9]) {
    res[0] = mat1[0] * mat2[0] + mat1[1] * mat2[1] + mat1[2] * mat2[2];
    res[1] = mat1[0] * mat2[3] + mat1[1] * mat2[4] + mat1[2] * mat2[5];
    res[2] = mat1[0] * mat2[6] + mat1[1] * mat2[7] + mat1[2] * mat2[8];
    res[3] = mat1[3] * mat2[0] + mat1[4] * mat2[1] + mat1[5] * mat2[2];
    res[4] = mat1[3] * mat2[3] + mat1[4] * mat2[4] + mat1[5] * mat2[5];
    res[5] = mat1[3] * mat2[6] + mat1[4] * mat2[7] + mat1[5] * mat2[8];
    res[6] = mat1[6] * mat2[0] + mat1[7] * mat2[1] + mat1[8] * mat2[2];
    res[7] = mat1[6] * mat2[3] + mat1[7] * mat2[4] + mat1[8] * mat2[5];
    res[8] = mat1[6] * mat2[6] + mat1[7] * mat2[7] + mat1[8] * mat2[8];
}

#[inline]
pub fn zero4(res: &mut [f64; 4]) {
    res[0] = 0.0;
    res[1] = 0.0;
    res[2] = 0.0;
    res[3] = 0.0;
}

#[inline]
pub fn unit4(res: &mut [f64; 4]) {
    res[0] = 1.0;
    res[1] = 0.0;
    res[2] = 0.0;
    res[3] = 0.0;
}

#[inline]
pub fn copy4(res: &mut [f64; 4], data: &[f64; 4]) {
    res[0] = data[0];
    res[1] = data[1];
    res[2] = data[2];
    res[3] = data[3];
}

#[inline]
pub fn normalize4(vec: &mut [f64; 4]) -> f64 {
    let norm: f64 =
        ((vec[0] * vec[0] + vec[1] * vec[1]) + (vec[2] * vec[2] + vec[3] * vec[3])).sqrt();

    if norm < crate::types::MINVAL {
        vec[0] = 1.0;
        vec[1] = 0.0;
        vec[2] = 0.0;
        vec[3] = 0.0;
    } else if (norm - 1.0).abs() > crate::types::MINVAL {
        let norm_inv: f64 = 1.0 / norm;
        vec[0] *= norm_inv;
        vec[1] *= norm_inv;
        vec[2] *= norm_inv;
        vec[3] *= norm_inv;
    }

    return norm;
}

#[inline]
pub fn zero(res: &mut [f64], n: usize) {
    res[..n].fill(0.0);
}

#[inline]
pub fn zero_ind(res: &mut [f64], n: usize, ind: &[i32]) {
    for i in 0..n {
        res[ind[i] as usize] = 0.0;
    }
}

#[inline]
pub fn fill(res: &mut [f64], val: f64, n: usize) {
    res[..n].fill(val);
}

#[inline]
pub fn copy(res: &mut [f64], vec: &[f64], n: usize) {
    res[..n].copy_from_slice(&vec[..n]);
}

#[inline]
pub fn copy_ind(res: &mut [f64], vec: &[f64], ind: &[i32], n: usize) {
    for i in 0..n {
        res[ind[i] as usize] = vec[ind[i] as usize];
    }
}

#[inline]
pub fn sum(vec: &[f64], n: usize) -> f64 {
    let mut res: f64 = 0.0;

    for &value in &vec[..n] {
        res += value;
    }

    return res;
}

#[inline]
pub fn l1(vec: &[f64], n: usize) -> f64 {
    let mut res: f64 = 0.0;

    for &value in &vec[..n] {
        res += value.abs();
    }

    return res;
}

#[inline]
pub fn scl(res: &mut [f64], vec: &[f64], scl: f64, n: usize) {
    let res = &mut res[..n];
    let vec = &vec[..n];
    for i in 0..n {
        res[i] = vec[i] * scl;
    }
}

#[inline]
pub fn add(res: &mut [f64], vec1: &[f64], vec2: &[f64], n: usize) {
    let res = &mut res[..n];
    let vec1 = &vec1[..n];
    let vec2 = &vec2[..n];
    for i in 0..n {
        res[i] = vec1[i] + vec2[i];
    }
}

#[inline]
pub fn add_ind(res: &mut [f64], vec1: &[f64], vec2: &[f64], ind: &[i32], n: usize) {
    for i in 0..n {
        let j = ind[i] as usize;
        res[j] = vec1[j] + vec2[j];
    }
}

#[inline]
pub fn sub(res: &mut [f64], vec1: &[f64], vec2: &[f64], n: usize) {
    let res = &mut res[..n];
    let vec1 = &vec1[..n];
    let vec2 = &vec2[..n];
    for i in 0..n {
        res[i] = vec1[i] - vec2[i];
    }
}

#[inline]
pub fn sub_ind(res: &mut [f64], vec1: &[f64], vec2: &[f64], ind: &[i32], n: usize) {
    for i in 0..n {
        let j = ind[i] as usize;
        res[j] = vec1[j] - vec2[j];
    }
}

#[inline]
pub fn add_to(res: &mut [f64], vec: &[f64], n: usize) {
    let res = &mut res[..n];
    let vec = &vec[..n];
    for i in 0..n {
        res[i] += vec[i];
    }
}

#[inline]
pub fn add_to_ind(res: &mut [f64], vec: &[f64], ind: &[i32], n: usize) {
    for i in 0..n {
        let j = ind[i] as usize;
        res[j] += vec[j];
    }
}

#[inline]
pub fn sub_from(res: &mut [f64], vec: &[f64], n: usize) {
    let res = &mut res[..n];
    let vec = &vec[..n];
    for i in 0..n {
        res[i] -= vec[i];
    }
}

#[inline]
pub fn add_to_scl(res: &mut [f64], vec: &[f64], scl: f64, n: usize) {
    let res = &mut res[..n];
    let vec = &vec[..n];
    for i in 0..n {
        res[i] += vec[i] * scl;
    }
}

#[inline]
pub fn add_to_scl_ind(res: &mut [f64], vec: &[f64], ind: &[i32], scl: f64, n: usize) {
    for i in 0..n {
        let k = ind[i] as usize;
        res[k] += vec[k] * scl;
    }
}

#[inline]
pub fn add_scl(res: &mut [f64], vec1: &[f64], vec2: &[f64], scl: f64, n: usize) {
    let res = &mut res[..n];
    let vec1 = &vec1[..n];
    let vec2 = &vec2[..n];
    for i in 0..n {
        res[i] = vec1[i] + vec2[i] * scl;
    }
}

#[inline]
pub fn normalize(res: &mut [f64], n: usize) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let norm = crate::util_blas::dot(res, res, n).sqrt();
    if norm < crate::types::MINVAL {
        res[0] = 1.0;
        crate::util_blas::zero(&mut res[1..], n - 1);
    } else {
        let norm_inv = 1.0 / norm;
        for value in &mut res[..n] {
            *value *= norm_inv;
        }
    }
    norm
}

#[inline]
pub fn norm(res: &[f64], n: usize) -> f64 {
    return (crate::util_blas::dot(res, res, n)).sqrt();
}

#[inline]
pub fn dot(vec1: &[f64], vec2: &[f64], n: usize) -> f64 {
    dot_lanes(vec1, vec2, n)
}

// The four-lane dot product. Like the reference's dot kernel it is always inlined, so the matrix-vector
// routines below take it inline instead of calling it once per row.
#[inline(always)]
fn dot_lanes(vec1: &[f64], vec2: &[f64], n: usize) -> f64 {
    let mut vec1 = &vec1[..n];
    let mut vec2 = &vec2[..n];
    let mut res0 = 0.0;
    let mut res1 = 0.0;
    let mut res2 = 0.0;
    let mut res3 = 0.0;
    // Check both four-lane windows before loading, preserving the AVX sums.
    while vec1.len() >= 4 && vec2.len() >= 4 {
        res0 += vec1[0] * vec2[0];
        res1 += vec1[1] * vec2[1];
        res2 += vec1[2] * vec2[2];
        res3 += vec1[3] * vec2[3];
        vec1 = &vec1[4..];
        vec2 = &vec2[4..];
    }
    let i = 0;
    let mut res = (res0 + res2) + (res1 + res3);
    let n_i = vec1.len();
    if n_i == 3 {
        res += vec1[i + 0] * vec2[i + 0] + vec1[i + 1] * vec2[i + 1] + vec1[i + 2] * vec2[i + 2];
    } else if n_i == 2 {
        res += vec1[i + 0] * vec2[i + 0] + vec1[i + 1] * vec2[i + 1];
    } else if n_i == 1 {
        res += vec1[i] * vec2[i];
    }
    res
}

pub fn dot_ind(vec1: &[f64], vec2: &[f64], ind: &[i32], n: usize) -> f64 {
    let mut res: f64 = 0.0;
    for i in 0..n {
        let k = ind[i] as usize;
        res += vec1[k] * vec2[k];
    }
    return res;
}

pub fn mul_mat_vec(res: &mut [f64], mat: &[f64], vec: &[f64], nr: usize, nc: usize) {
    for r in 0..nr {
        res[r] = dot_lanes(&mat[r * nc..], vec, nc);
    }
}

pub fn mul_mat_tvec(res: &mut [f64], mat: &[f64], vec: &[f64], nr: usize, nc: usize) {
    crate::util_blas::zero(res, nc);
    for r in 0..nr {
        let tmp = vec[r];
        if tmp != 0.0 {
            crate::util_blas::add_to_scl(res, &mat[r * nc..], tmp, nc);
        }
    }
}

pub fn mul_vec_mat_vec(vec1: &[f64], mat: &[f64], vec2: &[f64], n: usize) -> f64 {
    let mut res = 0.0;
    for i in 0..n {
        res += vec1[i] * dot_lanes(&mat[i * n..], vec2, n);
    }
    res
}

pub fn transpose(res: &mut [f64], mat: &[f64], nr: usize, nc: usize) {
    for i in 0..nr {
        for j in 0..nc {
            res[j * nr + i] = mat[i * nc + j];
        }
    }
}

pub fn symmetrize(res: &mut [f64], mat: &[f64], n: usize) {
    for i in 0..n {
        res[i * (n + 1)] = mat[i * (n + 1)];
        for j in 0..i {
            res[j * n + i] = 0.5 * (mat[i * n + j] + mat[j * n + i]);
            res[i * n + j] = res[j * n + i];
        }
    }
}

pub fn eye(mat: &mut [f64], n: usize) {
    crate::util_blas::zero(mat, n * n);
    for i in 0..n {
        mat[i * (n + 1)] = 1.0;
    }
}

pub fn copy_rows(res: &mut [f64], mat: &[f64], ind: &[i32], n: usize, nc: usize) {
    for i in 0..n {
        crate::util_blas::copy(
            &mut res[nc * ind[i] as usize..],
            &mat[nc * ind[i] as usize..],
            nc,
        );
    }
}

pub fn mul_mat_mat(res: &mut [f64], mat1: &[f64], mat2: &[f64], r1: usize, c1: usize, c2: usize) {
    crate::util_blas::zero(res, r1 * c2);
    for i in 0..r1 {
        for k in 0..c1 {
            let tmp = mat1[i * c1 + k];
            if tmp != 0.0 {
                crate::util_blas::add_to_scl(&mut res[i * c2..], &mat2[k * c2..], tmp, c2);
            }
        }
    }
}

pub fn mul_mat_mat_t(res: &mut [f64], mat1: &[f64], mat2: &[f64], r1: usize, c1: usize, r2: usize) {
    for i in 0..r1 {
        for j in 0..r2 {
            res[i * r2 + j] = dot_lanes(&mat1[i * c1..], &mat2[j * c1..], c1);
        }
    }
}

pub fn sqr_mat_td_impl(
    res: &mut [f64],
    mat: &[f64],
    diag: Option<&[f64]>,
    nr: usize,
    nc: usize,
    flg_upper: i32,
) {
    // Half of MatMat routine: only lower triangle.
    crate::util_blas::zero(res, nc * nc);
    for j in 0..nr {
        let dj = if let Some(diag) = diag { diag[j] } else { 1.0 };
        if dj != 0.0 {
            let mat_j = &mat[j * nc..];
            for i in 0..nc {
                let tmp = mat_j[i];
                if tmp != 0.0 {
                    crate::util_blas::add_to_scl(&mut res[i * nc..], mat_j, tmp * dj, i + 1);
                }
            }
        }
    }
    if flg_upper != 0 {
        for i in 0..nc {
            for j in i + 1..nc {
                res[i * nc + j] = res[j * nc + i];
            }
        }
    }
}

pub fn sqr_mat_td(res: &mut [f64], mat: &[f64], diag: Option<&[f64]>, nr: usize, nc: usize) {
    crate::util_blas::sqr_mat_td_impl(res, mat, diag, nr, nc, 1);
}

pub fn mul_mat_tmat(res: &mut [f64], mat1: &[f64], mat2: &[f64], r1: usize, c1: usize, c2: usize) {
    crate::util_blas::zero(res, c1 * c2);
    for i in 0..r1 {
        for j in 0..c1 {
            let tmp = mat1[i * c1 + j];
            if tmp != 0.0 {
                crate::util_blas::add_to_scl(&mut res[j * c2..], &mat2[i * c2..], tmp, c2);
            }
        }
    }
}
