//! Tendon wrapping, actuator models, geometry and miscellaneous numerical utilities.
//!
//! Translated from `src/engine/engine_util_misc.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::inline::{at3, at3_mut};
use crate::types::{
    GEOM_BOX, GEOM_CAPSULE, GEOM_CYLINDER, GEOM_ELLIPSOID, GEOM_PLANE, GEOM_SPHERE, MAXVAL, MINVAL,
    PI, PROJ_PERSPECTIVE, WRAP_CYLINDER, WRAP_SPHERE,
};
use crate::util_blas::{
    add3, add_scl, add_to3, copy, copy3, dot, dot3, mul_mat_tvec3, mul_mat_vec3, norm, norm3, normalize,
    normalize3, scl, scl3, sub, sub3,
};
use crate::util_spatial::cross;

// Check for intersection of two 2D line segments.
fn is_intersect(p1: &[f64], p2: &[f64], p3: &[f64], p4: &[f64]) -> bool {
    let u = [p2[0] - p1[0], p2[1] - p1[1]];
    let v = [p4[0] - p3[0], p4[1] - p3[1]];
    let det = v[1] * u[0] - v[0] * u[1];
    if det.abs() < MINVAL {
        return false;
    }
    let s = if det > 0.0 { 1.0 } else { -1.0 };
    let w = [p1[0] - p3[0], p1[1] - p3[1]];
    let d = [p2[0] - p4[0], p2[1] - p4[1]];
    let a0 = (v[0] * w[1] - v[1] * w[0]) * s;
    let a1 = (v[0] * d[1] - v[1] * d[0]) * s;
    let b0 = (u[0] * w[1] - u[1] * w[0]) * s;
    let b1 = (u[0] * d[1] - u[1] * d[0]) * s;
    a0 >= 0.0 && a1 < 0.0 && b0 >= 0.0 && b1 < 0.0
}

// Curve length along circle.
fn length_circle(p0: &[f64], p1: &[f64], ind: usize, radius: f64) -> f64 {
    let cross = p0[1] * p1[0] - p0[0] * p1[1];
    let mut angle = cross.abs().atan2(dot(p0, p1, 2));
    if angle > 1e-6 && ((cross > 0.0 && ind != 0) || (cross < 0.0 && ind == 0)) {
        angle = 2.0 * PI - angle;
    }
    radius * angle
}

// Signed angle swept from p to q around the origin, in (-pi, pi].
fn winding_angle(p: &[f64], q: &[f64]) -> f64 {
    (p[0] * q[1] - p[1] * q[0]).atan2(p[0] * q[0] + p[1] * q[1])
}

// 2D circle wrap: return circular length, or -1 if there is no wrap.
fn wrap_circle(pnt: &mut [f64; 4], end: &[f64; 4], side: Option<&[f64; 2]>, radius: f64) -> f64 {
    let sqlen0 = end[0] * end[0] + end[1] * end[1];
    let sqlen1 = end[2] * end[2] + end[3] * end[3];
    let sqrad = radius * radius;
    if sqlen0 < sqrad || sqlen1 < sqrad || radius < MINVAL {
        return -1.0;
    }
    let dif = [end[2] - end[0], end[3] - end[1]];
    let dd = dif[0] * dif[0] + dif[1] * dif[1];
    if dd < MINVAL {
        return -1.0;
    }
    let mut a = -(dif[0] * end[0] + dif[1] * end[1]) / dd;
    if a < 0.0 {
        a = 0.0;
    } else if a > 1.0 {
        a = 1.0;
    }
    let mut tmp = [a * dif[0] + end[0], a * dif[1] + end[1]];
    let clear = tmp[0] * tmp[0] + tmp[1] * tmp[1] > sqrad;
    let mut ccw = 0;
    let mut wside = 0.0;
    if let Some(side) = side {
        wside = winding_angle(end, side) + winding_angle(side, &end[2..]);
        if clear && (wside - winding_angle(end, &end[2..])).abs() < PI {
            return -1.0;
        }
        ccw = usize::from(wside > 0.0);
    } else if clear {
        return -1.0;
    }
    let sqrt0 = (sqlen0 - sqrad).sqrt();
    let sqrt1 = (sqlen1 - sqrad).sqrt();
    let mut sol = [[[0.0; 2]; 2]; 2];
    let mut good = [0.0; 2];
    for i in 0..2 {
        let sgn = if i == 0 { 1.0 } else { -1.0 };
        sol[i][0][0] = (end[0] * sqrad + sgn * radius * end[1] * sqrt0) / sqlen0;
        sol[i][0][1] = (end[1] * sqrad - sgn * radius * end[0] * sqrt0) / sqlen0;
        sol[i][1][0] = (end[2] * sqrad - sgn * radius * end[3] * sqrt1) / sqlen1;
        sol[i][1][1] = (end[3] * sqrad + sgn * radius * end[2] * sqrt1) / sqlen1;
        sub(&mut tmp, &sol[i][0], &sol[i][1], 2);
        good[i] = -dot(&tmp, &tmp, 2);
        if is_intersect(end, &sol[i][0], &end[2..], &sol[i][1]) {
            good[i] = -10000.0;
        }
    }
    let i = if side.is_some() {
        ccw
    } else if good[0] > good[1] {
        0
    } else {
        1
    };
    pnt[0] = sol[i][0][0];
    pnt[1] = sol[i][0][1];
    pnt[2] = sol[i][1][0];
    pnt[3] = sol[i][1][1];
    if is_intersect(end, pnt, &end[2..], &pnt[2..]) {
        return -1.0;
    }
    if side.is_some() {
        let arc = wside - winding_angle(end, pnt) - winding_angle(&pnt[2..], &end[2..]);
        return radius * arc.abs();
    }
    length_circle(&sol[i][0], &sol[i][1], i, radius)
}

// 2D inside wrap: return 0 if wrapped, -1 otherwise.
fn wrap_inside(pnt: &mut [f64; 4], end: &[f64; 4], radius: f64) -> f64 {
    let maxiter = 20;
    let tolerance = 1e-6;
    let len0 = norm(end, 2);
    let len1 = norm(&end[2..], 2);
    let dif = [end[2] - end[0], end[3] - end[1]];
    let dd = dif[0] * dif[0] + dif[1] * dif[1];
    if len0 <= radius || len1 <= radius || radius < MINVAL || len0 < MINVAL || len1 < MINVAL {
        return -1.0;
    }
    if dd > MINVAL {
        let a = -(dif[0] * end[0] + dif[1] * end[1]) / dd;
        if a > 0.0 && a < 1.0 {
            let mut tmp = [0.0; 2];
            add_scl(&mut tmp, end, &dif, a, 2);
            if norm(&tmp, 2) <= radius {
                return -1.0;
            }
        }
    }
    pnt[0] = 0.5 * (end[0] + end[2]);
    pnt[1] = 0.5 * (end[1] + end[3]);
    normalize(pnt, 2);
    let input = *pnt;
    scl(pnt, &input, radius, 2);
    pnt[2] = pnt[0];
    pnt[3] = pnt[1];
    let a = radius / len0;
    let b = radius / len1;
    let cos_g = (len0 * len0 + len1 * len1 - dd) / (2.0 * len0 * len1);
    if cos_g < -1.0 + MINVAL {
        return -1.0;
    } else if cos_g > 1.0 - MINVAL {
        return 0.0;
    }
    let g = cos_g.acos();
    let mut phi = PI / 2.0;
    let mut z = phi.sin();
    let mut f = (a * z).asin() + (b * z).asin() - 2.0 * phi + g;
    if f > 0.0 {
        return 0.0;
    }
    let mut iter = 0;
    while iter < maxiter && f.abs() > tolerance {
        let df = phi.cos()
            * (a / max(MINVAL, (1.0 - z * z * a * a).sqrt())
                + b / max(MINVAL, (1.0 - z * z * b * b).sqrt()))
            - 2.0;
        if df > -MINVAL {
            return 0.0;
        }
        let phi1 = phi - f / df;
        if phi1 > phi {
            return 0.0;
        }
        phi = phi1;
        z = phi.sin();
        f = (a * z).asin() + (b * z).asin() - 2.0 * phi + g;
        if f > tolerance {
            return 0.0;
        }
        iter += 1;
    }
    if iter >= maxiter {
        return 0.0;
    }
    let mut vec = [0.0; 2];
    let ang;
    if end[0] * end[3] - end[1] * end[2] > 0.0 {
        copy(&mut vec, end, 2);
        ang = phi - (a * z).asin();
    } else {
        copy(&mut vec, &end[2..], 2);
        ang = phi - (b * z).asin();
    }
    normalize(&mut vec, 2);
    pnt[0] = radius * (ang.cos() * vec[0] - ang.sin() * vec[1]);
    pnt[1] = radius * (ang.sin() * vec[0] + ang.cos() * vec[1]);
    pnt[2] = pnt[0];
    pnt[3] = pnt[1];
    0.0
}

// Wrap tendons around spheres and cylinders; return wrap length, or -1.
pub fn wrap(
    wpnt: &mut [f64; 6],
    x0: &[f64; 3],
    x1: &[f64; 3],
    xpos: &[f64; 3],
    xmat: &[f64; 9],
    radius: f64,
    type_: i32,
    side: Option<&[f64; 3]>,
) -> f64 {
    if type_ != WRAP_SPHERE && type_ != WRAP_CYLINDER {
        panic!("unknown wrapping object type {}", type_);
    }
    let mut tmp = [0.0; 3];
    sub3(&mut tmp, x0, xpos);
    let mut p = [[0.0; 3]; 2];
    mul_mat_tvec3(&mut p[0], xmat, &tmp);
    sub3(&mut tmp, x1, xpos);
    mul_mat_tvec3(&mut p[1], xmat, &tmp);
    if norm3(&p[0]) < MINVAL || norm3(&p[1]) < MINVAL {
        return -1.0;
    }
    let mut axis = [[0.0; 3]; 2];
    if type_ == WRAP_SPHERE {
        copy3(&mut axis[0], &p[0]);
        normalize3(&mut axis[0]);
        let mut normal = [0.0; 3];
        cross(&mut normal, &p[0], &p[1]);
        let nrm = normalize3(&mut normal);
        if nrm < MINVAL {
            let mut i = 0;
            if axis[0][1].abs() > axis[0][0].abs() && axis[0][1].abs() > axis[0][2].abs() {
                i = 1;
            }
            if axis[0][2].abs() > axis[0][0].abs() && axis[0][2].abs() > axis[0][1].abs() {
                i = 2;
            }
            axis[1][0] = 1.0;
            axis[1][1] = 1.0;
            axis[1][2] = 1.0;
            axis[1][i] = 0.0;
            cross(&mut normal, &axis[0], &axis[1]);
            normalize3(&mut normal);
        }
        let axis0 = axis[0];
        cross(&mut axis[1], &normal, &axis0);
        normalize3(&mut axis[1]);
    } else {
        axis[0][0] = 1.0;
        axis[0][2] = 0.0;
        axis[0][1] = 0.0;
        axis[1][1] = 1.0;
        axis[1][2] = 0.0;
        axis[1][0] = 0.0;
    }
    let mut s = [0.0; 3];
    let mut d = [0.0; 4];
    let mut sd = [0.0; 2];
    d[0] = dot3(&p[0], &axis[0]);
    d[1] = dot3(&p[0], &axis[1]);
    d[2] = dot3(&p[1], &axis[0]);
    d[3] = dot3(&p[1], &axis[1]);
    if let Some(side) = side {
        sub3(&mut tmp, side, xpos);
        mul_mat_tvec3(&mut s, xmat, &tmp);
        sd[0] = dot3(&s, &axis[0]);
        sd[1] = dot3(&s, &axis[1]);
        normalize(&mut sd, 2);
        let input = sd;
        scl(&mut sd, &input, radius, 2);
    }
    let mut wlen;
    let mut pnt = [0.0; 4];
    if side.is_some() && norm3(&s) < radius {
        wlen = wrap_inside(&mut pnt, &d, radius);
    } else {
        wlen = wrap_circle(
            &mut pnt,
            &d,
            if side.is_some() { Some(&sd) } else { None },
            radius,
        );
    }
    if wlen < 0.0 {
        return -1.0;
    }
    let mut res = [0.0; 6];
    for i in 0..2 {
        scl3(at3_mut(&mut res, 3 * i), &axis[0], pnt[2 * i]);
        scl3(&mut tmp, &axis[1], pnt[2 * i + 1]);
        add_to3(at3_mut(&mut res, 3 * i), &tmp);
    }
    if type_ == WRAP_CYLINDER {
        let l0 = ((p[0][0] - res[0]) * (p[0][0] - res[0])
            + (p[0][1] - res[1]) * (p[0][1] - res[1]))
            .sqrt();
        let l1 = ((p[1][0] - res[3]) * (p[1][0] - res[3])
            + (p[1][1] - res[4]) * (p[1][1] - res[4]))
            .sqrt();
        res[2] = p[0][2] + (p[1][2] - p[0][2]) * l0 / (l0 + wlen + l1);
        res[5] = p[0][2] + (p[1][2] - p[0][2]) * (l0 + wlen) / (l0 + wlen + l1);
        let height = (res[5] - res[2]).abs();
        wlen = (wlen * wlen + height * height).sqrt();
    }
    mul_mat_vec3(at3_mut(wpnt, 0), xmat, at3(&res, 0));
    mul_mat_vec3(at3_mut(wpnt, 3), xmat, at3(&res, 3));
    add_to3(at3_mut(wpnt, 0), xpos);
    add_to3(at3_mut(wpnt, 3), xpos);
    wlen
}

// All three semi-axes of a geom.
pub fn geom_semi_axes(semiaxes: &mut [f64; 3], size: &[f64; 3], type_: i32) {
    match type_ {
        GEOM_SPHERE => {
            semiaxes[0] = size[0];
            semiaxes[1] = size[0];
            semiaxes[2] = size[0];
        }
        GEOM_CAPSULE => {
            semiaxes[0] = size[0];
            semiaxes[1] = size[0];
            semiaxes[2] = size[1] + size[0];
        }
        GEOM_CYLINDER => {
            semiaxes[0] = size[0];
            semiaxes[1] = size[0];
            semiaxes[2] = size[1];
        }
        _ => {
            semiaxes[0] = size[0];
            semiaxes[1] = size[1];
            semiaxes[2] = size[2];
        }
    }
}

// Return 1 if point is inside a primitive geom, 0 otherwise.
pub fn inside_primitive(
    pos: &[f64; 3],
    mat: &[f64; 9],
    size: &[f64; 3],
    type_: i32,
    point: &[f64; 3],
) -> i32 {
    let mut vec = [0.0; 3];
    sub3(&mut vec, point, pos);
    if type_ == GEOM_SPHERE {
        return i32::from(dot3(&vec, &vec) < size[0] * size[0]);
    }
    let mut plocal = [0.0; 3];
    mul_mat_tvec3(&mut plocal, mat, &vec);
    i32::from(match type_ {
        GEOM_CAPSULE => {
            let z = plocal[2];
            let z_clamped = clip(z, -size[1], size[1]);
            let z_dist_sq = (z - z_clamped) * (z - z_clamped);
            plocal[0] * plocal[0] + plocal[1] * plocal[1] + z_dist_sq < size[0] * size[0]
        }
        GEOM_ELLIPSOID => {
            plocal[0] * plocal[0] / (size[0] * size[0])
                + plocal[1] * plocal[1] / (size[1] * size[1])
                + plocal[2] * plocal[2] / (size[2] * size[2])
                < 1.0
        }
        GEOM_CYLINDER => {
            plocal[2].abs() < size[1]
                && plocal[0] * plocal[0] + plocal[1] * plocal[1] < size[0] * size[0]
        }
        GEOM_BOX => {
            plocal[0].abs() < size[0] && plocal[1].abs() < size[1] && plocal[2].abs() < size[2]
        }
        GEOM_PLANE => plocal[2] < 0.0,
        _ => false,
    })
}

// The ray through a camera pixel's centre (row 0 at the top of the image), from the intrinsics
// support::cam_intrinsics gives: from the camera position along a unit direction for a perspective
// camera; along the camera's view axis from a point on the image plane for an orthographic one.
pub fn cam_pixel_ray(origin: &mut [f64; 3], direction: &mut [f64; 3], cam_xpos: &[f64; 3], cam_xmat: &[f64; 9],
    col: i32, row: i32, fx: f64, fy: f64, cx: f64, cy: f64, projection: i32, ortho_extent: f64) {
    let px = col as f64 + 0.5 - cx;
    let py = row as f64 + 0.5 - cy;
    if projection == PROJ_PERSPECTIVE {
        copy3(origin, cam_xpos);
        // (x/fx, -y/fy, -1) in the camera frame, normalized
        let dir_cam = [px / fx, -py / fy, -1.0];
        mul_mat_vec3(direction, cam_xmat, &dir_cam);
        normalize3(direction);
    } else {
        direction[0] = -cam_xmat[2];
        direction[1] = -cam_xmat[5];
        direction[2] = -cam_xmat[8];
        // the extent is the image's full height
        let half_extent = ortho_extent / 2.0;
        let offset_cam = [px / fx * half_extent, -py / fy * half_extent, 0.0];
        let mut offset_world = [0.0; 3];
        mul_mat_vec3(&mut offset_world, cam_xmat, &offset_cam);
        add3(origin, cam_xpos, &offset_world);
    }
}

// Skipped flex: mju_defGradient, mju_evalBasis, mju_evalBasisArray, mju_cellLookup, flexInterpRotation, mju_flexGatherCellState, mju_flexInterpRotation2D, mju_flexGatherFaceState, mju_flexFaceNormal2D.

// Normalized muscle length-gain curve.
pub fn muscle_gain_length(length: f64, lmin: f64, lmax: f64) -> f64 {
    if lmin <= length && length <= lmax {
        let a = 0.5 * (lmin + 1.0);
        let b = 0.5 * (1.0 + lmax);
        // mjMAX uses strict >, unlike mju_max's >=.
        if length <= a {
            let x = (length - lmin) / if MINVAL > a - lmin { MINVAL } else { a - lmin };
            return 0.5 * x * x;
        } else if length <= 1.0 {
            let x = (1.0 - length) / if MINVAL > 1.0 - a { MINVAL } else { 1.0 - a };
            return 1.0 - 0.5 * x * x;
        } else if length <= b {
            let x = (length - 1.0) / if MINVAL > b - 1.0 { MINVAL } else { b - 1.0 };
            return 1.0 - 0.5 * x * x;
        } else {
            let x = (lmax - length) / if MINVAL > lmax - b { MINVAL } else { lmax - b };
            return 0.5 * x * x;
        }
    }
    0.0
}

// Muscle active force, prm = (range[2], force, scale, lmin, lmax, vmax, fpmax, fvmax).
pub fn muscle_gain(len: f64, vel: f64, lengthrange: &[f64; 2], acc0: f64, prm: &[f64; 9]) -> f64 {
    let range = [prm[0], prm[1]];
    let mut force = prm[2];
    let scale = prm[3];
    let lmin = prm[4];
    let lmax = prm[5];
    let vmax = prm[6];
    let fvmax = prm[8];
    if force < 0.0 {
        force = scale / if MINVAL > acc0 { MINVAL } else { acc0 };
    }
    let l0 = (lengthrange[1] - lengthrange[0])
        / if MINVAL > range[1] - range[0] {
            MINVAL
        } else {
            range[1] - range[0]
        };
    let l = range[0] + (len - lengthrange[0]) / if MINVAL > l0 { MINVAL } else { l0 };
    let v = vel
        / if MINVAL > l0 * vmax {
            MINVAL
        } else {
            l0 * vmax
        };
    let fl = muscle_gain_length(l, lmin, lmax);
    let fv;
    let y = fvmax - 1.0;
    if v <= -1.0 {
        fv = 0.0;
    } else if v <= 0.0 {
        fv = (v + 1.0) * (v + 1.0);
    } else if v <= y {
        fv = fvmax - (y - v) * (y - v) / if MINVAL > y { MINVAL } else { y };
    } else {
        fv = fvmax;
    }
    -force * fl * fv
}

// Muscle passive force.
pub fn muscle_bias(len: f64, lengthrange: &[f64; 2], acc0: f64, prm: &[f64; 9]) -> f64 {
    let range = [prm[0], prm[1]];
    let mut force = prm[2];
    let scale = prm[3];
    let lmax = prm[5];
    let fpmax = prm[7];
    if force < 0.0 {
        force = scale / if MINVAL > acc0 { MINVAL } else { acc0 };
    }
    let l0 = (lengthrange[1] - lengthrange[0])
        / if MINVAL > range[1] - range[0] {
            MINVAL
        } else {
            range[1] - range[0]
        };
    let l = range[0] + (len - lengthrange[0]) / if MINVAL > l0 { MINVAL } else { l0 };
    let b = 0.5 * (1.0 + lmax);
    if l <= 1.0 {
        0.0
    } else if l <= b {
        let x = (l - 1.0) / if MINVAL > b - 1.0 { MINVAL } else { b - 1.0 };
        -force * fpmax * 0.5 * x * x
    } else {
        let x = (l - b) / if MINVAL > b - 1.0 { MINVAL } else { b - 1.0 };
        -force * fpmax * (0.5 + x)
    }
}

// Muscle time constant with optional smoothing.
pub fn muscle_dynamics_timescale(
    dctrl: f64,
    tau_act: f64,
    tau_deact: f64,
    smoothing_width: f64,
) -> f64 {
    let tau;
    if smoothing_width < MINVAL {
        tau = if dctrl > 0.0 { tau_act } else { tau_deact };
    } else {
        tau = tau_deact + (tau_act - tau_deact) * sigmoid(dctrl / smoothing_width + 0.5);
    }
    tau
}

// Muscle activation dynamics, prm = (tau_act, tau_deact, smoothing_width).
pub fn muscle_dynamics(ctrl: f64, act: f64, prm: &[f64; 3]) -> f64 {
    let ctrlclamp = clip(ctrl, 0.0, 1.0);
    let actclamp = clip(act, 0.0, 1.0);
    let tau_act = prm[0] * (0.5 + 1.5 * actclamp);
    let tau_deact = prm[1] / (0.5 + 1.5 * actclamp);
    let smoothing_width = prm[2];
    let dctrl = ctrlclamp - act;
    let tau = muscle_dynamics_timescale(dctrl, tau_act, tau_deact, smoothing_width);
    dctrl / if MINVAL > tau { MINVAL } else { tau }
}

// LuGre Stribeck function.
pub fn lugre_stribeck(velocity: f64, f_c: f64, f_s: f64, v_s: f64) -> f64 {
    let ratio = velocity / max(MINVAL, v_s);
    f_c + (f_s - f_c) * (-ratio * ratio).exp()
}

// Activation slot indices (-1 = inactive), from engine_util_misc.h's mjDCMotorSlots.
pub struct DCMotorSlots {
    pub slew: i32,
    pub integral: i32,
    pub temperature: i32,
    pub bristle: i32,
    pub current: i32,
    pub num_slots: i32,
}

// Compute DC motor activation slot indices from parameter arrays.
pub fn dcmotor_slots(dynprm: &[f64], gainprm: &[f64]) -> DCMotorSlots {
    let mut s = DCMotorSlots {
        slew: -1,
        integral: -1,
        temperature: -1,
        bristle: -1,
        current: -1,
        num_slots: 0,
    };
    if dynprm[7] > 0.0 {
        s.slew = s.num_slots;
        s.num_slots += 1;
    }
    if gainprm[5] > 0.0 {
        s.integral = s.num_slots;
        s.num_slots += 1;
    }
    if dynprm[2] > 0.0 {
        s.temperature = s.num_slots;
        s.num_slots += 1;
    }
    if dynprm[5] > 0.0 {
        s.bristle = s.num_slots;
        s.num_slots += 1;
    }
    if dynprm[0] > 0.0 {
        s.current = s.num_slots;
        s.num_slots += 1;
    }
    s
}

// Skipped base64: _decode, mju_encodeBase64, mju_isValidBase64, mju_decodeBase64.
// History buffers: [user value, cursor, times(n), values(n*dim)]; the cursor is the physical index of
// the newest sample, the oldest follows it.

// physical index of logical index `logical` (0 the oldest, n-1 the newest)
fn history_physical_index(cursor: i32, n: i32, logical: i32) -> i32 {
    (cursor + 1 + logical) % n
}

// the logical index i with times[i-1] < t <= times[i] by a circular binary search: 0 if t is at or before the
// oldest time, n if after the newest
fn history_find_index(times: &[f64], n: i32, cursor: i32, t: f64) -> i32 {
    let t_oldest = times[history_physical_index(cursor, n, 0) as usize];
    let t_newest = times[history_physical_index(cursor, n, n-1) as usize];
    if t <= t_oldest {
        return 0;
    }
    if t > t_newest {
        return n;
    }
    // the smallest logical i with times[i] >= t
    let mut lo = 0;
    let mut hi = n - 1;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if times[history_physical_index(cursor, n, mid) as usize] < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

/// mju_historyInit: set a history buffer's user value and its times and values, given oldest first.
/// `times` None keeps the buffer's own times, read through its cursor; `values` None keeps its values.
/// Times must increase strictly.
pub fn history_init(buf: &mut [f64], n: i32, dim: usize, times: Option<&[f64]>, values: Option<&[f64]>, user: f64) {
    let own = times.is_none();
    let mut cursor = if own { buf[1] as i32 } else { n - 1 };
    if cursor < 0 || cursor >= n {
        cursor = n - 1;
    }
    {
        let t = times.unwrap_or(&buf[2..2 + n as usize]);
        for i in 0..n-1 {
            let idx0 = if own { history_physical_index(cursor, n, i) } else { i } as usize;
            let idx1 = if own { history_physical_index(cursor, n, i+1) } else { i+1 } as usize;
            if t[idx1] - t[idx0] < MINVAL {
                crate::util_errmem::error(&format!("times must be strictly increasing, got times[{}]={} >= times[{}]={}",
                    i, t[idx0], i+1, t[idx1]));
            }
        }
    }
    buf[0] = user;
    buf[1] = cursor as f64;
    let nu = n as usize;
    if let Some(times) = times {
        copy(&mut buf[2..], times, nu);
        if let Some(values) = values {
            copy(&mut buf[2 + nu..], values, nu*dim);
        }
    } else if let Some(values) = values {
        for i in 0..n {
            let phys = history_physical_index(cursor, n, i) as usize;
            copy(&mut buf[2 + nu + phys*dim..], &values[i as usize*dim..], dim);
        }
    }
}

/// mju_historyInsert: the slot for a sample at time `t`, kept in time order, as an index into `buf` where
/// the caller writes its `dim` values: an existing sample's at the same time, else a new one that drops
/// the oldest.
pub fn history_insert(buf: &mut [f64], n: i32, dim: usize, t: f64) -> usize {
    let mut cursor = buf[1] as i32;
    let values = 2 + n as usize;
    let (times, _) = buf[2..].split_at_mut(n as usize);
    let i = history_find_index(times, n, cursor, t);

    // the same time as a sample's: its slot
    if i < n {
        let phys_i = history_physical_index(cursor, n, i) as usize;
        if (t - times[phys_i]).abs() < MINVAL {
            return values + phys_i*dim;
        }
    }

    // older than the oldest: replace it
    if i == 0 {
        let oldest_phys = history_physical_index(cursor, n, 0) as usize;
        times[oldest_phys] = t;
        return values + oldest_phys*dim;
    }

    // newer than the newest: the cursor moves onto the oldest slot
    if i == n {
        cursor = (cursor + 1) % n;
        times[cursor as usize] = t;
        buf[1] = cursor as f64;
        return values + cursor as usize*dim;
    }

    // out of order: shift logical 1..i-1 down one, dropping the oldest, and insert at i-1
    for j in 0..i-1 {
        let src_phys = history_physical_index(cursor, n, j+1) as usize;
        let dst_phys = history_physical_index(cursor, n, j) as usize;
        buf[2 + dst_phys] = buf[2 + src_phys];
        buf.copy_within(values + src_phys*dim..values + (src_phys+1)*dim, values + dst_phys*dim);
    }
    let insert_phys = history_physical_index(cursor, n, i-1) as usize;
    buf[2 + insert_phys] = t;
    values + insert_phys*dim
}

/// mju_historyRead: a history buffer's value at time `t`, by zero-order hold (`interp` 0), linear (1) or
/// cubic (2) interpolation, held at the ends. A sample read as it is is returned as its index into
/// `buf` (`res` untouched); an interpolated value is written to `res` and None returned.
pub fn history_read(buf: &[f64], n: i32, dim: usize, res: &mut [f64], t: f64, interp: i32) -> Option<usize> {
    let cursor = buf[1] as i32;
    let times = &buf[2..2 + n as usize];
    let v0 = 2 + n as usize;
    let values = &buf[v0..];
    let oldest_phys = history_physical_index(cursor, n, 0) as usize;
    let newest_phys = history_physical_index(cursor, n, n-1) as usize;

    // held before the oldest and after the newest
    if t <= times[oldest_phys] + MINVAL {
        return Some(v0 + oldest_phys*dim);
    }
    if t >= times[newest_phys] - MINVAL {
        return Some(v0 + newest_phys*dim);
    }

    // the bracket times[i-1] < t <= times[i], i > 0
    let i = history_find_index(times, n, cursor, t);
    let phys_i = history_physical_index(cursor, n, i) as usize;
    if (t - times[phys_i]).abs() < MINVAL {
        return Some(v0 + phys_i*dim);
    }
    let phys_lo = history_physical_index(cursor, n, i-1) as usize;
    let phys_hi = phys_i;

    // zero-order hold: the latest sample at or before t
    if interp == 0 {
        return Some(v0 + phys_lo*dim);
    }

    let dt = times[phys_hi] - times[phys_lo];
    let alpha = (t - times[phys_lo]) / dt;
    if interp == 1 {
        for d in 0..dim {
            res[d] = values[phys_lo*dim+d] + alpha * (values[phys_hi*dim+d] - values[phys_lo*dim+d]);
        }
    } else {
        // cubic Hermite with finite-differenced Catmull-Rom slopes, zero at the ends
        let alpha2 = alpha * alpha;
        let alpha3 = alpha2 * alpha;
        let h00 = 2.0*alpha3 - 3.0*alpha2 + 1.0;
        let h10 = alpha3 - 2.0*alpha2 + alpha;
        let h01 = -2.0*alpha3 + 3.0*alpha2;
        let h11 = alpha3 - alpha2;
        for d in 0..dim {
            let mut m_lo = 0.0;
            if i > 1 {
                let phys_lo_prev = history_physical_index(cursor, n, i-2) as usize;
                let dt_lo = times[phys_hi] - times[phys_lo_prev];
                m_lo = (values[phys_hi*dim+d] - values[phys_lo_prev*dim+d]) / dt_lo;
            }
            let mut m_hi = 0.0;
            if i < n - 1 {
                let phys_hi_next = history_physical_index(cursor, n, i+1) as usize;
                let dt_hi = times[phys_hi_next] - times[phys_lo];
                m_hi = (values[phys_hi_next*dim+d] - values[phys_lo*dim+d]) / dt_hi;
            }
            res[d] = h00 * values[phys_lo*dim+d] + h10 * dt * m_lo + h01 * values[phys_hi*dim+d] + h11 * dt * m_hi;
        }
    }
    None
}

// Convert contact force to pyramid representation.
pub fn encode_pyramid(pyramid: &mut [f64], force: &[f64], mu: &[f64], dim: usize) {
    if dim == 1 {
        pyramid[0] = force[0];
        return;
    }
    let a = force[0] / (dim - 1) as f64;
    for i in 0..dim - 1 {
        let b = min(a, force[i + 1] / mu[i]);
        pyramid[2 * i] = 0.5 * (a + b);
        pyramid[2 * i + 1] = 0.5 * (a - b);
    }
}

// Convert pyramid representation to contact force.
pub fn decode_pyramid(force: &mut [f64], pyramid: &[f64], mu: &[f64], dim: usize) {
    if dim == 1 {
        force[0] = pyramid[0];
        return;
    }
    force[0] = 0.0;
    for i in 0..2 * (dim - 1) {
        force[0] += pyramid[i];
    }
    for i in 0..dim - 1 {
        force[i + 1] = (pyramid[2 * i] - pyramid[2 * i + 1]) * mu[i];
    }
}

// Integrate spring-damper analytically, returning pos(t).
pub fn spring_damper(pos0: f64, vel0: f64, k: f64, b: f64, t: f64) -> f64 {
    let det = b * b - 4.0 * k;
    let det_tol = MINVAL * max(b * b, 4.0 * k.abs());
    if det > det_tol {
        let w = det.sqrt() / 2.0;
        let r1;
        let r2;
        if b >= 0.0 {
            r2 = -b / 2.0 - w;
            r1 = k / r2;
        } else {
            r1 = -b / 2.0 + w;
            r2 = k / r1;
        }
        let c1 = (pos0 * r2 - vel0) / (r2 - r1);
        let c2 = (pos0 * r1 - vel0) / (r1 - r2);
        c1 * (r1 * t).exp() + c2 * (r2 * t).exp()
    } else if det <= det_tol && det >= -det_tol {
        let c1 = pos0;
        let c2 = vel0 + b * c1 / 2.0;
        (-b * t / 2.0).exp() * (c1 + c2 * t)
    } else {
        let w = det.abs().sqrt() / 2.0;
        let c1 = pos0;
        let c2 = (vel0 + b * c1 / 2.0) / w;
        (-b * t / 2.0).exp() * (c1 * (w * t).cos() + c2 * (w * t).sin())
    }
}

// Return 1 outside the inflated box, -1 inside the deflated box, 0 between.
pub fn outside_box(
    point: &[f64; 3],
    pos: &[f64; 3],
    mat: &[f64; 9],
    size: &[f64; 3],
    inflate: f64,
) -> i32 {
    if inflate < 1.0 {
        panic!("inflation coefficient must be >= 1");
    }
    let mut vec = [point[0] - pos[0], point[1] - pos[1], point[2] - pos[2]];
    let input = vec;
    mul_mat_tvec3(&mut vec, mat, &input);
    let mut big = [size[0], size[1], size[2]];
    if inflate > 1.0 {
        let input = big;
        scl3(&mut big, &input, inflate);
    }
    if vec[0] > big[0]
        || vec[0] < -big[0]
        || vec[1] > big[1]
        || vec[1] < -big[1]
        || vec[2] > big[2]
        || vec[2] < -big[2]
    {
        return 1;
    }
    if inflate == 1.0 {
        return -1;
    }
    let small = [size[0] / inflate, size[1] / inflate, size[2] / inflate];
    if vec[0] < small[0]
        && vec[0] > -small[0]
        && vec[1] < small[1]
        && vec[1] > -small[1]
        && vec[2] < small[2]
        && vec[2] > -small[2]
    {
        return -1;
    }
    0
}

// Skipped matrix printing: mju_printMat, mju_printMatSparse.

pub fn min(a: f64, b: f64) -> f64 {
    if a <= b {
        a
    } else {
        b
    }
}

pub fn max(a: f64, b: f64) -> f64 {
    if a >= b {
        a
    } else {
        b
    }
}

pub fn clip(x: f64, min: f64, max: f64) -> f64 {
    if x < min {
        min
    } else if x > max {
        max
    } else {
        x
    }
}

pub fn sign(x: f64) -> f64 {
    (i32::from(x > 0.0) - i32::from(x < 0.0)) as f64
}

// Round to nearest integer, with the C function's explicit range checks.
pub fn round(x: f64) -> i32 {
    let d = x;
    if d > i32::MAX as f64 {
        return i32::MAX;
    }
    if d < i32::MIN as f64 {
        return i32::MIN;
    }
    d.round() as i32
}

// Convert type id to type name.
pub fn type2_str(type_: i32) -> Option<&'static str> {
    match type_ {
        crate::types::OBJ_BODY => Some("body"),
        crate::types::OBJ_XBODY => Some("xbody"),
        crate::types::OBJ_JOINT => Some("joint"),
        crate::types::OBJ_DOF => Some("dof"),
        crate::types::OBJ_GEOM => Some("geom"),
        crate::types::OBJ_SITE => Some("site"),
        crate::types::OBJ_CAMERA => Some("camera"),
        crate::types::OBJ_LIGHT => Some("light"),
        crate::types::OBJ_FLEX => Some("flex"),
        crate::types::OBJ_MESH => Some("mesh"),
        crate::types::OBJ_SKIN => Some("skin"),
        crate::types::OBJ_HFIELD => Some("hfield"),
        crate::types::OBJ_TEXTURE => Some("texture"),
        crate::types::OBJ_MATERIAL => Some("material"),
        crate::types::OBJ_PAIR => Some("pair"),
        crate::types::OBJ_EXCLUDE => Some("exclude"),
        crate::types::OBJ_EQUALITY => Some("equality"),
        crate::types::OBJ_TENDON => Some("tendon"),
        crate::types::OBJ_ACTUATOR => Some("actuator"),
        crate::types::OBJ_SENSOR => Some("sensor"),
        crate::types::OBJ_NUMERIC => Some("numeric"),
        crate::types::OBJ_TEXT => Some("text"),
        crate::types::OBJ_TUPLE => Some("tuple"),
        crate::types::OBJ_KEY => Some("key"),
        crate::types::OBJ_PLUGIN => Some("plugin"),
        crate::types::OBJ_FRAME => Some("frame"),
        _ => None,
    }
}

// Convert type name to type id.
pub fn str2_type(str_: &str) -> i32 {
    match str_ {
        "body" => crate::types::OBJ_BODY,
        "xbody" => crate::types::OBJ_XBODY,
        "joint" => crate::types::OBJ_JOINT,
        "dof" => crate::types::OBJ_DOF,
        "geom" => crate::types::OBJ_GEOM,
        "site" => crate::types::OBJ_SITE,
        "camera" => crate::types::OBJ_CAMERA,
        "light" => crate::types::OBJ_LIGHT,
        "flex" => crate::types::OBJ_FLEX,
        "mesh" => crate::types::OBJ_MESH,
        "skin" => crate::types::OBJ_SKIN,
        "hfield" => crate::types::OBJ_HFIELD,
        "texture" => crate::types::OBJ_TEXTURE,
        "material" => crate::types::OBJ_MATERIAL,
        "pair" => crate::types::OBJ_PAIR,
        "exclude" => crate::types::OBJ_EXCLUDE,
        "equality" => crate::types::OBJ_EQUALITY,
        "tendon" => crate::types::OBJ_TENDON,
        "actuator" => crate::types::OBJ_ACTUATOR,
        "sensor" => crate::types::OBJ_SENSOR,
        "numeric" => crate::types::OBJ_NUMERIC,
        "text" => crate::types::OBJ_TEXT,
        "tuple" => crate::types::OBJ_TUPLE,
        "key" => crate::types::OBJ_KEY,
        "plugin" => crate::types::OBJ_PLUGIN,
        "frame" => crate::types::OBJ_FRAME,
        _ => crate::types::OBJ_UNKNOWN,
    }
}

// Skipped new translation of mju_writeNumBytes: retain the existing callee below.
pub fn write_num_bytes(nbytes: usize) -> String {
    let suffix = b" KMGTPE";
    let mut i = 0;
    while i < 6 {
        let bits = 1_u64 << (10 * (6 - i));
        if nbytes as u64 >= bits && (nbytes as u64 & (bits - 1)) == 0 {
            break;
        }
        i += 1;
    }
    if i < 6 {
        format!(
            "{}{}",
            (nbytes as u64) >> (10 * (6 - i)),
            suffix[6 - i] as char
        )
    } else {
        format!("{}", (nbytes as u64) >> (10 * (6 - i)))
    }
}

// Skipped mju_warningText: already util_errmem::warning_text_of.

// Return 1 for NaN or a magnitude greater than MAXVAL.
pub fn is_bad(x: f64) -> i32 {
    i32::from(x != x || x > MAXVAL || x < -MAXVAL)
}

pub fn is_zero(vec: &[f64], n: usize) -> i32 {
    for i in 0..n {
        if vec[i] != 0.0 {
            return 0;
        }
    }
    1
}

pub fn is_zero_byte(vec: &[u8], n: usize) -> i32 {
    if n == 0 || vec[0] != 0 {
        return i32::from(n == 0);
    }
    i32::from(vec[..n - 1] == vec[1..n])
}

pub fn zero_int(res: &mut [i32], n: usize) {
    res[..n].fill(0);
}

pub fn copy_int(res: &mut [i32], vec: &[i32], n: usize) {
    res[..n].copy_from_slice(&vec[..n]);
}

pub fn fill_int(res: &mut [i32], val: i32, n: usize) {
    for i in 0..n {
        res[i] = val;
    }
}

// Skipped random normal: mju_standardNormal (deferred deterministic actuator noise).

pub fn f2n(res: &mut [f64], vec: &[f32], n: usize) {
    for i in 0..n {
        res[i] = vec[i] as f64;
    }
}

pub fn n2f(res: &mut [f32], vec: &[f64], n: usize) {
    for i in 0..n {
        res[i] = vec[i] as f32;
    }
}

pub fn d2n(res: &mut [f64], vec: &[f64], n: usize) {
    for i in 0..n {
        res[i] = vec[i];
    }
}

pub fn n2d(res: &mut [f64], vec: &[f64], n: usize) {
    for i in 0..n {
        res[i] = vec[i];
    }
}

// Gather, or copy when ind is absent.
pub fn gather(res: &mut [f64], vec: &[f64], ind: Option<&[i32]>, n: usize) {
    let Some(ind) = ind else {
        copy(res, vec, n);
        return;
    };
    for i in 0..n {
        res[i] = vec[ind[i] as usize];
    }
}

// Masked gather: zero at negative indices.
pub fn gather_masked(res: &mut [f64], vec: &[f64], ind: &[i32], n: usize) {
    for i in 0..n {
        res[i] = if ind[i] >= 0 {
            vec[ind[i] as usize]
        } else {
            0.0
        };
    }
}

// Scatter, or copy when ind is absent.
pub fn scatter(res: &mut [f64], vec: &[f64], ind: Option<&[i32]>, n: usize) {
    let Some(ind) = ind else {
        copy(res, vec, n);
        return;
    };
    for i in 0..n {
        res[ind[i] as usize] = vec[i];
    }
}

pub fn gather_int(res: &mut [i32], vec: &[i32], ind: &[i32], n: usize) {
    for i in 0..n {
        res[i] = vec[ind[i] as usize];
    }
}

pub fn scatter_int(res: &mut [i32], vec: &[i32], ind: &[i32], n: usize) {
    for i in 0..n {
        res[ind[i] as usize] = vec[i];
    }
}

pub fn sparse_map(
    map: &mut [i32],
    nr: usize,
    res_rowadr: &[i32],
    res_rownnz: &[i32],
    res_colind: &[i32],
    src_rowadr: &[i32],
    src_rownnz: &[i32],
    src_colind: &[i32],
) {
    for i in 0..nr {
        let mut res_cursor = res_rowadr[i];
        let res_end = res_cursor + res_rownnz[i];
        let mut src_cursor = src_rowadr[i];
        let src_end = src_cursor + src_rownnz[i];
        while res_cursor < res_end {
            let res_col = res_colind[res_cursor as usize];
            while src_cursor < src_end && src_colind[src_cursor as usize] < res_col {
                src_cursor += 1;
            }
            map[res_cursor as usize] = src_cursor;
            res_cursor += 1;
            src_cursor += 1;
        }
    }
}

pub fn lower2_sym_map(
    map: &mut [i32],
    nr: usize,
    res_rowadr: &[i32],
    res_rownnz: &[i32],
    res_colind: &[i32],
    src_rowadr: &[i32],
    src_rownnz: &[i32],
    src_colind: &[i32],
    cursor: &mut [i32],
) {
    if nr != 0 {
        let nnz = (res_rowadr[nr - 1] + res_rownnz[nr - 1]) as usize;
        map[..nnz].fill(-1);
        cursor[..nr].copy_from_slice(&res_rowadr[..nr]);
        for i in 0..nr {
            let src_start = src_rowadr[i];
            let src_end = src_start + src_rownnz[i];
            for k in src_start..src_end {
                let j = src_colind[k as usize];
                if j > i as i32 {
                    break;
                }
                let mut res_start = res_rowadr[i];
                let mut res_end = res_start + res_rownnz[i];
                let mut c = cursor[i];
                while c < res_end && res_colind[c as usize] < j {
                    c += 1;
                }
                if c < res_end && res_colind[c as usize] == j {
                    map[c as usize] = k;
                    c += 1;
                }
                cursor[i] = c;
                if j != i as i32 {
                    res_start = res_rowadr[j as usize];
                    res_end = res_start + res_rownnz[j as usize];
                    c = cursor[j as usize];
                    while c < res_end && res_colind[c as usize] < i as i32 {
                        c += 1;
                    }
                    if c < res_end && res_colind[c as usize] == i as i32 {
                        map[c as usize] = k;
                        c += 1;
                    }
                    cursor[j as usize] = c;
                }
            }
        }
    }
}

// Insertion sort, increasing order.
pub fn insertion_sort(list: &mut [f64], n: usize) {
    for i in 1..n {
        let x = list[i];
        let mut j = i as i32 - 1;
        while j >= 0 && list[j as usize] > x {
            list[(j + 1) as usize] = list[j as usize];
            j -= 1;
        }
        list[(j + 1) as usize] = x;
    }
}

pub fn insertion_sort_int(list: &mut [i32], n: usize) {
    for i in 1..n {
        let x = list[i];
        let mut j = i as i32 - 1;
        while j >= 0 && list[j as usize] > x {
            list[(j + 1) as usize] = list[j as usize];
            j -= 1;
        }
        list[(j + 1) as usize] = x;
    }
}

// Halton sequence.
pub fn halton(index: i32, base: i32) -> f64 {
    let mut n0 = index;
    let b = base as f64;
    let mut f = 1.0 / b;
    let mut hn = 0.0;
    while n0 > 0 {
        let n1 = n0 / base;
        let r = n0 - n1 * base;
        hn += f * r as f64;
        f /= b;
        n0 = n1;
    }
    hn
}

// Copy at most n-1 bytes and NUL-terminate; pad as C strncpy does.
pub fn strncpy<'a>(dst: &'a mut [u8], src: &[u8], n: usize) -> &'a mut [u8] {
    if n > 0 {
        let mut i = 0;
        while i < n - 1 && i < src.len() && src[i] != 0 {
            dst[i] = src[i];
            i += 1;
        }
        while i < n {
            dst[i] = 0;
            i += 1;
        }
    }
    dst
}

// Polynomial force coefficient: force = -x * poly_force(...).
pub fn poly_force(linear: f64, poly: &[f64], mut x: f64, n: usize, flg_odd: i32) -> f64 {
    x = if flg_odd != 0 { x.abs() } else { x };
    let mut res = linear;
    let mut xpow = 1.0;
    for i in 0..n {
        xpow *= x;
        res += poly[i] * xpow;
    }
    res
}

// Derivative of x * poly_force with respect to x; mjd_ retains the d_ prefix.
pub fn d_x_poly_force(linear: f64, poly: &[f64], mut x: f64, n: usize, flg_odd: i32) -> f64 {
    x = if flg_odd != 0 { x.abs() } else { x };
    let mut res = linear;
    let mut xpow = 1.0;
    for i in 0..n {
        xpow *= x;
        res += (i + 2) as f64 * poly[i] * xpow;
    }
    res
}

// Potential energy: integral from 0 to x of poly_force(t) * t dt.
pub fn poly_potential(linear: f64, poly: &[f64], mut x: f64, n: usize, flg_odd: i32) -> f64 {
    x = if flg_odd != 0 { x.abs() } else { x };
    let mut res = 0.5 * linear * (x * x);
    let mut xpow = x;
    for i in 0..n {
        xpow *= x;
        res += poly[i] / (i + 3) as f64 * (xpow * x);
    }
    res
}

// Quintic sigmoid on [0, 1].
pub fn sigmoid(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    x * x * x * (3.0 * x * (2.0 * x - 5.0) + 10.0)
}
