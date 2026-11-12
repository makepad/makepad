//! Analytic sphere, capsule and box contacts with boxes.
//!
//! Translated from `src/engine/engine_collision_box.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::data::{Data, PreContact};
use crate::inline::{self as i, at3, at9};
use crate::model::Model;
use crate::types::{MAXVAL, MINVAL};
use crate::util_blas as u;
use crate::util_misc::clip;

fn clamp_vec(vec: &mut [f64], limit: &[f64], n: usize) {
    for i in 0..n {
        if limit[i] > 0.0 {
            vec[i] = clip(vec[i], -limit[i], limit[i]);
        }
    }
}

pub fn raw_sphere_box(
    con: &mut [PreContact],
    margin: f64,
    pos1: &[f64; 3],
    mat1: &[f64; 9],
    size1: &[f64],
    pos2: &[f64; 3],
    mat2: &[f64; 9],
    size2: &[f64],
) -> usize {
    let _ = mat1; // Unused in the C signature.
    let mut k = 0;
    let mut tmp = [0.0; 3];
    let mut center = [0.0; 3];
    let mut clamped = [0.0; 3];
    let mut deepest = [0.0; 3];
    let mut pos = [0.0; 3];
    i::sub3(&mut tmp, pos1, pos2);
    i::mul_mat_tvec3(&mut center, mat2, &tmp);
    i::copy3(&mut clamped, &center);
    clamp_vec(&mut clamped, size2, 3);
    i::copy3(&mut deepest, &center);
    i::sub3(&mut tmp, &clamped, &center);
    let mut dist = u::normalize3(&mut tmp);
    if dist - size1[0] > margin {
        return 0;
    }
    if dist <= MINVAL {
        let mut closest = (size2[0] + size2[1] + size2[2]) * 2.0;
        for i in 0..6 {
            if closest
                > ((if i % 2 != 0 { 1.0 } else { -1.0 }) * size2[i / 2] - center[i / 2]).abs()
            {
                closest =
                    ((if i % 2 != 0 { 1.0 } else { -1.0 }) * size2[i / 2] - center[i / 2]).abs();
                k = i;
            }
        }
        let mut nearest = [0.0; 3];
        nearest[k / 2] = if k % 2 != 0 { -1.0 } else { 1.0 };
        i::copy3(&mut pos, &center);
        i::add_to_scl3(&mut pos, &nearest, (size1[0] - closest) / 2.0);
        i::mul_mat_vec3(&mut con[0].normal, mat2, &nearest);
        dist = -closest;
    } else {
        i::add_to_scl3(&mut deepest, &tmp, size1[0]);
        u::zero3(&mut pos);
        i::add_to_scl3(&mut pos, &clamped, 0.5);
        i::add_to_scl3(&mut pos, &deepest, 0.5);
        i::mul_mat_vec3(&mut con[0].normal, mat2, &tmp);
    }
    i::mul_mat_vec3(&mut tmp, mat2, &pos);
    i::add3(&mut con[0].pos, &tmp, pos2);
    con[0].dist = dist - size1[0];
    i::zero3(&mut con[0].tangent);
    1
}

pub fn sphere_box(
    m: &Model,
    d: &Data,
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
    raw_sphere_box(con, margin, pos1, mat1, size1, pos2, mat2, size2)
}

// Find the closest point of the capsule segment to the box, then a second
// contact when the segment has a sufficiently low angle to a box face or edge.
pub fn raw_capsule_box(
    con: &mut [PreContact],
    margin: f64,
    pos1: &[f64; 3],
    mat1: &[f64; 9],
    size1: &[f64],
    pos2: &[f64; 3],
    mat2: &[f64; 9],
    size2: &[f64],
) -> usize {
    let mut tmp1 = [0.0; 3];
    let mut tmp2 = [0.0; 3];
    let mut tmp3 = [0.0; 3];
    let mut halfaxis = [0.0; 3];
    let mut axis = [0.0; 3];
    let mut dif = [0.0; 3];
    let mut pos = [0.0; 3];
    let mut bestboxpos = 0.0;
    let mut c1: i32;
    let mut cltype = -4;
    let mut clface = -1;
    let mut clcorner = 0;
    let mut cledge = 0;
    let halflength = size1[1];
    let mut secondpos = -4.0;
    i::sub3(&mut tmp1, pos1, pos2);
    i::mul_mat_tvec3(&mut pos, mat2, &tmp1);
    tmp1[0] = mat1[2];
    tmp1[1] = mat1[5];
    tmp1[2] = mat1[8];
    i::mul_mat_tvec3(&mut axis, mat2, &tmp1);
    i::scl3(&mut halfaxis, &axis, halflength);
    let mut axisdir = 0;
    if halfaxis[0] > 0.0 {
        axisdir += 1;
    }
    if halfaxis[1] > 0.0 {
        axisdir += 2;
    }
    if halfaxis[2] > 0.0 {
        axisdir += 4;
    }
    let mut bestdistmax = margin + 2.0 * (size1[0] + halflength + size2[0] + size2[1] + size2[2]);
    bestdistmax *= bestdistmax;
    let mut bestdist = bestdistmax;
    let mut bestsegmentpos = 0.0;
    u::zero3(&mut tmp2);
    for i in (-1..=1).step_by(2) {
        i::copy3(&mut tmp1, &pos);
        i::add_to_scl3(&mut tmp1, &halfaxis, i as f64);
        i::copy3(&mut tmp2, &tmp1);
        c1 = 0;
        let mut c2 = -1;
        for j in 0..3 {
            if tmp1[j] < -size2[j] {
                c1 += 1;
                c2 = j as i32;
                tmp1[j] = -size2[j];
            } else if tmp1[j] > size2[j] {
                c1 += 1;
                c2 = j as i32;
                tmp1[j] = size2[j];
            }
        }
        if c1 > 1 {
            continue;
        }
        i::sub_from3(&mut tmp1, &tmp2);
        let dist = u::dot3(&tmp1, &tmp1);
        if dist < bestdist {
            bestdist = dist;
            bestsegmentpos = i as f64;
            cltype = -2 + i;
            clface = c2;
        }
    }
    u::zero3(&mut tmp2);
    for j in 0..3 {
        for i in 0..8 {
            if i & (1 << j) == 0 {
                tmp3[0] = (if i & 1 != 0 { 1.0 } else { -1.0 }) * size2[0];
                tmp3[1] = (if i & 2 != 0 { 1.0 } else { -1.0 }) * size2[1];
                tmp3[2] = (if i & 4 != 0 { 1.0 } else { -1.0 }) * size2[2];
                tmp3[j] = 0.0;
                i::sub3(&mut dif, &tmp3, &pos);
                let ma = size2[j] * size2[j];
                let mb = -size2[j] * halfaxis[j];
                let mc = size1[1] * size1[1];
                let u = -size2[j] * dif[j];
                let v = u::dot3(&halfaxis, &dif);
                let det = ma * mc - mb * mb;
                if det.abs() < MINVAL {
                    continue;
                }
                let idet = 1.0 / det;
                let mut x1 = (mc * u - mb * v) * idet;
                let mut x2 = (ma * v - mb * u) * idet;
                let mut s2 = 1;
                let mut s1 = s2;
                if x1 > 1.0 {
                    x1 = 1.0;
                    s1 = 2;
                    x2 = (v - mb) * (1.0 / mc);
                } else if x1 < -1.0 {
                    x1 = -1.0;
                    s1 = 0;
                    x2 = (v + mb) * (1.0 / mc);
                }
                if x2 > 1.0 {
                    x2 = 1.0;
                    s2 = 2;
                    x1 = (u - mb) * (1.0 / ma);
                    if x1 > 1.0 {
                        x1 = 1.0;
                        s1 = 2;
                    } else if x1 < -1.0 {
                        x1 = -1.0;
                        s1 = 0;
                    }
                } else if x2 < -1.0 {
                    x2 = -1.0;
                    s2 = 0;
                    x1 = (u + mb) * (1.0 / ma);
                    if x1 > 1.0 {
                        x1 = 1.0;
                        s1 = 2;
                    } else if x1 < -1.0 {
                        x1 = -1.0;
                        s1 = 0;
                    }
                }
                i::sub3(&mut dif, &tmp3, &pos);
                i::add_to_scl3(&mut dif, &halfaxis, -x2);
                dif[j] += size2[j] * x1;
                tmp1[2] = u::dot3(&dif, &dif);
                c1 = s1 * 3 + s2;
                if tmp1[2] < bestdist - MINVAL {
                    bestdist = tmp1[2];
                    bestsegmentpos = x2;
                    bestboxpos = x1;
                    let c2 = c1 / 6;
                    clcorner = i + (1 << j) * c2;
                    cledge = j;
                    cltype = c1;
                }
            }
        }
    }

    // Preserve the C's otherwise discarded diagnostic calculations. Its d2 union
    // is represented by [x, y]; explicit reads retain dead stores without allows.
    for j in 0..3 {
        if j == 2 {
            bestdist = bestdistmax;
            let _ = bestdist;
            let p = [pos[0], pos[1]];
            let dd = [halfaxis[0], halfaxis[1]];
            let s = [size2[0], size2[1]];
            let l = (dd[0] * dd[0] + dd[1] * dd[1]).sqrt();
            let uu = dd[0] * s[1];
            let vv = dd[1] * s[0];
            let w = dd[0] * p[1] - dd[1] * p[0];
            let mut best = -1.0;
            let mut ee1 = uu - vv;
            if (ee1 < 0.0) == (w < 0.0) {
                if best < ee1.abs() {
                    best = ee1.abs();
                    c1 = 0;
                    let _ = c1;
                }
            }
            ee1 = -uu - vv;
            if (ee1 < 0.0) == (w < 0.0) {
                if best < ee1.abs() {
                    best = ee1.abs();
                    c1 = 1;
                    let _ = c1;
                }
            }
            ee1 = uu + vv;
            if (ee1 < 0.0) == (w < 0.0) {
                if best < ee1.abs() {
                    best = ee1.abs();
                    c1 = 2;
                    let _ = c1;
                }
            }
            ee1 = -uu + vv;
            if (ee1 < 0.0) == (w < 0.0) {
                if best < ee1.abs() {
                    best = ee1.abs();
                    c1 = 3;
                    let _ = (best, c1);
                }
            }
            ee1 = w.abs() / l;
            let _ = ee1;
            ee1 = dd[0] * dd[0] + dd[1] * dd[1];
            let _ = ee1;
            ee1 = p[0] + (s[1] - p[1]) / dd[1] * dd[0];
            let _ = ee1;
        }
    }
    if cltype == -4 {
        return 0;
    }
    // Breaking this block is the C's goto skip.
    'skip: {
        if cltype >= 0 && cltype / 3 != 1 {
            c1 = axisdir ^ clcorner;
            if c1 == 0 || c1 == 7 {
                break 'skip;
            }
            let mut mul = 0.0;
            let mut de = 0.0;
            let mut dp = 0.0;
            if c1 == 1 || c1 == 2 || c1 == 4 {
                mul = 1.0;
                de = 1.0 - bestsegmentpos;
                dp = 1.0 + bestsegmentpos;
            }
            if c1 == 3 || c1 == 5 || c1 == 6 {
                mul = -1.0;
                c1 = 7 - c1;
                dp = 1.0 - bestsegmentpos;
                de = 1.0 + bestsegmentpos;
            }
            let mut ax = 0;
            let mut ax1 = 0;
            let mut ax2 = 0;
            if c1 == 1 {
                ax = 0;
                ax1 = 1;
                ax2 = 2;
            }
            if c1 == 2 {
                ax = 1;
                ax1 = 2;
                ax2 = 0;
            }
            if c1 == 4 {
                ax = 2;
                ax1 = 0;
                ax2 = 1;
            }
            if axis[ax] * axis[ax] > 0.5 {
                secondpos = de;
                let e1 = 2.0 * size2[ax] / halfaxis[ax].abs();
                if e1 < secondpos {
                    secondpos = e1;
                }
                secondpos *= mul;
            } else {
                secondpos = dp;
                let mut e1 = 2.0 * size2[ax1] / halfaxis[ax1].abs();
                if e1 < secondpos {
                    secondpos = e1;
                }
                e1 = 2.0 * size2[ax2] / halfaxis[ax2].abs();
                if e1 < secondpos {
                    secondpos = e1;
                }
                secondpos *= -mul;
            }
        } else if cltype >= 0 && cltype / 3 == 1 {
            c1 = axisdir ^ clcorner;
            c1 &= 7 - (1 << cledge);
            if c1 != 1 && c1 != 2 && c1 != 4 {
                break 'skip;
            }
            let mut ax1 = 0;
            let mut ax2 = 0;
            if cledge == 0 {
                ax1 = 1;
                ax2 = 2;
            }
            if cledge == 1 {
                ax1 = 2;
                ax2 = 0;
            }
            if cledge == 2 {
                ax1 = 0;
                ax2 = 1;
            }
            let ax = cledge;
            if axis[ax1].abs() > axis[ax2].abs() {
                ax1 = ax2;
            }
            ax2 = 3 - ax - ax1;
            let mul;
            if c1 & (1 << ax2) != 0 {
                mul = 1.0;
                secondpos = 1.0 - bestsegmentpos;
            } else {
                mul = -1.0;
                secondpos = 1.0 + bestsegmentpos;
            }
            let mut e1 = 2.0 * size2[ax2] / halfaxis[ax2].abs();
            if e1 < secondpos {
                secondpos = e1;
            }
            let e2 = if (axisdir & (1 << ax) != 0) == (c1 & (1 << ax2) != 0) {
                1.0 - bestboxpos
            } else {
                1.0 + bestboxpos
            };
            e1 = size2[ax] * e2 / halfaxis[ax].abs();
            if e1 < secondpos {
                secondpos = e1;
            }
            secondpos *= mul;
        } else if cltype < 0 {
            if clface == -1 {
                break 'skip;
            }
            let mul = if cltype == -3 { 1.0 } else { -1.0 };
            secondpos = 2.0;
            i::copy3(&mut tmp1, &pos);
            i::add_to_scl3(&mut tmp1, &halfaxis, -mul);
            for i in 0..3 {
                if i as i32 != clface {
                    let mut e1 = (size2[i] - tmp1[i]) / halfaxis[i] * mul;
                    if e1 > 0.0 {
                        if e1 < secondpos {
                            secondpos = e1;
                        }
                    }
                    e1 = (-size2[i] - tmp1[i]) / halfaxis[i] * mul;
                    if e1 > 0.0 {
                        if e1 < secondpos {
                            secondpos = e1;
                        }
                    }
                }
            }
            secondpos *= mul;
        }
    }
    u::copy3(&mut tmp1, &pos);
    i::add_to_scl3(&mut tmp1, &halfaxis, bestsegmentpos);
    u::mul_mat_vec3(&mut tmp2, mat2, &tmp1);
    u::add_to3(&mut tmp2, pos2);
    let mut n = raw_sphere_box(con, margin, &tmp2, mat1, size1, pos2, mat2, size2);
    if secondpos > -3.0 {
        u::copy3(&mut tmp1, &pos);
        i::add_to_scl3(&mut tmp1, &halfaxis, secondpos + bestsegmentpos);
        u::mul_mat_vec3(&mut tmp2, mat2, &tmp1);
        u::add_to3(&mut tmp2, pos2);
        n += raw_sphere_box(&mut con[n..], margin, &tmp2, mat1, size1, pos2, mat2, size2);
    }
    n
}

pub fn capsule_box(
    m: &Model,
    d: &Data,
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
    raw_capsule_box(con, margin, pos1, mat1, size1, pos2, mat2, size2)
}

const BOXBOX_SEPEPS: f64 = 1e-13;
const BOXBOX_PAREPS: f64 = 1e-16;
const BOXBOX_SGNEPS: f64 = 1e-9;
const BOXBOX_DUPEPS: f64 = 1e-14;
const BOXBOX_EDGEBIAS: f64 = 1e-6;
const BOXBOX_MAXVERT: usize = 12;

// Clip against sign*v[coord] <= limit; swap buffers only when clipping occurs.
fn clip_half_plane<'a>(
    nin: usize,
    cur: &mut &'a mut [[f64; 3]; BOXBOX_MAXVERT],
    spare: &mut &'a mut [[f64; 3]; BOXBOX_MAXVERT],
    coord: usize,
    sign: f64,
    limit: f64,
) -> usize {
    let mut d = [0.0; BOXBOX_MAXVERT];
    let mut all_inside = true;
    for k in 0..nin {
        d[k] = sign * cur[k][coord] - limit;
        all_inside &= d[k] <= 0.0;
    }
    if all_inside {
        return nin;
    }
    let mut nout = 0;
    for k in 0..nin {
        let p = &cur[k];
        let k1 = if k + 1 == nin { 0 } else { k + 1 };
        let dp = d[k];
        let dq = d[k1];
        if dp <= 0.0 && nout < BOXBOX_MAXVERT {
            i::copy3(&mut spare[nout], p);
            nout += 1;
        }
        if ((dp < 0.0 && dq > 0.0) || (dp > 0.0 && dq < 0.0)) && nout < BOXBOX_MAXVERT {
            let q = &cur[k1];
            let t = dp / (dp - dq);
            spare[nout][0] = p[0] + t * (q[0] - p[0]);
            spare[nout][1] = p[1] + t * (q[1] - p[1]);
            spare[nout][2] = p[2] + t * (q[2] - p[2]);
            nout += 1;
        }
    }
    std::mem::swap(cur, spare);
    nout
}

// Separating-axis test followed by an edge pair or clipped face manifold.
pub fn raw_box_box(
    con: &mut [PreContact],
    margin: f64,
    pos1: &[f64; 3],
    mat1: &[f64; 9],
    size1: &[f64],
    pos2: &[f64; 3],
    mat2: &[f64; 9],
    size2: &[f64],
) -> usize {
    let mut rot = [0.0; 9];
    let mut rotabs = [0.0; 9];
    let mut pos21 = [0.0; 3];
    let mut pos12 = [0.0; 3];
    let mut tmp = [0.0; 3];
    i::sub3(&mut tmp, pos2, pos1);
    i::mul_mat_tvec3(&mut pos21, mat1, &tmp);
    i::sub3(&mut tmp, pos1, pos2);
    i::mul_mat_tvec3(&mut pos12, mat2, &tmp);
    u::mul_mat_tmat3(&mut rot, mat1, mat2);
    for i in 0..9 {
        rotabs[i] = rot[i].abs();
    }
    let septol =
        margin + BOXBOX_SEPEPS * (size1[0] + size1[1] + size1[2] + size2[0] + size2[1] + size2[2]);
    let mut sep_best = -MAXVAL;
    let mut sep_face = -MAXVAL;
    let _ = sep_face; // The C initializes this before replacing it after the face tests.
    let mut code: i32 = -1;
    for i in 0..3 {
        let radius2 = rotabs[3 * i + 0] * size2[0]
            + rotabs[3 * i + 1] * size2[1]
            + rotabs[3 * i + 2] * size2[2];
        let sep = pos21[i].abs() - size1[i] - radius2;
        if sep > septol {
            return 0;
        }
        if sep > sep_best {
            sep_best = sep;
            code = i as i32;
        }
    }
    for j in 0..3 {
        let radius1 =
            rotabs[0 + j] * size1[0] + rotabs[3 + j] * size1[1] + rotabs[6 + j] * size1[2];
        let sep = pos12[j].abs() - size2[j] - radius1;
        if sep > septol {
            return 0;
        }
        if sep > sep_best {
            sep_best = sep;
            code = 3 + j as i32;
        }
    }
    sep_face = sep_best;
    let code_face = code;
    for i in 0..3 {
        for j in 0..3 {
            let i1 = (i + 1) % 3;
            let i2 = (i + 2) % 3;
            let mut ax1 = -rot[3 * i2 + j];
            let mut ax2 = rot[3 * i1 + j];
            let norm2 = ax1 * ax1 + ax2 * ax2;
            if norm2 < BOXBOX_PAREPS {
                continue;
            }
            let inv = 1.0 / norm2.sqrt();
            ax1 *= inv;
            ax2 *= inv;
            let radius1 = size1[i1] * ax1.abs() + size1[i2] * ax2.abs();
            let j1 = (j + 1) % 3;
            let j2 = (j + 2) % 3;
            let a2_1 = ax1 * rot[3 * i1 + j1] + ax2 * rot[3 * i2 + j1];
            let a2_2 = ax1 * rot[3 * i1 + j2] + ax2 * rot[3 * i2 + j2];
            let radius2 = size2[j1] * a2_1.abs() + size2[j2] * a2_2.abs();
            let sep = (ax1 * pos21[i1] + ax2 * pos21[i2]).abs() - radius1 - radius2;
            if sep > septol {
                return 0;
            }
            if sep - BOXBOX_EDGEBIAS * sep.abs() > sep_best && sep > sep_face {
                sep_best = sep;
                code = (6 + 3 * i + j) as i32;
            }
        }
    }
    if code < 0 {
        return 0;
    }
    // Substitute the best face for an almost parallel edge axis on a near tie.
    if code >= 6 {
        let i = ((code - 6) / 3) as usize;
        let j = ((code - 6) % 3) as usize;
        let i1 = (i + 1) % 3;
        let i2 = (i + 2) % 3;
        let mut axis = [0.0; 3];
        axis[i] = 0.0;
        axis[i1] = -rot[3 * i2 + j];
        axis[i2] = rot[3 * i1 + j];
        u::normalize3(&mut axis);
        let face_dot;
        if code_face < 3 {
            face_dot = axis[code_face as usize].abs();
        } else {
            let f = (code_face - 3) as usize;
            face_dot = (axis[0] * rot[0 + f] + axis[1] * rot[3 + f] + axis[2] * rot[6 + f]).abs();
        }
        if face_dot > 0.99 && sep_best < sep_face + 0.05 * sep_face.abs() + MINVAL {
            code = code_face;
            sep_best = sep_face;
            let _ = sep_best; // The C retains this assignment after its final comparison.
        }
    }
    if code >= 6 {
        let i = ((code - 6) / 3) as usize;
        let j = ((code - 6) % 3) as usize;
        let i1 = (i + 1) % 3;
        let i2 = (i + 2) % 3;
        let j1 = (j + 1) % 3;
        let j2 = (j + 2) % 3;
        let mut axis = [0.0; 3];
        axis[i] = 0.0;
        axis[i1] = -rot[3 * i2 + j];
        axis[i2] = rot[3 * i1 + j];
        u::normalize3(&mut axis);
        if u::dot3(&axis, &pos21) < 0.0 {
            axis[0] = -axis[0];
            axis[1] = -axis[1];
            axis[2] = -axis[2];
        }
        let a2 = [
            axis[0] * rot[0 + 0] + axis[1] * rot[3 + 0] + axis[2] * rot[6 + 0],
            axis[0] * rot[0 + 1] + axis[1] * rot[3 + 1] + axis[2] * rot[6 + 1],
            axis[0] * rot[0 + 2] + axis[1] * rot[3 + 2] + axis[2] * rot[6 + 2],
        ];
        let ambig = BOXBOX_SGNEPS;
        let mut amb1: i32 = -1;
        let mut amb2: i32 = -1;
        if axis[i1].abs() < ambig {
            amb1 = i1 as i32;
        } else if axis[i2].abs() < ambig {
            amb1 = i2 as i32;
        }
        if a2[j1].abs() < ambig {
            amb2 = j1 as i32;
        } else if a2[j2].abs() < ambig {
            amb2 = j2 as i32;
        }
        let d2 = [rot[0 + j], rot[3 + j], rot[6 + j]];
        let b = d2[i];
        let denom = 1.0 - b * b;
        let mut w1 = [0.0; 3];
        let mut w2 = [0.0; 3];
        let mut best_d2 = MAXVAL;
        for v1 in 0..(if amb1 >= 0 { 2 } else { 1 }) {
            for v2 in 0..(if amb2 >= 0 { 2 } else { 1 }) {
                let mut c1 = [0.0; 3];
                c1[i] = 0.0;
                c1[i1] = if axis[i1] >= 0.0 {
                    size1[i1]
                } else {
                    -size1[i1]
                };
                c1[i2] = if axis[i2] >= 0.0 {
                    size1[i2]
                } else {
                    -size1[i2]
                };
                if amb1 >= 0 && v1 != 0 {
                    c1[amb1 as usize] = -c1[amb1 as usize];
                }
                let mut cc = [0.0; 3];
                cc[j] = 0.0;
                cc[j1] = if a2[j1] >= 0.0 { -size2[j1] } else { size2[j1] };
                cc[j2] = if a2[j2] >= 0.0 { -size2[j2] } else { size2[j2] };
                if amb2 >= 0 && v2 != 0 {
                    cc[amb2 as usize] = -cc[amb2 as usize];
                }
                let mut c2 = [0.0; 3];
                i::mul_mat_vec3(&mut c2, &rot, &cc);
                i::add_to3(&mut c2, &pos21);
                let mut e = [0.0; 3];
                i::sub3(&mut e, &c2, &c1);
                let d1e = e[i];
                let d2e = u::dot3(&d2, &e);
                let mut s = if denom < MINVAL {
                    0.0
                } else {
                    (d1e - b * d2e) / denom
                };
                s = clip(s, -size1[i], size1[i]);
                let t = clip(b * s - d2e, -size2[j], size2[j]);
                s = clip(d1e + b * t, -size1[i], size1[i]);
                let mut p1 = [0.0; 3];
                let mut p2 = [0.0; 3];
                let mut gap = [0.0; 3];
                i::copy3(&mut p1, &c1);
                p1[i] += s;
                i::copy3(&mut p2, &c2);
                i::add_to_scl3(&mut p2, &d2, t);
                i::sub3(&mut gap, &p2, &p1);
                let gap2 = u::dot3(&gap, &gap);
                if gap2 < best_d2 {
                    best_d2 = gap2;
                    i::copy3(&mut w1, &p1);
                    i::copy3(&mut w2, &p2);
                }
            }
        }
        let mut gap = [0.0; 3];
        i::sub3(&mut gap, &w2, &w1);
        let dist = u::dot3(&gap, &axis);
        if dist > septol {
            return 0;
        }
        let mid = [
            0.5 * (w1[0] + w2[0]),
            0.5 * (w1[1] + w2[1]),
            0.5 * (w1[2] + w2[2]),
        ];
        con[0].dist = dist;
        i::mul_mat_vec3(&mut tmp, mat1, &mid);
        i::add3(&mut con[0].pos, &tmp, pos1);
        i::mul_mat_vec3(&mut con[0].normal, mat1, &axis);
        i::zero3(&mut con[0].tangent);
        return 1;
    }
    let ref1 = code < 3;
    let a = (if ref1 { code } else { code - 3 }) as usize;
    let sizeref = if ref1 { size1 } else { size2 };
    let sizeinc = if ref1 { size2 } else { size1 };
    let posref = if ref1 { pos1 } else { pos2 };
    let matref = if ref1 { mat1 } else { mat2 };
    let posoi = if ref1 { &pos21 } else { &pos12 };
    let mut rinc = [0.0; 9];
    if ref1 {
        u::copy(&mut rinc, &rot, 9);
    } else {
        u::transpose(&mut rinc, &rot, 3, 3);
    }
    let sgn = if posoi[a] >= 0.0 { 1.0 } else { -1.0 };
    let mut binc = 0;
    for k in 1..3 {
        if rinc[3 * a + k].abs() > rinc[3 * a + binc].abs() {
            binc = k;
        }
    }
    let tinc = if sgn * rinc[3 * a + binc] > 0.0 {
        -1.0
    } else {
        1.0
    };
    let ax = (a + 1) % 3;
    let ay = (a + 2) % 3;
    let bu = (binc + 1) % 3;
    let bv = (binc + 2) % 3;
    let mut poly = [[[0.0; 3]; BOXBOX_MAXVERT]; 2];
    let mut cx = [0.0; 3];
    let mut du = [0.0; 3];
    let mut dv = [0.0; 3];
    for r in 0..3 {
        let c = if r == 0 {
            ax
        } else {
            if r == 1 {
                ay
            } else {
                a
            }
        };
        cx[r] = posoi[c] + tinc * sizeinc[binc] * rinc[3 * c + binc];
        du[r] = sizeinc[bu] * rinc[3 * c + bu];
        dv[r] = sizeinc[bv] * rinc[3 * c + bv];
    }
    cx[2] = sgn * cx[2] - sizeref[a];
    du[2] *= sgn;
    dv[2] *= sgn;
    let corner_sign: [[f64; 2]; 4] = [[1.0, 1.0], [-1.0, 1.0], [-1.0, -1.0], [1.0, -1.0]];
    for k in 0..4 {
        let su = corner_sign[k][0];
        let sv = corner_sign[k][1];
        poly[0][k][0] = cx[0] + su * du[0] + sv * dv[0];
        poly[0][k][1] = cx[1] + su * du[1] + sv * dv[1];
        poly[0][k][2] = cx[2] + su * du[2] + sv * dv[2];
    }
    let mut nvert = 4;
    let (first, second) = poly.split_at_mut(1);
    let mut cur = &mut first[0];
    let mut spare = &mut second[0];
    nvert = clip_half_plane(nvert, &mut cur, &mut spare, 0, 1.0, sizeref[ax]);
    nvert = clip_half_plane(nvert, &mut cur, &mut spare, 0, -1.0, sizeref[ax]);
    nvert = clip_half_plane(nvert, &mut cur, &mut spare, 1, 1.0, sizeref[ay]);
    nvert = clip_half_plane(nvert, &mut cur, &mut spare, 1, -1.0, sizeref[ay]);
    let mut accepted = [[0.0; 3]; BOXBOX_MAXVERT];
    let mut naccept = 0;
    let dupe2 = BOXBOX_DUPEPS * (sizeref[ax] * sizeref[ax] + sizeref[ay] * sizeref[ay]);
    for k in 0..nvert {
        if cur[k][2] > margin {
            continue;
        }
        let mut dupe = false;
        for q in 0..naccept {
            let dx = accepted[q][0] - cur[k][0];
            let dy = accepted[q][1] - cur[k][1];
            if dx * dx + dy * dy < dupe2 {
                dupe = true;
                break;
            }
        }
        if !dupe {
            i::copy3(&mut accepted[naccept], &cur[k]);
            naccept += 1;
        }
    }
    if naccept == 0 {
        return 0;
    }
    let mut normal = [0.0; 3];
    let nsign = if ref1 { sgn } else { -sgn };
    normal[0] = nsign * matref[3 * 0 + a];
    normal[1] = nsign * matref[3 * 1 + a];
    normal[2] = nsign * matref[3 * 2 + a];
    for k in 0..naccept {
        let v = &accepted[k];
        let mut posc = [0.0; 3];
        posc[ax] = v[0];
        posc[ay] = v[1];
        posc[a] = sgn * (sizeref[a] + 0.5 * v[2]);
        con[k].dist = v[2];
        i::mul_mat_vec3(&mut tmp, matref, &posc);
        i::add3(&mut con[k].pos, &tmp, posref);
        i::copy3(&mut con[k].normal, &normal);
        i::zero3(&mut con[k].tangent);
    }
    naccept
}

pub fn box_box(
    m: &Model,
    d: &Data,
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
    raw_box_box(con, margin, pos1, mat1, size1, pos2, mat2, size2)
}
