//! Analytic contacts between primitive geometries and closest-point weights.
//!
//! Translated from `src/engine/engine_collision_primitive.c` and its header of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::data::{Data, PreContact};
use crate::inline::{self as i, at3, at9};
use crate::model::Model;
use crate::types::MINVAL;
use crate::util_blas as u;
use crate::util_misc::clip;

// Raw plane : sphere. The C signature includes unused size1 and mat2.
fn raw_plane_sphere(
    con: &mut [PreContact],
    margin: f64,
    pos1: &[f64; 3],
    mat1: &[f64; 9],
    size1: &[f64],
    pos2: &[f64; 3],
    mat2: &[f64; 9],
    size2: &[f64],
) -> usize {
    let _ = (size1, mat2);
    con[0].normal[0] = mat1[2];
    con[0].normal[1] = mat1[5];
    con[0].normal[2] = mat1[8];
    let mut tmp = [pos2[0] - pos1[0], pos2[1] - pos1[1], pos2[2] - pos1[2]];
    let cdist = u::dot3(&tmp, &con[0].normal);
    if cdist > margin + size2[0] {
        return 0;
    }
    con[0].dist = cdist - size2[0];
    i::scl3(&mut tmp, &con[0].normal, -con[0].dist / 2.0 - size2[0]);
    i::add3(&mut con[0].pos, pos2, &tmp);
    i::zero3(&mut con[0].tangent);
    1
}

pub fn plane_sphere(
    m: &Model,
    d: &mut Data,
    con: &mut [PreContact],
    g1: i32,
    g2: i32,
    margin: f64,
) -> usize {
    let pos1 = at3(&d.geom_xpos, 3 * g1 as usize);
    let pos2 = at3(&d.geom_xpos, 3 * g2 as usize);
    let mat1 = at9(&d.geom_xmat, 9 * g1 as usize);
    let mat2 = at9(&d.geom_xmat, 9 * g2 as usize);
    let size1 = at3(&m.geom_size, 3 * g1 as usize);
    let size2 = at3(&m.geom_size, 3 * g2 as usize);
    raw_plane_sphere(con, margin, pos1, mat1, size1, pos2, mat2, size2)
}

pub fn plane_capsule(
    m: &Model,
    d: &mut Data,
    con: &mut [PreContact],
    g1: i32,
    g2: i32,
    margin: f64,
) -> usize {
    let pos1 = at3(&d.geom_xpos, 3 * g1 as usize);
    let pos2 = at3(&d.geom_xpos, 3 * g2 as usize);
    let mat1 = at9(&d.geom_xmat, 9 * g1 as usize);
    let mat2 = at9(&d.geom_xmat, 9 * g2 as usize);
    let size1 = at3(&m.geom_size, 3 * g1 as usize);
    let size2 = at3(&m.geom_size, 3 * g2 as usize);
    let axis = [mat2[2], mat2[5], mat2[8]];
    let segment = [size2[1] * axis[0], size2[1] * axis[1], size2[1] * axis[2]];
    let mut endpoint = [0.0; 3];
    u::add3(&mut endpoint, pos2, &segment);
    let n1 = raw_plane_sphere(con, margin, pos1, mat1, size1, &endpoint, mat2, size2);
    u::sub3(&mut endpoint, pos2, &segment);
    let n2 = raw_plane_sphere(
        &mut con[n1..],
        margin,
        pos1,
        mat1,
        size1,
        &endpoint,
        mat2,
        size2,
    );
    if n1 != 0 {
        i::copy3(&mut con[0].tangent, &axis);
    }
    if n2 != 0 {
        i::copy3(&mut con[n1].tangent, &axis);
    }
    n1 + n2
}

pub fn plane_cylinder(
    m: &Model,
    d: &mut Data,
    con: &mut [PreContact],
    g1: i32,
    g2: i32,
    margin: f64,
) -> usize {
    let pos1 = at3(&d.geom_xpos, 3 * g1 as usize);
    let pos2 = at3(&d.geom_xpos, 3 * g2 as usize);
    let mat1 = at9(&d.geom_xmat, 9 * g1 as usize);
    let mat2 = at9(&d.geom_xmat, 9 * g2 as usize);
    let size2 = at3(&m.geom_size, 3 * g2 as usize);
    let normal = [mat1[2], mat1[5], mat1[8]];
    let mut axis = [mat2[2], mat2[5], mat2[8]];
    let mut prjaxis = u::dot3(&normal, &axis);
    if prjaxis > 0.0 {
        let input = axis;
        u::scl3(&mut axis, &input, -1.0);
        prjaxis = -prjaxis;
    }
    let mut vec = [pos2[0] - pos1[0], pos2[1] - pos1[1], pos2[2] - pos1[2]];
    let dist0 = u::dot3(&vec, &normal);
    u::scl3(&mut vec, &axis, prjaxis);
    u::sub_from3(&mut vec, &normal);
    let len_sqr = u::dot3(&vec, &vec);
    if len_sqr >= MINVAL * MINVAL {
        let scl = size2[0] / len_sqr.sqrt();
        vec[0] *= scl;
        vec[1] *= scl;
        vec[2] *= scl;
    } else {
        vec[0] = mat2[0] * size2[0];
        vec[1] = mat2[3] * size2[0];
        vec[2] = mat2[6] * size2[0];
    }
    let prjvec = u::dot3(&vec, &normal);
    let input = axis;
    u::scl3(&mut axis, &input, size2[1]);
    prjaxis *= size2[1];
    let mut cnt = 0;
    if dist0 + prjaxis + prjvec <= margin {
        con[cnt].dist = dist0 + prjaxis + prjvec;
        i::add3(&mut con[cnt].pos, pos2, &vec);
        i::add_to3(&mut con[cnt].pos, &axis);
        let dist = con[cnt].dist;
        i::add_to_scl3(&mut con[cnt].pos, &normal, -dist * 0.5);
        i::copy3(&mut con[cnt].normal, &normal);
        i::zero3(&mut con[cnt].tangent);
        cnt += 1;
    } else {
        return 0;
    }
    if dist0 - prjaxis + prjvec <= margin {
        con[cnt].dist = dist0 - prjaxis + prjvec;
        i::add3(&mut con[cnt].pos, pos2, &vec);
        i::sub_from3(&mut con[cnt].pos, &axis);
        let dist = con[cnt].dist;
        i::add_to_scl3(&mut con[cnt].pos, &normal, -dist * 0.5);
        i::copy3(&mut con[cnt].normal, &normal);
        i::zero3(&mut con[cnt].tangent);
        cnt += 1;
    }
    let prjvec1 = -prjvec * 0.5;
    if dist0 + prjaxis + prjvec1 <= margin {
        let mut vec1 = [0.0; 3];
        i::cross(&mut vec1, &vec, &axis);
        u::normalize3(&mut vec1);
        let input = vec1;
        u::scl3(&mut vec1, &input, size2[0] * 3.0_f64.sqrt() / 2.0);
        con[cnt].dist = dist0 + prjaxis + prjvec1;
        i::add3(&mut con[cnt].pos, pos2, &vec1);
        i::add_to3(&mut con[cnt].pos, &axis);
        i::add_to_scl3(&mut con[cnt].pos, &vec, -0.5);
        let dist = con[cnt].dist;
        i::add_to_scl3(&mut con[cnt].pos, &normal, -dist * 0.5);
        i::copy3(&mut con[cnt].normal, &normal);
        i::zero3(&mut con[cnt].tangent);
        cnt += 1;
        con[cnt].dist = dist0 + prjaxis + prjvec1;
        i::sub3(&mut con[cnt].pos, pos2, &vec1);
        i::add_to3(&mut con[cnt].pos, &axis);
        i::add_to_scl3(&mut con[cnt].pos, &vec, -0.5);
        let dist = con[cnt].dist;
        i::add_to_scl3(&mut con[cnt].pos, &normal, -dist * 0.5);
        i::copy3(&mut con[cnt].normal, &normal);
        i::zero3(&mut con[cnt].tangent);
        cnt += 1;
    }
    cnt
}

pub fn plane_box(
    m: &Model,
    d: &mut Data,
    con: &mut [PreContact],
    g1: i32,
    g2: i32,
    margin: f64,
) -> usize {
    let pos1 = at3(&d.geom_xpos, 3 * g1 as usize);
    let pos2 = at3(&d.geom_xpos, 3 * g2 as usize);
    let mat1 = at9(&d.geom_xmat, 9 * g1 as usize);
    let mat2 = at9(&d.geom_xmat, 9 * g2 as usize);
    let size2 = at3(&m.geom_size, 3 * g2 as usize);
    let norm = [mat1[2], mat1[5], mat1[8]];
    let dif = [pos2[0] - pos1[0], pos2[1] - pos1[1], pos2[2] - pos1[2]];
    let dist = u::dot3(&dif, &norm);
    let mut cnt = 0;
    for i in 0..8 {
        let mut vec = [0.0; 3];
        vec[0] = if i & 1 != 0 { size2[0] } else { -size2[0] };
        vec[1] = if i & 2 != 0 { size2[1] } else { -size2[1] };
        vec[2] = if i & 4 != 0 { size2[2] } else { -size2[2] };
        let mut corner = [0.0; 3];
        u::mul_mat_vec3(&mut corner, mat2, &vec);
        let ldist = u::dot3(&norm, &corner);
        if dist + ldist > margin || ldist > 0.0 {
            continue;
        }
        con[cnt].dist = dist + ldist;
        i::copy3(&mut con[cnt].normal, &norm);
        i::add_to3(&mut corner, pos2);
        i::scl3(&mut vec, &norm, -con[cnt].dist / 2.0);
        i::add3(&mut con[cnt].pos, &corner, &vec);
        i::zero3(&mut con[cnt].tangent);
        cnt += 1;
        if cnt >= 4 {
            return 4;
        }
    }
    cnt
}

fn raw_sphere_sphere(
    con: &mut [PreContact],
    margin: f64,
    pos1: &[f64; 3],
    mat1: &[f64; 9],
    size1: &[f64],
    pos2: &[f64; 3],
    mat2: &[f64; 9],
    size2: &[f64],
) -> usize {
    let dif = [pos1[0] - pos2[0], pos1[1] - pos2[1], pos1[2] - pos2[2]];
    let cdist_sqr = u::dot3(&dif, &dif);
    let min_dist = margin + size1[0] + size2[0];
    if cdist_sqr > min_dist * min_dist {
        return 0;
    }
    con[0].dist = cdist_sqr.sqrt() - size1[0] - size2[0];
    i::sub3(&mut con[0].normal, pos2, pos1);
    let len = u::normalize3(&mut con[0].normal);
    if len < MINVAL {
        let axis1 = [mat1[2], mat1[5], mat1[8]];
        let axis2 = [mat2[2], mat2[5], mat2[8]];
        i::cross(&mut con[0].normal, &axis1, &axis2);
        u::normalize3(&mut con[0].normal);
    }
    let normal = con[0].normal;
    let dist = con[0].dist;
    i::scl3(&mut con[0].pos, &normal, size1[0] + dist / 2.0);
    i::add_to3(&mut con[0].pos, pos1);
    i::zero3(&mut con[0].tangent);
    1
}

pub fn sphere_sphere(
    m: &Model,
    d: &mut Data,
    con: &mut [PreContact],
    g1: i32,
    g2: i32,
    margin: f64,
) -> usize {
    let pos1 = at3(&d.geom_xpos, 3 * g1 as usize);
    let mat1 = at9(&d.geom_xmat, 9 * g1 as usize);
    let size1 = at3(&m.geom_size, 3 * g1 as usize);
    let pos2 = at3(&d.geom_xpos, 3 * g2 as usize);
    let mat2 = at9(&d.geom_xmat, 9 * g2 as usize);
    let size2 = at3(&m.geom_size, 3 * g2 as usize);
    raw_sphere_sphere(con, margin, pos1, mat1, size1, pos2, mat2, size2)
}

pub fn raw_sphere_capsule(
    con: &mut [PreContact],
    margin: f64,
    pos1: &[f64; 3],
    mat1: &[f64; 9],
    size1: &[f64],
    pos2: &[f64; 3],
    mat2: &[f64; 9],
    size2: &[f64],
) -> usize {
    let len = size2[1];
    let axis = [mat2[2], mat2[5], mat2[8]];
    let mut vec = [pos1[0] - pos2[0], pos1[1] - pos2[1], pos1[2] - pos2[2]];
    let x = clip(u::dot3(&axis, &vec), -len, len);
    i::scl3(&mut vec, &axis, x);
    i::add_to3(&mut vec, pos2);
    raw_sphere_sphere(con, margin, pos1, mat1, size1, &vec, mat2, size2)
}

pub fn sphere_capsule(
    m: &Model,
    d: &mut Data,
    con: &mut [PreContact],
    g1: i32,
    g2: i32,
    margin: f64,
) -> usize {
    let pos1 = at3(&d.geom_xpos, 3 * g1 as usize);
    let pos2 = at3(&d.geom_xpos, 3 * g2 as usize);
    let mat1 = at9(&d.geom_xmat, 9 * g1 as usize);
    let mat2 = at9(&d.geom_xmat, 9 * g2 as usize);
    let size1 = at3(&m.geom_size, 3 * g1 as usize);
    let size2 = at3(&m.geom_size, 3 * g2 as usize);
    raw_sphere_capsule(con, margin, pos1, mat1, size1, pos2, mat2, size2)
}

pub fn sphere_cylinder(
    m: &Model,
    d: &mut Data,
    con: &mut [PreContact],
    g1: i32,
    g2: i32,
    margin: f64,
) -> usize {
    let pos1 = at3(&d.geom_xpos, 3 * g1 as usize);
    let pos2 = at3(&d.geom_xpos, 3 * g2 as usize);
    let mat1 = at9(&d.geom_xmat, 9 * g1 as usize);
    let mat2 = at9(&d.geom_xmat, 9 * g2 as usize);
    let size1 = at3(&m.geom_size, 3 * g1 as usize);
    let size2 = at3(&m.geom_size, 3 * g2 as usize);
    let radius = size2[0];
    let height = size2[1];
    let axis = [mat2[2], mat2[5], mat2[8]];
    let mut vec = [pos1[0] - pos2[0], pos1[1] - pos2[1], pos1[2] - pos2[2]];
    let x = u::dot3(&axis, &vec);
    let mut a_proj = [0.0; 3];
    let mut p_proj = [0.0; 3];
    i::scl3(&mut a_proj, &axis, x);
    i::sub3(&mut p_proj, &vec, &a_proj);
    let p_proj_sqr = u::dot3(&p_proj, &p_proj);
    let mut collide_side = x.abs() < height;
    let mut collide_cap = p_proj_sqr < radius * radius;
    if collide_side && collide_cap {
        let dist_cap = height - x.abs();
        let dist_radius = radius - p_proj_sqr.sqrt();
        if dist_cap < dist_radius {
            collide_side = false;
        } else {
            collide_cap = false;
        }
    }
    if collide_side {
        i::add_to3(&mut a_proj, pos2);
        return raw_sphere_sphere(con, margin, pos1, mat1, size1, &a_proj, mat2, size2);
    }
    if collide_cap {
        let flipmat = [
            -mat2[0], mat2[1], -mat2[2], -mat2[3], mat2[4], -mat2[5], -mat2[6], mat2[7], -mat2[8],
        ];
        let mat_cap;
        let mut pos_cap = [0.0; 3];
        if x > 0.0 {
            u::add_scl3(&mut pos_cap, pos2, &axis, height);
            mat_cap = mat2;
        } else {
            u::add_scl3(&mut pos_cap, pos2, &axis, -height);
            mat_cap = &flipmat;
        }
        let ncon = raw_plane_sphere(con, margin, &pos_cap, mat_cap, size2, pos1, mat1, size1);
        if ncon != 0 {
            let normal = con[0].normal;
            u::scl3(&mut con[0].normal, &normal, -1.0);
        }
        return ncon;
    }
    let input = p_proj;
    u::scl3(&mut p_proj, &input, size2[0] / p_proj_sqr.sqrt());
    i::scl3(&mut vec, &axis, if x > 0.0 { height } else { -height });
    i::add_to3(&mut vec, &p_proj);
    i::add_to3(&mut vec, pos2);
    let size_zero = [0.0];
    raw_sphere_sphere(con, margin, pos1, mat1, size1, &vec, mat2, &size_zero)
}

pub fn raw_capsule_capsule(
    con: &mut [PreContact],
    margin: f64,
    pos1: &[f64; 3],
    mat1: &[f64; 9],
    size1: &[f64],
    pos2: &[f64; 3],
    mat2: &[f64; 9],
    size2: &[f64],
) -> usize {
    let axis1 = [mat1[2] * size1[1], mat1[5] * size1[1], mat1[8] * size1[1]];
    let axis2 = [mat2[2] * size2[1], mat2[5] * size2[1], mat2[8] * size2[1]];
    let mut p1 = [0.0; 3];
    let mut p2 = [0.0; 3];
    let mut q1 = [0.0; 3];
    let mut q2 = [0.0; 3];
    u::sub3(&mut p1, pos1, &axis1);
    u::add3(&mut p2, pos1, &axis1);
    u::sub3(&mut q1, pos2, &axis2);
    u::add3(&mut q2, pos2, &axis2);
    let mut w = [0.0; 4];
    let n = seg_weights(&mut w, &p1, &p2, &q1, &q2);
    let mut ncon = 0;
    for i in 0..n {
        let mut vec1 = [0.0; 3];
        let mut vec2 = [0.0; 3];
        for k in 0..3 {
            vec1[k] = p1[k] + w[2 * i] * (p2[k] - p1[k]);
            vec2[k] = q1[k] + w[2 * i + 1] * (q2[k] - q1[k]);
        }
        ncon += raw_sphere_sphere(
            &mut con[ncon..],
            margin,
            &vec1,
            mat1,
            size1,
            &vec2,
            mat2,
            size2,
        );
    }
    ncon
}

pub fn capsule_capsule(
    m: &Model,
    d: &mut Data,
    con: &mut [PreContact],
    g1: i32,
    g2: i32,
    margin: f64,
) -> usize {
    let pos1 = at3(&d.geom_xpos, 3 * g1 as usize);
    let pos2 = at3(&d.geom_xpos, 3 * g2 as usize);
    let mat1 = at9(&d.geom_xmat, 9 * g1 as usize);
    let mat2 = at9(&d.geom_xmat, 9 * g2 as usize);
    let size1 = at3(&m.geom_size, 3 * g1 as usize);
    let size2 = at3(&m.geom_size, 3 * g2 as usize);
    raw_capsule_capsule(con, margin, pos1, mat1, size1, pos2, mat2, size2)
}

// Barycentric weights of the closest point on triangle ABC to p.
pub fn tri_weights(w: &mut [f64; 3], p: &[f64; 3], a: &[f64; 3], b: &[f64; 3], c: &[f64; 3]) {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let bp = [p[0] - b[0], p[1] - b[1], p[2] - b[2]];
    let cp = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
    let dot1 = u::dot3(&ab, &ap);
    let dot2 = u::dot3(&ac, &ap);
    let dot3 = u::dot3(&ab, &bp);
    let dot4 = u::dot3(&ac, &bp);
    let dot5 = u::dot3(&ab, &cp);
    let dot6 = u::dot3(&ac, &cp);
    if dot1 <= 0.0 && dot2 <= 0.0 {
        w[0] = 1.0;
        w[1] = 0.0;
        w[2] = 0.0;
        return;
    }
    if dot3 >= 0.0 && dot4 <= dot3 {
        w[0] = 0.0;
        w[1] = 1.0;
        w[2] = 0.0;
        return;
    }
    if dot6 >= 0.0 && dot5 <= dot6 {
        w[0] = 0.0;
        w[1] = 0.0;
        w[2] = 1.0;
        return;
    }
    let va = dot3 * dot6 - dot5 * dot4;
    let vb = dot5 * dot2 - dot1 * dot6;
    let vc = dot1 * dot4 - dot3 * dot2;
    if vc <= 0.0 && dot1 >= 0.0 && dot3 <= 0.0 {
        let t = dot1 / (dot1 - dot3);
        w[0] = 1.0 - t;
        w[1] = t;
        w[2] = 0.0;
        return;
    }
    if vb <= 0.0 && dot2 >= 0.0 && dot6 <= 0.0 {
        let t = dot2 / (dot2 - dot6);
        w[0] = 1.0 - t;
        w[1] = 0.0;
        w[2] = t;
        return;
    }
    if va <= 0.0 && (dot4 - dot3) >= 0.0 && (dot5 - dot6) >= 0.0 {
        let t = (dot4 - dot3) / ((dot4 - dot3) + (dot5 - dot6));
        w[0] = 0.0;
        w[1] = 1.0 - t;
        w[2] = t;
        return;
    }
    let d = 1.0 / (va + vb + vc);
    let t = vb * d;
    let u = vc * d;
    w[0] = 1.0 - t - u;
    w[1] = t;
    w[2] = u;
}

#[inline]
fn clamp_seg(w: &mut [f64; 2], s: f64, t: f64, s0: f64, s1: f64) {
    if t < 0.0 {
        w[0] = clip(s0, 0.0, 1.0);
        w[1] = 0.0;
    } else if t > 1.0 {
        w[0] = clip(s1, 0.0, 1.0);
        w[1] = 1.0;
    } else {
        w[0] = s;
        w[1] = t;
    }
}

// Closest-point weights {s1, t1, s2, t2}; two pairs for parallel segments.
pub fn seg_weights(
    w: &mut [f64; 4],
    a: &[f64; 3],
    b: &[f64; 3],
    c: &[f64; 3],
    d: &[f64; 3],
) -> usize {
    let sin2 = 1e-8;
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let cd = [d[0] - c[0], d[1] - c[1], d[2] - c[2]];
    let ca = [a[0] - c[0], a[1] - c[1], a[2] - c[2]];
    let dot1 = u::dot3(&ab, &ab);
    let dot2 = u::dot3(&cd, &cd);
    let dot3 = u::dot3(&cd, &ca);
    let dot4 = u::dot3(&ab, &ca);
    let dot5 = u::dot3(&ab, &cd);
    if dot1 <= MINVAL && dot2 <= MINVAL {
        w[0] = 0.0;
        w[1] = 0.0;
        return 1;
    }
    if dot1 <= MINVAL {
        w[0] = 0.0;
        w[1] = clip(dot3 / dot2, 0.0, 1.0);
        return 1;
    }
    if dot2 <= MINVAL {
        w[0] = clip(-dot4 / dot1, 0.0, 1.0);
        w[1] = 0.0;
        return 1;
    }
    let s0 = -dot4 / dot1;
    let s1 = (dot5 - dot4) / dot1;
    let mut cross = [0.0; 3];
    i::cross(&mut cross, &ab, &cd);
    let det = u::dot3(&cross, &cross);
    if det > sin2 * dot1 * dot2 {
        let mut cross_ca = [0.0; 3];
        i::cross(&mut cross_ca, &cd, &ca);
        let s = clip(u::dot3(&cross, &cross_ca) / det, 0.0, 1.0);
        clamp_seg(
            (&mut w[..2]).try_into().unwrap(),
            s,
            (dot5 * s + dot3) / dot2,
            s0,
            s1,
        );
        return 1;
    }
    clamp_seg((&mut w[..2]).try_into().unwrap(), 0.0, dot3 / dot2, s0, s1);
    clamp_seg(
        (&mut w[2..]).try_into().unwrap(),
        1.0,
        (dot5 + dot3) / dot2,
        s0,
        s1,
    );
    2
}
