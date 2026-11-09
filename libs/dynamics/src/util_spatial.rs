//! Quaternion, pose and spatial algebra operations.
//!
//! Translated from `src/engine/engine_util_spatial.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

// Scalar branches only; these retain the C AVX reduction order.

#[inline]
pub fn rot_vec_quat(res: &mut [f64; 3], vec: &[f64; 3], quat: &[f64; 4]) {
    // zero vec: zero res
    if vec[0] == 0.0 && vec[1] == 0.0 && vec[2] == 0.0 {
        crate::util_blas::zero3(res);
    }
    // null quat: copy vec
    else if quat[0] == 1.0 && quat[1] == 0.0 && quat[2] == 0.0 && quat[3] == 0.0 {
        crate::inline::copy3(res, vec);
    }
    // regular processing
    else {
        // tmp = q_w * v + cross(q_xyz, v)
        let tmp: [f64; 3] = [
            quat[0] * vec[0] + quat[2] * vec[2] - quat[3] * vec[1],
            quat[0] * vec[1] + quat[3] * vec[0] - quat[1] * vec[2],
            quat[0] * vec[2] + quat[1] * vec[1] - quat[2] * vec[0],
        ];

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
    let tmp: [f64; 4] = [
        (qa[0] * qb[0] - qa[1] * qb[1]) + (-qa[2] * qb[2] - qa[3] * qb[3]),
        (qa[0] * qb[1] + qa[1] * qb[0]) + (qa[2] * qb[3] - qa[3] * qb[2]),
        (qa[0] * qb[2] - qa[1] * qb[3]) + (qa[2] * qb[0] + qa[3] * qb[1]),
        (qa[0] * qb[3] + qa[1] * qb[2]) + (-qa[2] * qb[1] + qa[3] * qb[0]),
    ];
    res[0] = tmp[0];
    res[1] = tmp[1];
    res[2] = tmp[2];
    res[3] = tmp[3];
}

#[inline]
pub fn mul_quat_axis(res: &mut [f64; 4], quat: &[f64; 4], axis: &[f64; 3]) {
    let tmp: [f64; 4] = [
        -quat[1] * axis[0] - quat[2] * axis[1] - quat[3] * axis[2],
        quat[0] * axis[0] - quat[3] * axis[1] + quat[2] * axis[2],
        quat[3] * axis[0] + quat[0] * axis[1] - quat[1] * axis[2],
        -quat[2] * axis[0] + quat[1] * axis[1] + quat[0] * axis[2],
    ];
    res[0] = tmp[0];
    res[1] = tmp[1];
    res[2] = tmp[2];
    res[3] = tmp[3];
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
    let sin_a_2: f64 = crate::util_blas::normalize3(&mut axis);
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
    // Rust callers copy an aliased input first, as in the C res == quat branch.
    crate::inline::quat2mat(res, quat);
}

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

    crate::util_blas::normalize4(quat);
}

pub fn deriv_quat(res: &mut [f64; 4], quat: &[f64; 4], vel: &[f64; 3]) {
    res[0] = 0.5 * (-vel[0] * quat[1] - vel[1] * quat[2] - vel[2] * quat[3]);
    res[1] = 0.5 * (vel[0] * quat[0] + vel[1] * quat[3] - vel[2] * quat[2]);
    res[2] = 0.5 * (-vel[0] * quat[3] + vel[1] * quat[0] + vel[2] * quat[1]);
    res[3] = 0.5 * (vel[0] * quat[2] - vel[1] * quat[1] + vel[2] * quat[0]);
}

fn bortz_coef(xx: f64) -> f64 {
    const K: [f64; 9] = [
        0.083333333333333384,
        0.0013888888888859755,
        3.3068783094433201e-05,
        8.2671949071520734e-07,
        2.0876900642039142e-08,
        5.2828494303844588e-10,
        1.3455485951057888e-11,
        3.1615515500778219e-13,
        1.230313397310169e-14,
    ];
    if xx < 1.75 * 1.75 {
        let mut res = K[8];
        for i in (0..=7).rev() {
            res = K[i] + xx * res;
        }
        return res;
    }
    let x = xx.sqrt();
    (1.0 - 0.5 * x / (0.5 * x).tan()) / xx
}

pub fn deriv_expmap(res: &mut [f64; 3], expmap: &[f64; 3], vel: &[f64; 3]) {
    let mut r: [f64; 3] = [expmap[0], expmap[1], expmap[2]];
    let mut xx: f64 = crate::util_blas::dot3(&r, &r);

    // reduce the angle to (-pi, pi], like the SO3 setpoint re-anchoring in mj_advance
    if xx > crate::types::PI * crate::types::PI {
        let x: f64 = (xx).sqrt();
        let scale: f64 =
            (x - 2.0 * crate::types::PI * (x / (2.0 * crate::types::PI) + 0.5).floor()) / x;
        for k in 0..3 {
            r[k] *= scale;
        }
        xx = crate::util_blas::dot3(&r, &r);
    }

    let c: f64 = crate::util_spatial::bortz_coef(xx);

    let mut rxv: [f64; 3] = [0.0; 3];
    let mut rxrxv: [f64; 3] = [0.0; 3];
    crate::inline::cross(&mut rxv, &r, vel);
    crate::inline::cross(&mut rxrxv, &r, &rxv);
    for k in 0..3 {
        res[k] = vel[k] + 0.5 * rxv[k] + c * rxrxv[k];
    }
}

#[inline]
pub fn quat_integrate(quat: &mut [f64; 4], vel: &[f64; 3], scale: f64) {
    let mut tmp = [0.0; 4];
    let mut qrot = [0.0; 4];
    crate::inline::copy3(crate::inline::at3_mut(&mut tmp, 0), vel);
    let angle = scale * crate::util_blas::normalize3(crate::inline::at3_mut(&mut tmp, 0));
    crate::inline::axis_angle2quat(&mut qrot, crate::inline::at3(&tmp, 0), angle);
    crate::util_blas::normalize4(quat);
    let old_quat = *quat;
    crate::util_spatial::mul_quat(quat, &old_quat, &qrot);
}

pub fn quat_z2vec(quat: &mut [f64; 4], vec: &[f64; 3]) {
    let mut axis = [0.0; 3];
    let mut vn = [vec[0], vec[1], vec[2]];
    let z = [0.0, 0.0, 1.0];
    quat[0] = 1.0;
    crate::util_blas::zero3(crate::inline::at3_mut(quat, 1));
    if crate::util_blas::normalize3(&mut vn) < crate::types::MINVAL {
        return;
    }
    crate::inline::cross(&mut axis, &z, &vn);
    let mut a = crate::util_blas::normalize3(&mut axis);
    if a.abs() < crate::types::MINVAL {
        if crate::util_blas::dot3(&vn, &z) < 0.0 {
            quat[0] = 0.0;
            quat[1] = 1.0;
        }
        return;
    }
    a = a.atan2(crate::util_blas::dot3(&vn, &z));
    crate::inline::axis_angle2quat(quat, &axis, a);
}

const EIG_TOL: f64 = 4e-15;
const EIG_EPS: f64 = 1e-12;

pub fn eig3(
    eigval: &mut [f64; 3],
    eigvec: &mut [f64; 9],
    quat: &mut [f64; 4],
    mat: &[f64; 9],
) -> usize {
    crate::util_spatial::eig3tol(eigval, eigvec, quat, mat, EIG_TOL)
}

pub fn eig3tol(
    eigval: &mut [f64; 3],
    eigvec: &mut [f64; 9],
    quat: &mut [f64; 4],
    mat: &[f64; 9],
    reltol: f64,
) -> usize {
    let mut d = [0.0; 9];
    let mut tmp = [0.0; 9];
    let mut scale = 0.0;
    for i in 0..9 {
        let abs_elem = mat[i].abs();
        if abs_elem > scale {
            scale = abs_elem;
        }
    }
    let eff_reltol = if reltol > EIG_TOL { reltol } else { EIG_TOL };
    let tol = scale * eff_reltol;
    quat[0] = 1.0;
    quat[3] = 0.0;
    quat[2] = quat[3];
    quat[1] = quat[2];
    let mut iter = 0;
    while iter < 500 {
        crate::util_spatial::quat2mat(eigvec, quat);
        crate::inline::mul_mat_tmat3(&mut tmp, eigvec, mat);
        crate::inline::mul_mat_mat3(&mut d, &tmp, eigvec);
        eigval[0] = d[0];
        eigval[1] = d[4];
        eigval[2] = d[8];
        let (rk, ck, rotk);
        if d[1].abs() > d[2].abs() && d[1].abs() > d[5].abs() {
            rk = 0;
            ck = 1;
            rotk = 2;
        } else if d[2].abs() > d[5].abs() {
            rk = 0;
            ck = 2;
            rotk = 1;
        } else {
            rk = 1;
            ck = 2;
            rotk = 0;
        }
        if d[3 * rk + ck].abs() <= tol {
            break;
        }
        let tau = (d[4 * ck] - d[4 * rk]) / (2.0 * d[3 * rk + ck]);
        let t;
        if tau >= 0.0 {
            t = 1.0 / (tau + (1.0 + tau * tau).sqrt());
        } else {
            t = -1.0 / (-tau + (1.0 + tau * tau).sqrt());
        }
        let h = t / (1.0 + (1.0 + t * t).sqrt());
        tmp[0] = 1.0 / (1.0 + h * h).sqrt();
        tmp[3] = 0.0;
        tmp[2] = tmp[3];
        tmp[1] = tmp[2];
        tmp[rotk + 1] = (if rotk == 1 { h } else { -h }) * tmp[0];
        let old_quat = *quat;
        crate::util_spatial::mul_quat(quat, &old_quat, crate::inline::at4(&tmp, 0));
        crate::util_blas::normalize4(quat);
        iter += 1;
    }
    let eps = scale * EIG_EPS;
    for j in 0..3 {
        let j1 = j % 2;
        if eigval[j1] + eps < eigval[j1 + 1] {
            let t = eigval[j1];
            eigval[j1] = eigval[j1 + 1];
            eigval[j1 + 1] = t;
            tmp[0] = 0.707106781186548;
            tmp[3] = 0.0;
            tmp[2] = tmp[3];
            tmp[1] = tmp[2];
            tmp[(j1 + 2) % 3 + 1] = tmp[0];
            let old_quat = *quat;
            crate::util_spatial::mul_quat(quat, &old_quat, crate::inline::at4(&tmp, 0));
            crate::util_blas::normalize4(quat);
        }
    }
    crate::util_spatial::quat2mat(eigvec, quat);
    iter
}

const ROT_EPS: f64 = 1e-9;

pub fn mat2rot(quat: &mut [f64; 4], mat: &[f64; 9]) -> usize {
    // Müller et al., A robust method to extract the rotational part of deformations (2016).
    crate::util_blas::normalize4(quat);
    let col1_mat = [mat[0], mat[3], mat[6]];
    let col2_mat = [mat[1], mat[4], mat[7]];
    let col3_mat = [mat[2], mat[5], mat[8]];
    let mut iter = 0;
    while iter < 500 {
        let mut rot = [0.0; 9];
        crate::util_spatial::quat2mat(&mut rot, quat);
        let col1_rot = [rot[0], rot[3], rot[6]];
        let col2_rot = [rot[1], rot[4], rot[7]];
        let col3_rot = [rot[2], rot[5], rot[8]];
        let mut omega = [0.0; 3];
        let mut vec1 = [0.0; 3];
        let mut vec2 = [0.0; 3];
        let mut vec3 = [0.0; 3];
        crate::inline::cross(&mut vec1, &col1_rot, &col1_mat);
        crate::inline::cross(&mut vec2, &col2_rot, &col2_mat);
        crate::inline::cross(&mut vec3, &col3_rot, &col3_mat);
        crate::inline::add3(&mut omega, &vec1, &vec2);
        crate::inline::add_to3(&mut omega, &vec3);
        let dot = [
            crate::util_blas::dot3(&col1_rot, &col1_mat),
            crate::util_blas::dot3(&col2_rot, &col2_mat),
            crate::util_blas::dot3(&col3_rot, &col3_mat),
        ];
        let trace = dot[0] + dot[1] + dot[2];
        let old_omega = omega;
        crate::util_blas::scl3(
            &mut omega,
            &old_omega,
            1.0 / (trace.abs() + crate::types::MINVAL),
        );
        let w = crate::util_blas::normalize3(&mut omega);
        if w < ROT_EPS {
            let mut a = [0.0; 9];
            let mut sym = [0.0; 9];
            crate::inline::mul_mat_tmat3(&mut a, &rot, mat);
            sym[0] = a[0];
            sym[1] = 0.5 * (a[1] + a[3]);
            sym[2] = 0.5 * (a[2] + a[6]);
            sym[3] = sym[1];
            sym[4] = a[4];
            sym[5] = 0.5 * (a[5] + a[7]);
            sym[6] = sym[2];
            sym[7] = sym[5];
            sym[8] = a[8];
            // T = tr(S)*I - S is positive definite iff this rotation maximizes tr(R'*M).
            let t00 = sym[4] + sym[8];
            let t11 = sym[0] + sym[8];
            let t22 = sym[0] + sym[4];
            let det_t = t00 * (t11 * t22 - sym[5] * sym[5])
                - sym[1] * sym[1] * t22
                - sym[2] * sym[2] * t11
                - 2.0 * sym[1] * sym[2] * sym[5];
            let scale = sym[0].abs() + sym[4].abs() + sym[8].abs() + crate::types::MINVAL;
            let tol = ROT_EPS * scale;
            let tol2 = tol * scale;
            let tol3 = tol2 * scale;
            if t00 > tol && (t00 * t11 - sym[1] * sym[1]) > tol2 && det_t > tol3 {
                break;
            }
            let mut eigval = [0.0; 3];
            let mut eigvec = [0.0; 9];
            let mut qtmp = [0.0; 4];
            crate::util_spatial::eig3(&mut eigval, &mut eigvec, &mut qtmp, &sym);
            if eigval[0] <= trace + ROT_EPS * (eigval[0].abs() + trace.abs() + crate::types::MINVAL)
            {
                break;
            }
            let v_max = [eigvec[0], eigvec[3], eigvec[6]];
            let mut axis = [0.0; 3];
            crate::inline::mul_mat_vec3(&mut axis, &rot, &v_max);
            let qflip = [0.0, axis[0], axis[1], axis[2]];
            let old_quat = *quat;
            crate::util_spatial::mul_quat(quat, &qflip, &old_quat);
            crate::util_blas::normalize4(quat);
            iter += 1;
            continue;
        }
        let mut qrot = [0.0; 4];
        crate::inline::axis_angle2quat(&mut qrot, &omega, w);
        let old_quat = *quat;
        crate::util_spatial::mul_quat(quat, &qrot, &old_quat);
        crate::util_blas::normalize4(quat);
        iter += 1;
    }
    iter
}

pub fn mul_pose(
    posres: &mut [f64; 3],
    quatres: &mut [f64; 4],
    pos1: &[f64; 3],
    quat1: &[f64; 4],
    pos2: &[f64; 3],
    quat2: &[f64; 4],
) {
    let mut ptmp: [f64; 3] = [0.0; 3];
    let mut qtmp: [f64; 4] = [0.0; 4];

    // ptmp = quat1*pos2 + pos1
    crate::inline::rot_vec_quat(&mut ptmp, pos2, quat1);
    crate::inline::add_to3(&mut ptmp, pos1);

    // qtmp = quat1*quat2
    crate::inline::mul_quat(&mut qtmp, quat1, quat2);
    crate::util_blas::normalize4(&mut qtmp);

    crate::inline::copy3(posres, &ptmp);
    crate::inline::copy4(quatres, &qtmp);
}

pub fn neg_pose(posres: &mut [f64; 3], quatres: &mut [f64; 4], pos: &[f64; 3], quat: &[f64; 4]) {
    let mut ptmp: [f64; 3] = [0.0; 3];
    let mut qtmp: [f64; 4] = [0.0; 4];

    // qtmp = neg(quat)
    crate::inline::neg_quat(&mut qtmp, quat);

    // posres = -neg(quat)*pos
    crate::inline::rot_vec_quat(&mut ptmp, pos, &qtmp);
    crate::inline::scl3(posres, &ptmp, -1.0);

    crate::inline::copy4(quatres, &qtmp);
}

pub fn trn_vec_pose(res: &mut [f64; 3], pos: &[f64; 3], quat: &[f64; 4], vec: &[f64; 3]) {
    let mut tmp: [f64; 3] = [0.0; 3];

    // res = quat*vec + pos
    crate::inline::rot_vec_quat(&mut tmp, vec, quat);
    crate::inline::add_to3(&mut tmp, pos);
    crate::inline::copy3(res, &tmp);
}

#[inline]
pub fn cross(res: &mut [f64; 3], a: &[f64; 3], b: &[f64; 3]) {
    let tmp: [f64; 3] = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    res[0] = tmp[0];
    res[1] = tmp[1];
    res[2] = tmp[2];
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
pub fn inert_com(res: &mut [f64; 10], inert: &[f64; 3], mat: &[f64; 9], dif: &[f64; 3], mass: f64) {
    // tmp = diag(inert) * mat'  (mat is local-to-global rotation)
    let tmp: [f64; 9] = [
        mat[0] * inert[0],
        mat[3] * inert[0],
        mat[6] * inert[0],
        mat[1] * inert[1],
        mat[4] * inert[1],
        mat[7] * inert[1],
        mat[2] * inert[2],
        mat[5] * inert[2],
        mat[8] * inert[2],
    ];

    // res_rot = mat * diag(inert) * mat'
    res[0] = mat[0] * tmp[0] + mat[1] * tmp[3] + mat[2] * tmp[6];
    res[1] = mat[3] * tmp[1] + mat[4] * tmp[4] + mat[5] * tmp[7];
    res[2] = mat[6] * tmp[2] + mat[7] * tmp[5] + mat[8] * tmp[8];
    res[3] = mat[0] * tmp[1] + mat[1] * tmp[4] + mat[2] * tmp[7];
    res[4] = mat[0] * tmp[2] + mat[1] * tmp[5] + mat[2] * tmp[8];
    res[5] = mat[3] * tmp[2] + mat[4] * tmp[5] + mat[5] * tmp[8];

    // res_rot -= mass * dif_cross * dif_cross
    res[0] += mass * (dif[1] * dif[1] + dif[2] * dif[2]);
    res[1] += mass * (dif[0] * dif[0] + dif[2] * dif[2]);
    res[2] += mass * (dif[0] * dif[0] + dif[1] * dif[1]);
    res[3] -= mass * dif[0] * dif[1];
    res[4] -= mass * dif[0] * dif[2];
    res[5] -= mass * dif[1] * dif[2];

    // res_tran = mass * dif
    res[6] = mass * dif[0];
    res[7] = mass * dif[1];
    res[8] = mass * dif[2];

    // res_mass = mass
    res[9] = mass;
}

#[inline]
pub fn mul_inert_vec(res: &mut [f64; 6], i: &[f64; 10], v: &[f64; 6]) {
    res[0] = i[0] * v[0] + i[3] * v[1] + i[4] * v[2] - i[8] * v[4] + i[7] * v[5];
    res[1] = i[3] * v[0] + i[1] * v[1] + i[5] * v[2] + i[8] * v[3] - i[6] * v[5];
    res[2] = i[4] * v[0] + i[5] * v[1] + i[2] * v[2] - i[7] * v[3] + i[6] * v[4];
    res[3] = i[8] * v[1] - i[7] * v[2] + i[9] * v[3];
    res[4] = i[6] * v[2] - i[8] * v[0] + i[9] * v[4];
    res[5] = i[7] * v[0] - i[6] * v[1] + i[9] * v[5];
}

#[inline]
pub fn dof_com(res: &mut [f64; 6], axis: &[f64; 3], offset: Option<&[f64; 3]>) {
    if let Some(offset) = offset {
        crate::inline::copy3(crate::inline::at3_mut(res, 0), axis);
        crate::inline::cross(crate::inline::at3_mut(res, 3), axis, offset);
    } else {
        crate::util_blas::zero3(crate::inline::at3_mut(res, 0));
        crate::inline::copy3(crate::inline::at3_mut(res, 3), axis);
    }
}

#[inline]
pub fn mul_dof_vec(res: &mut [f64; 6], dof: &[f64], vec: &[f64], n: usize) {
    if n == 1 {
        crate::util_blas::scl(res, dof, vec[0], 6);
    } else if n == 0 {
        crate::util_blas::zero(res, 6);
    } else {
        crate::util_blas::mul_mat_tvec(res, dof, vec, n, 6);
    }
}

pub fn transform_spatial(
    res: &mut [f64; 6],
    vec: &[f64; 6],
    flg_force: i32,
    newpos: &[f64; 3],
    oldpos: &[f64; 3],
    rotnew2old: Option<&[f64; 9]>,
) {
    let mut cros = [0.0; 3];
    let mut dif = [0.0; 3];
    let mut tran = [0.0; 6];
    crate::util_blas::copy(&mut tran, vec, 6);
    crate::inline::sub3(&mut dif, newpos, oldpos);
    if flg_force != 0 {
        crate::inline::cross(&mut cros, &dif, crate::inline::at3(vec, 3));
        crate::inline::sub3(
            crate::inline::at3_mut(&mut tran, 0),
            crate::inline::at3(vec, 0),
            &cros,
        );
    } else {
        crate::inline::cross(&mut cros, &dif, crate::inline::at3(vec, 0));
        crate::inline::sub3(
            crate::inline::at3_mut(&mut tran, 3),
            crate::inline::at3(vec, 3),
            &cros,
        );
    }
    if let Some(rotnew2old) = rotnew2old {
        crate::inline::mul_mat_tvec3(
            crate::inline::at3_mut(res, 0),
            rotnew2old,
            crate::inline::at3(&tran, 0),
        );
        crate::inline::mul_mat_tvec3(
            crate::inline::at3_mut(res, 3),
            rotnew2old,
            crate::inline::at3(&tran, 3),
        );
    } else {
        crate::inline::copy6(res, &tran);
    }
}

pub fn make_frame(frame: &mut [f64; 9]) {
    let mut tmp = [0.0; 3];
    if crate::util_blas::normalize3(crate::inline::at3_mut(frame, 0)) < 0.5 {
        panic!("xaxis of contact frame undefined");
    }
    if crate::util_blas::dot3(crate::inline::at3(frame, 3), crate::inline::at3(frame, 3)) < 0.25 {
        crate::util_blas::zero3(crate::inline::at3_mut(frame, 3));
        if frame[1] < 0.5 && frame[1] > -0.5 {
            frame[4] = 1.0;
        } else {
            frame[5] = 1.0;
        }
    }
    crate::inline::scl3(
        &mut tmp,
        crate::inline::at3(frame, 0),
        crate::util_blas::dot3(crate::inline::at3(frame, 0), crate::inline::at3(frame, 3)),
    );
    crate::inline::sub_from3(crate::inline::at3_mut(frame, 3), &tmp);
    crate::util_blas::normalize3(crate::inline::at3_mut(frame, 3));
    let xaxis = *crate::inline::at3(frame, 0);
    let yaxis = *crate::inline::at3(frame, 3);
    crate::inline::cross(crate::inline::at3_mut(frame, 6), &xaxis, &yaxis);
}

pub fn euler2quat(quat: &mut [f64; 4], euler: &[f64; 3], seq: &[u8]) {
    let mut len = 0;
    while len < 4 && len < seq.len() && seq[len] != 0 {
        len += 1;
    }
    if len != 3 {
        panic!("seq must contain exactly 3 characters");
    }
    let mut tmp = [1.0, 0.0, 0.0, 0.0];
    for i in 0..3 {
        let mut rot = [(euler[i] / 2.0).cos(), 0.0, 0.0, 0.0];
        let sa = (euler[i] / 2.0).sin();
        if seq[i] == b'x' || seq[i] == b'X' {
            rot[1] = sa;
        } else if seq[i] == b'y' || seq[i] == b'Y' {
            rot[2] = sa;
        } else if seq[i] == b'z' || seq[i] == b'Z' {
            rot[3] = sa;
        } else {
            panic!(
                "seq[{}] is '{}', should be one of x, y, z, X, Y, Z",
                i, seq[i] as char
            );
        }
        let old_tmp = tmp;
        if seq[i] == b'x' || seq[i] == b'y' || seq[i] == b'z' {
            crate::util_spatial::mul_quat(&mut tmp, &old_tmp, &rot);
        } else {
            crate::util_spatial::mul_quat(&mut tmp, &rot, &old_tmp);
        }
    }
    crate::inline::copy4(quat, &tmp);
}
