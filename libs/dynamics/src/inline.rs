//! Inline vector, matrix and quaternion operations.
//!
//! Translated from `src/engine/engine_inline.h` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2025 DeepMind Technologies Limited);
//! changed: translated to Rust.

// Scalar branches only; these retain the C AVX reduction order.

#[inline]
pub fn zero3(res: &mut [f64; 3]) {
    res[0] = 0.0;
    res[1] = 0.0;
    res[2] = 0.0;
}

#[inline]
pub fn copy3(res: &mut [f64; 3], vec: &[f64; 3]) {
    res[0] = vec[0];
    res[1] = vec[1];
    res[2] = vec[2];
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
fn _normalize3(vec: &mut [f64; 3]) -> f64 {
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
pub fn mul_mat_vec3(res: &mut [f64; 3], mat: &[f64; 9], vec: &[f64; 3]) {
    res[0] = mat[0] * vec[0] + mat[1] * vec[1] + mat[2] * vec[2];
    res[1] = mat[3] * vec[0] + mat[4] * vec[1] + mat[5] * vec[2];
    res[2] = mat[6] * vec[0] + mat[7] * vec[1] + mat[8] * vec[2];
}

#[inline]
pub fn mul_mat_tvec3(res: &mut [f64; 3], mat: &[f64; 9], vec: &[f64; 3]) {
    res[0] = mat[0] * vec[0] + mat[3] * vec[1] + mat[6] * vec[2];
    res[1] = mat[1] * vec[0] + mat[4] * vec[1] + mat[7] * vec[2];
    res[2] = mat[2] * vec[0] + mat[5] * vec[1] + mat[8] * vec[2];
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
pub fn transpose3(res: &mut [f64; 9], mat: &[f64; 9]) {
    res[0] = mat[0];
    res[1] = mat[3];
    res[2] = mat[6];
    res[3] = mat[1];
    res[4] = mat[4];
    res[5] = mat[7];
    res[6] = mat[2];
    res[7] = mat[5];
    res[8] = mat[8];
}

#[inline]
pub fn copy4(res: &mut [f64; 4], data: &[f64; 4]) {
    res[0] = data[0];
    res[1] = data[1];
    res[2] = data[2];
    res[3] = data[3];
}

#[inline]
fn _normalize4(vec: &mut [f64; 4]) -> f64 {
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
pub fn rot_vec_quat(res: &mut [f64; 3], vec: &[f64; 3], quat: &[f64; 4]) {
    // null quat: copy vec
    if quat[0] == 1.0 && quat[1] == 0.0 && quat[2] == 0.0 && quat[3] == 0.0 {
        crate::inline::copy3(res, vec);
    }
    // regular processing
    else {
        let mut tmp: [f64; 3] = [0.0; 3];
        // tmp = q_w * v + cross(q_xyz, v)
        tmp[0] = quat[0] * vec[0] + quat[2] * vec[2] - quat[3] * vec[1];
        tmp[1] = quat[0] * vec[1] + quat[3] * vec[0] - quat[1] * vec[2];
        tmp[2] = quat[0] * vec[2] + quat[1] * vec[1] - quat[2] * vec[0];
        // res = v + 2 * cross(q_xyz, t)
        res[0] = vec[0] + 2.0 * (quat[2] * tmp[2] - quat[3] * tmp[1]);
        res[1] = vec[1] + 2.0 * (quat[3] * tmp[0] - quat[1] * tmp[2]);
        res[2] = vec[2] + 2.0 * (quat[1] * tmp[1] - quat[2] * tmp[0]);
    }
}

#[inline]
pub fn neg_quat(res: &mut [f64; 4], quat: &[f64; 4]) {
    res[0] = quat[0];
    res[1] = -quat[1];
    res[2] = -quat[2];
    res[3] = -quat[3];
}

#[inline]
pub fn mul_quat(res: &mut [f64; 4], qa: &[f64; 4], qb: &[f64; 4]) {
    res[0] = (qa[0] * qb[0] - qa[1] * qb[1]) + (-qa[2] * qb[2] - qa[3] * qb[3]);
    res[1] = (qa[0] * qb[1] + qa[1] * qb[0]) + (qa[2] * qb[3] - qa[3] * qb[2]);
    res[2] = (qa[0] * qb[2] - qa[1] * qb[3]) + (qa[2] * qb[0] + qa[3] * qb[1]);
    res[3] = (qa[0] * qb[3] + qa[1] * qb[2]) + (-qa[2] * qb[1] + qa[3] * qb[0]);
}

#[inline]
pub fn mul_quat_axis(res: &mut [f64; 4], quat: &[f64; 4], axis: &[f64; 3]) {
    res[0] = -quat[1] * axis[0] - quat[2] * axis[1] - quat[3] * axis[2];
    res[1] = quat[0] * axis[0] - quat[3] * axis[1] + quat[2] * axis[2];
    res[2] = quat[3] * axis[0] + quat[0] * axis[1] - quat[1] * axis[2];
    res[3] = -quat[2] * axis[0] + quat[1] * axis[1] + quat[0] * axis[2];
}

#[inline]
pub fn axis_angle2quat(res: &mut [f64; 4], axis: &[f64; 3], angle: f64) {
    // zero angle: null quat
    if angle == 0.0 {
        res[0] = 1.0;
        res[1] = 0.0;
        res[2] = 0.0;
        res[3] = 0.0;
    }
    // regular processing
    else {
        let s: f64 = (angle * 0.5).sin();
        res[0] = (angle * 0.5).cos();
        res[1] = axis[0] * s;
        res[2] = axis[1] * s;
        res[3] = axis[2] * s;
    }
}

#[inline]
pub fn quat2vel(res: &mut [f64; 3], quat: &[f64; 4], dt: f64) {
    let mut axis: [f64; 3] = [quat[1], quat[2], quat[3]];
    let sin_a_2: f64 = crate::inline::_normalize3(&mut axis);
    let mut speed: f64 = 2.0 * (sin_a_2).atan2(quat[0]);

    // when axis-angle is larger than pi, rotation is in the opposite direction
    if speed > crate::types::PI {
        speed -= 2.0 * crate::types::PI;
    }
    speed /= dt;

    crate::inline::scl3(res, &axis, speed);
}

#[inline]
pub fn sub_quat(res: &mut [f64; 3], qa: &[f64; 4], qb: &[f64; 4]) {
    // qdif = neg(qb)*qa
    let mut qneg: [f64; 4] = [0.0; 4];
    let mut qdif: [f64; 4] = [0.0; 4];
    crate::inline::neg_quat(&mut qneg, qb);
    crate::inline::mul_quat(&mut qdif, &qneg, qa);

    // convert to 3D velocity
    crate::inline::quat2vel(res, &qdif, 1.0);
}

#[inline]
pub fn quat2mat(res: &mut [f64; 9], quat: &[f64; 4]) {
    // null quat: identity
    if quat[0] == 1.0 && quat[1] == 0.0 && quat[2] == 0.0 && quat[3] == 0.0 {
        res[0] = 1.0;
        res[1] = 0.0;
        res[2] = 0.0;
        res[3] = 0.0;
        res[4] = 1.0;
        res[5] = 0.0;
        res[6] = 0.0;
        res[7] = 0.0;
        res[8] = 1.0;
    }
    // regular processing
    else {
        let q00: f64 = quat[0] * quat[0];
        let q01: f64 = quat[0] * quat[1];
        let q02: f64 = quat[0] * quat[2];
        let q03: f64 = quat[0] * quat[3];
        let q11: f64 = quat[1] * quat[1];
        let q12: f64 = quat[1] * quat[2];
        let q13: f64 = quat[1] * quat[3];
        let q22: f64 = quat[2] * quat[2];
        let q23: f64 = quat[2] * quat[3];
        let q33: f64 = quat[3] * quat[3];

        res[0] = (q00 - q22) + (q11 - q33);
        res[4] = (q00 + q22) - (q11 + q33);
        res[8] = (q00 - q22) - (q11 - q33);

        res[1] = 2.0 * (q12 - q03);
        res[2] = 2.0 * (q13 + q02);
        res[3] = 2.0 * (q12 + q03);
        res[5] = 2.0 * (q23 - q01);
        res[6] = 2.0 * (q13 - q02);
        res[7] = 2.0 * (q23 + q01);
    }
}

#[inline]
pub fn mat2quat(quat: &mut [f64; 4], mat: &[f64; 9]) {
    // q0 largest
    if mat[0] + mat[4] + mat[8] > 0.0 {
        quat[0] = 0.5 * (1.0 + mat[0] + mat[4] + mat[8]).sqrt();
        quat[1] = 0.25 * (mat[7] - mat[5]) / quat[0];
        quat[2] = 0.25 * (mat[2] - mat[6]) / quat[0];
        quat[3] = 0.25 * (mat[3] - mat[1]) / quat[0];
    }
    // q1 largest
    else if mat[0] > mat[4] && mat[0] > mat[8] {
        quat[1] = 0.5 * (1.0 + mat[0] - mat[4] - mat[8]).sqrt();
        quat[0] = 0.25 * (mat[7] - mat[5]) / quat[1];
        quat[2] = 0.25 * (mat[1] + mat[3]) / quat[1];
        quat[3] = 0.25 * (mat[2] + mat[6]) / quat[1];
    }
    // q2 largest
    else if mat[4] > mat[8] {
        quat[2] = 0.5 * (1.0 - mat[0] + mat[4] - mat[8]).sqrt();
        quat[0] = 0.25 * (mat[2] - mat[6]) / quat[2];
        quat[1] = 0.25 * (mat[1] + mat[3]) / quat[2];
        quat[3] = 0.25 * (mat[5] + mat[7]) / quat[2];
    }
    // q3 largest
    else {
        quat[3] = 0.5 * (1.0 - mat[0] - mat[4] + mat[8]).sqrt();
        quat[0] = 0.25 * (mat[3] - mat[1]) / quat[3];
        quat[1] = 0.25 * (mat[2] + mat[6]) / quat[3];
        quat[2] = 0.25 * (mat[5] + mat[7]) / quat[3];
    }

    crate::inline::_normalize4(quat);
}

#[inline]
pub fn quat_integrate(quat: &mut [f64; 4], vel: &[f64; 3], scale: f64) {
    let mut tmp = [0.0; 4];
    let mut qrot = [0.0; 4];
    crate::inline::copy3(crate::inline::at3_mut(&mut tmp, 0), vel);
    let angle = scale * crate::inline::_normalize3(crate::inline::at3_mut(&mut tmp, 0));
    crate::inline::axis_angle2quat(&mut qrot, crate::inline::at3(&tmp, 0), angle);
    crate::inline::_normalize4(quat);
    crate::inline::copy4(&mut tmp, quat);
    crate::inline::mul_quat(quat, &tmp, &qrot);
}

#[inline]
pub fn cross(res: &mut [f64; 3], a: &[f64; 3], b: &[f64; 3]) {
    res[0] = a[1] * b[2] - a[2] * b[1];
    res[1] = a[2] * b[0] - a[0] * b[2];
    res[2] = a[0] * b[1] - a[1] * b[0];
}

#[inline]
pub fn cross_motion(res: &mut [f64; 6], vel: &[f64; 6], v: &[f64; 6]) {
    res[0] = -vel[2] * v[1] + vel[1] * v[2];
    res[1] = vel[2] * v[0] - vel[0] * v[2];
    res[2] = -vel[1] * v[0] + vel[0] * v[1];
    res[3] = -vel[2] * v[4] + vel[1] * v[5];
    res[4] = vel[2] * v[3] - vel[0] * v[5];
    res[5] = -vel[1] * v[3] + vel[0] * v[4];

    res[3] += -vel[5] * v[1] + vel[4] * v[2];
    res[4] += vel[5] * v[0] - vel[3] * v[2];
    res[5] += -vel[4] * v[0] + vel[3] * v[1];
}

#[inline]
pub fn cross_force(res: &mut [f64; 6], vel: &[f64; 6], f: &[f64; 6]) {
    res[0] = -vel[2] * f[1] + vel[1] * f[2];
    res[1] = vel[2] * f[0] - vel[0] * f[2];
    res[2] = -vel[1] * f[0] + vel[0] * f[1];
    res[3] = -vel[2] * f[4] + vel[1] * f[5];
    res[4] = vel[2] * f[3] - vel[0] * f[5];
    res[5] = -vel[1] * f[3] + vel[0] * f[4];

    res[0] += -vel[5] * f[4] + vel[4] * f[5];
    res[1] += vel[5] * f[3] - vel[3] * f[5];
    res[2] += -vel[4] * f[3] + vel[3] * f[4];
}

#[inline]
pub fn dot6(vec1: &[f64; 6], vec2: &[f64; 6]) -> f64 {
    // match order of operations to mju_dot
    return ((vec1[0] * vec2[0] + vec1[2] * vec2[2]) + (vec1[1] * vec2[1] + vec1[3] * vec2[3]))
        + (vec1[4] * vec2[4] + vec1[5] * vec2[5]);
}

#[inline]
pub fn copy6(res: &mut [f64; 6], vec: &[f64; 6]) {
    res[0] = vec[0];
    res[1] = vec[1];
    res[2] = vec[2];
    res[3] = vec[3];
    res[4] = vec[4];
    res[5] = vec[5];
}

#[inline]
pub fn copy9(res: &mut [f64; 9], data: &[f64; 9]) {
    res[0] = data[0];
    res[1] = data[1];
    res[2] = data[2];
    res[3] = data[3];
    res[4] = data[4];
    res[5] = data[5];
    res[6] = data[6];
    res[7] = data[7];
    res[8] = data[8];
}

#[inline]
pub fn at3(slice: &[f64], off: usize) -> &[f64; 3] {
    slice[off..off + 3].try_into().unwrap()
}

#[inline]
pub fn at3_mut(slice: &mut [f64], off: usize) -> &mut [f64; 3] {
    (&mut slice[off..off + 3]).try_into().unwrap()
}

#[inline]
pub fn at4(slice: &[f64], off: usize) -> &[f64; 4] {
    slice[off..off + 4].try_into().unwrap()
}

#[inline]
pub fn at4_mut(slice: &mut [f64], off: usize) -> &mut [f64; 4] {
    (&mut slice[off..off + 4]).try_into().unwrap()
}

#[inline]
pub fn at6(slice: &[f64], off: usize) -> &[f64; 6] {
    slice[off..off + 6].try_into().unwrap()
}

#[inline]
pub fn at6_mut(slice: &mut [f64], off: usize) -> &mut [f64; 6] {
    (&mut slice[off..off + 6]).try_into().unwrap()
}

#[inline]
pub fn at9(slice: &[f64], off: usize) -> &[f64; 9] {
    slice[off..off + 9].try_into().unwrap()
}

#[inline]
pub fn at9_mut(slice: &mut [f64], off: usize) -> &mut [f64; 9] {
    (&mut slice[off..off + 9]).try_into().unwrap()
}

#[inline]
pub fn at10(slice: &[f64], off: usize) -> &[f64; 10] {
    slice[off..off + 10].try_into().unwrap()
}

#[inline]
pub fn at10_mut(slice: &mut [f64], off: usize) -> &mut [f64; 10] {
    (&mut slice[off..off + 10]).try_into().unwrap()
}

#[inline]
pub fn at36(slice: &[f64], off: usize) -> &[f64; 36] {
    slice[off..off + 36].try_into().unwrap()
}

#[inline]
pub fn at36_mut(slice: &mut [f64], off: usize) -> &mut [f64; 36] {
    (&mut slice[off..off + 36]).try_into().unwrap()
}
