//! Signed-distance collision queries and contact generation.
//!
//! Translated from `src/engine/engine_collision_sdf.c` and its header of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust; Sdf replaces plugin callbacks; a mesh face against an SDF gives one start's
//! deepest point and the deepest faces are kept (see `process_sdf_corners`); an SDF meets another SDF or a
//! primitive through our own search (`sdf`, `walk_midsurface`, `touch_extent`): the reference walks each start down
//! `d1 + d2 + |max(d1, d2)|`, which is the smaller distance where both are inside, so its points end at the
//! overlap's rim or tips, scattered from step to step, and report `max(d1, d2)` there (at most half the
//! overlap): a resting part never settled and a part held hard against another slipped into it.

use crate::data::{Data, PreContact};
use crate::inline::{at3, at3_mut, at4, at6};
use crate::model::Model;
use crate::sdf::Sdf;
use crate::types::*;
use crate::util_blas::*;
use crate::util_misc::{clip, halton, max, min};
use crate::util_spatial::{cross, mul_pose, neg_pose, quat2mat, rot_vec_quat};

// mjSDF from mjplugin.h; the callback trait retains the name Sdf.
pub struct SdfQuery<'a> {
    pub plugin: &'a [Option<&'a dyn Sdf>],
    pub id: &'a [i32],
    pub r#type: i32,
    pub relpos: &'a [f64; 3],
    pub relmat: &'a [f64; 9],
    pub geomtype: &'a [i32],
}

pub fn box_projection(point: &mut [f64; 3], box_: &[f64; 6]) -> f64 {
    let r = [point[0] - box_[0], point[1] - box_[1], point[2] - box_[2]];
    let q = [
        r[0].abs() - box_[3],
        r[1].abs() - box_[4],
        r[2].abs() - box_[5],
    ];
    let mut dist_sqr = 0.0;
    let eps = 1e-6;
    if q[0] <= 0.0 && q[1] <= 0.0 && q[2] <= 0.0 {
        return max(q[0], max(q[1], q[2]));
    }
    if q[0] >= 0.0 {
        dist_sqr += q[0] * q[0];
        point[0] -= if r[0] > 0.0 {
            q[0] + eps
        } else {
            -(q[0] + eps)
        };
    }
    if q[1] >= 0.0 {
        dist_sqr += q[1] * q[1];
        point[1] -= if r[1] > 0.0 {
            q[1] + eps
        } else {
            -(q[1] + eps)
        };
    }
    if q[2] >= 0.0 {
        dist_sqr += q[2] * q[2];
        point[2] -= if r[2] > 0.0 {
            q[2] + eps
        } else {
            -(q[2] + eps)
        };
    }
    dist_sqr.sqrt()
}

fn find_oct(
    mut w: Option<&mut [f64; 8]>,
    mut dw: Option<&mut [[f64; 3]; 8]>,
    oct_aabb: &[f64],
    oct_child: &[i32],
    p: &[f64; 3],
) -> usize {
    let mut stack = 0_i32;
    let eps = 1e-8;
    let mut niter = 100;
    while niter > 0 {
        niter -= 1;
        let node = stack;
        let mut vmin = [0.0; 3];
        let mut vmax = [0.0; 3];
        if node == -1 {
            panic!("Invalid node number");
        }
        let node = node as usize;
        for j in 0..3 {
            vmin[j] = oct_aabb[6 * node + j] - oct_aabb[6 * node + 3 + j];
            vmax[j] = oct_aabb[6 * node + j] + oct_aabb[6 * node + 3 + j];
        }
        if p[0] + eps < vmin[0]
            || p[0] - eps > vmax[0]
            || p[1] + eps < vmin[1]
            || p[1] - eps > vmax[1]
            || p[2] + eps < vmin[2]
            || p[2] - eps > vmax[2]
        {
            continue;
        }
        let coord = [
            (p[0] - vmin[0]) / (vmax[0] - vmin[0]),
            (p[1] - vmin[1]) / (vmax[1] - vmin[1]),
            (p[2] - vmin[2]) / (vmax[2] - vmin[2]),
        ];
        if oct_child[8 * node] == -1
            && oct_child[8 * node + 1] == -1
            && oct_child[8 * node + 2] == -1
            && oct_child[8 * node + 3] == -1
            && oct_child[8 * node + 4] == -1
            && oct_child[8 * node + 5] == -1
            && oct_child[8 * node + 6] == -1
            && oct_child[8 * node + 7] == -1
        {
            let inv_cell = [
                1.0 / (vmax[0] - vmin[0]),
                1.0 / (vmax[1] - vmin[1]),
                1.0 / (vmax[2] - vmin[2]),
            ];
            for j in 0..8 {
                if let Some(w) = w.as_deref_mut() {
                    w[j] = (if j & 1 != 0 { coord[0] } else { 1.0 - coord[0] })
                        * (if j & 2 != 0 { coord[1] } else { 1.0 - coord[1] })
                        * (if j & 4 != 0 { coord[2] } else { 1.0 - coord[2] });
                }
                if let Some(dw) = dw.as_deref_mut() {
                    dw[j][0] = (if j & 1 != 0 { 1.0 } else { -1.0 })
                        * (if j & 2 != 0 { coord[1] } else { 1.0 - coord[1] })
                        * (if j & 4 != 0 { coord[2] } else { 1.0 - coord[2] })
                        * inv_cell[0];
                    dw[j][1] = (if j & 1 != 0 { coord[0] } else { 1.0 - coord[0] })
                        * (if j & 2 != 0 { 1.0 } else { -1.0 })
                        * (if j & 4 != 0 { coord[2] } else { 1.0 - coord[2] })
                        * inv_cell[1];
                    dw[j][2] = (if j & 1 != 0 { coord[0] } else { 1.0 - coord[0] })
                        * (if j & 2 != 0 { coord[1] } else { 1.0 - coord[1] })
                        * (if j & 4 != 0 { 1.0 } else { -1.0 })
                        * inv_cell[2];
                }
            }
            return node;
        }
        let x = if coord[0] < 0.5 { 0 } else { 1 };
        let y = if coord[1] < 0.5 { 0 } else { 1 };
        let z = if coord[2] < 0.5 { 0 } else { 1 };
        stack = oct_child[8 * node + 4 * z + 2 * y + x];
    }
    panic!("Node not found");
}

pub fn oct_distance(m: &Model, p: &[f64; 3], meshid: i32) -> f64 {
    let octadr = m.mesh_octadr[meshid as usize];
    if octadr == -1 {
        panic!("Octree not found in mesh {}", meshid);
    }
    let octadr = octadr as usize;
    let oct_child = &m.oct_child[8 * octadr..];
    let oct_aabb = &m.oct_aabb[6 * octadr..];
    let oct_coeff = &m.oct_coeff[8 * octadr..];
    let mut w = [0.0; 8];
    let mut sdf = 0.0;
    let mut point = [p[0], p[1], p[2]];
    let box_dist = box_projection(&mut point, at6(oct_aabb, 0));
    let node = find_oct(Some(&mut w), None, oct_aabb, oct_child, &point);
    for i in 0..8 {
        sdf += w[i] * oct_coeff[8 * node + i];
    }
    if box_dist > 0.0 {
        sdf + box_dist
    } else {
        sdf
    }
}

pub fn oct_gradient(m: &Model, grad: &mut [f64; 3], point: &[f64; 3], meshid: i32) {
    zero3(grad);
    let mut p = [point[0], point[1], point[2]];
    let octadr = m.mesh_octadr[meshid as usize];
    if octadr == -1 {
        panic!("Octree not found in mesh {}", meshid);
    }
    let octadr = octadr as usize;
    let oct_child = &m.oct_child[8 * octadr..];
    let oct_aabb = &m.oct_aabb[6 * octadr..];
    let oct_coeff = &m.oct_coeff[8 * octadr..];
    if box_projection(&mut p, at6(oct_aabb, 0)) <= 0.0 {
        let mut dw = [[0.0; 3]; 8];
        let node = find_oct(None, Some(&mut dw), oct_aabb, oct_child, &p);
        for i in 0..8 {
            grad[0] += dw[i][0] * oct_coeff[8 * node + i];
            grad[1] += dw[i][1] * oct_coeff[8 * node + i];
            grad[2] += dw[i][2] * oct_coeff[8 * node + i];
        }
        return;
    }
    let eps = 1e-8;
    let dist0 = oct_distance(m, point, meshid);
    let point_x = [point[0] + eps, point[1], point[2]];
    let dist_x = oct_distance(m, &point_x, meshid);
    let point_y = [point[0], point[1] + eps, point[2]];
    let dist_y = oct_distance(m, &point_y, meshid);
    let point_z = [point[0], point[1], point[2] + eps];
    let dist_z = oct_distance(m, &point_z, meshid);
    grad[0] = (dist_x - dist0) / eps;
    grad[1] = (dist_y - dist0) / eps;
    grad[2] = (dist_z - dist0) / eps;
}

fn radial_field3d(field: &mut [f64; 3], a: &[f64; 3], x: &[f64; 3], size: &[f64; 3]) {
    field[0] = -size[0] / a[0];
    field[1] = -size[1] / a[1];
    field[2] = -size[2] / a[2];
    normalize3(field);
    if x[0] < 0.0 {
        field[0] = -field[0];
    }
    if x[1] < 0.0 {
        field[1] = -field[1];
    }
    if x[2] < 0.0 {
        field[2] = -field[2];
    }
}

fn geom_distance(
    m: &Model,
    _d: &Data,
    p: Option<&dyn Sdf>,
    i: i32,
    x: &[f64; 3],
    r#type: i32,
) -> f64 {
    let mut a = [0.0; 3];
    let mut b = [0.0; 3];
    // C forms the size pointer even for plugin/mesh ids, but dereferences it only for primitives.
    match r#type {
        GEOM_PLANE => x[2],
        GEOM_SPHERE => norm3(x) - m.geom_size[3 * i as usize],
        GEOM_BOX => {
            let size = at3(&m.geom_size, 3 * i as usize);
            a[0] = x[0].abs() - size[0];
            a[1] = x[1].abs() - size[1];
            a[2] = x[2].abs() - size[2];
            if a[0] >= 0.0 || a[1] >= 0.0 || a[2] >= 0.0 {
                b[0] = max(a[0], 0.0);
                b[1] = max(a[1], 0.0);
                b[2] = max(a[2], 0.0);
                return norm3(&b) + min(max(a[0], max(a[1], a[2])), 0.0);
            }
            radial_field3d(&mut b, &a, x, size);
            let t = [-a[0] / b[0].abs(), -a[1] / b[1].abs(), -a[2] / b[2].abs()];
            -min(t[0], min(t[1], t[2])) * norm3(&b)
        }
        GEOM_CAPSULE => {
            let size = at3(&m.geom_size, 3 * i as usize);
            a[0] = x[0];
            a[1] = x[1];
            a[2] = x[2] - clip(x[2], -size[1], size[1]);
            norm3(&a) - size[0]
        }
        GEOM_ELLIPSOID => {
            let size = at3(&m.geom_size, 3 * i as usize);
            a[0] = x[0] / size[0];
            a[1] = x[1] / size[1];
            a[2] = x[2] / size[2];
            b[0] = a[0] / size[0];
            b[1] = a[1] / size[1];
            b[2] = a[2] / size[2];
            let k0 = norm3(&a);
            let k1 = norm3(&b);
            if k1 < MINVAL {
                return -min(size[0], min(size[1], size[2]));
            }
            k0 * (k0 - 1.0) / k1
        }
        GEOM_CYLINDER => {
            let size = at3(&m.geom_size, 3 * i as usize);
            a[0] = (x[0] * x[0] + x[1] * x[1]).sqrt() - size[0];
            a[1] = x[2].abs() - size[1];
            b[0] = max(a[0], 0.0);
            b[1] = max(a[1], 0.0);
            min(max(a[0], a[1]), 0.0) + norm(&b, 2)
        }
        GEOM_SDF => {
            if let Some(p) = p {
                p.distance(x)
            } else {
                oct_distance(m, x, i)
            }
        }
        GEOM_MESH => {
            if m.mesh_octadr[i as usize] == -1 {
                panic!("sdf queries require needsdf=\"true\" on mesh {}", i);
            }
            oct_distance(m, x, i)
        }
        _ => panic!("sdf collisions not available for geom type {}", r#type),
    }
}

fn geom_gradient(
    gradient: &mut [f64; 3],
    m: &Model,
    _d: &Data,
    p: Option<&dyn Sdf>,
    i: i32,
    x: &[f64; 3],
    r#type: i32,
) {
    let mut a = [0.0; 3];
    let mut b = [0.0; 3];
    match r#type {
        GEOM_PLANE => {
            zero3(gradient);
            gradient[2] = 1.0;
        }
        GEOM_SPHERE => {
            copy3(gradient, x);
            let c = max(norm3(x), 1.0 / MAXVAL);
            gradient[0] *= 1.0 / c;
            gradient[1] *= 1.0 / c;
            gradient[2] *= 1.0 / c;
        }
        GEOM_BOX => {
            let size = at3(&m.geom_size, 3 * i as usize);
            zero3(gradient);
            a[0] = x[0].abs() - size[0];
            a[1] = x[1].abs() - size[1];
            a[2] = x[2].abs() - size[2];
            let k = if a[0] > a[1] { 0 } else { 1 };
            let l = if a[2] > a[k] { 2 } else { k };
            if a[l] < 0.0 {
                radial_field3d(gradient, &a, x, size);
            } else {
                b[0] = max(a[0], 0.0);
                b[1] = max(a[1], 0.0);
                b[2] = max(a[2], 0.0);
                let c = norm3(&b);
                if c == 0.0 {
                    for j in 0..3 {
                        if a[j] == 0.0 {
                            gradient[j] = if x[j] < 0.0 { -1.0 } else { 1.0 };
                        }
                    }
                    normalize3(gradient);
                } else {
                    gradient[0] = if a[0] > 0.0 {
                        b[0] / c * x[0] / x[0].abs()
                    } else {
                        0.0
                    };
                    gradient[1] = if a[1] > 0.0 {
                        b[1] / c * x[1] / x[1].abs()
                    } else {
                        0.0
                    };
                    gradient[2] = if a[2] > 0.0 {
                        b[2] / c * x[2] / x[2].abs()
                    } else {
                        0.0
                    };
                }
            }
        }
        GEOM_CAPSULE => {
            let size = at3(&m.geom_size, 3 * i as usize);
            a[0] = x[0];
            a[1] = x[1];
            a[2] = x[2] - clip(x[2], -size[1], size[1]);
            let c = max(norm3(&a), 1.0 / MAXVAL);
            gradient[0] = a[0] / c;
            gradient[1] = a[1] / c;
            gradient[2] = a[2] / c;
        }
        GEOM_ELLIPSOID => {
            let size = at3(&m.geom_size, 3 * i as usize);
            a[0] = x[0] / size[0];
            a[1] = x[1] / size[1];
            a[2] = x[2] / size[2];
            b[0] = a[0] / size[0];
            b[1] = a[1] / size[1];
            b[2] = a[2] / size[2];
            let k0 = norm3(&a);
            let k1 = norm3(&b);
            if k0 < MINVAL || k1 < MINVAL {
                zero3(gradient);
                return;
            }
            let inv_k0 = 1.0 / k0;
            let inv_k1 = 1.0 / k1;
            let gk0 = [b[0] * inv_k0, b[1] * inv_k0, b[2] * inv_k0];
            let gk1 = [
                b[0] * inv_k1 / (size[0] * size[0]),
                b[1] * inv_k1 / (size[1] * size[1]),
                b[2] * inv_k1 / (size[2] * size[2]),
            ];
            let df_dk0 = (2.0 * k0 - 1.0) * inv_k1;
            let df_dk1 = k0 * (k0 - 1.0) * inv_k1 * inv_k1;
            gradient[0] = gk0[0] * df_dk0 - gk1[0] * df_dk1;
            gradient[1] = gk0[1] * df_dk0 - gk1[1] * df_dk1;
            gradient[2] = gk0[2] * df_dk0 - gk1[2] * df_dk1;
            if norm3(gradient) < MINVAL {
                zero3(gradient);
            } else {
                normalize3(gradient);
            }
        }
        GEOM_CYLINDER => {
            let size = at3(&m.geom_size, 3 * i as usize);
            let c = (x[0] * x[0] + x[1] * x[1]).sqrt();
            let e = x[2].abs();
            a[0] = c - size[0];
            a[1] = e - size[1];
            let grada = [
                x[0] / max(c, 1.0 / MAXVAL),
                x[1] / max(c, 1.0 / MAXVAL),
                x[2] / max(e, 1.0 / MAXVAL),
            ];
            let j = if a[0] > a[1] { 0 } else { 1 };
            if a[j] < 0.0 {
                gradient[0] = if j == 0 { grada[0] } else { 0.0 };
                gradient[1] = if j == 0 { grada[1] } else { 0.0 };
                gradient[2] = if j == 1 { grada[2] } else { 0.0 };
            } else {
                b[0] = max(a[0], 0.0);
                b[1] = max(a[1], 0.0);
                let bnorm = max(norm(&b, 2), 1.0 / MAXVAL);
                gradient[0] = grada[0] * b[0] / bnorm;
                gradient[1] = grada[1] * b[0] / bnorm;
                gradient[2] = grada[2] * b[1] / bnorm;
            }
        }
        GEOM_SDF => {
            if let Some(p) = p {
                p.gradient(gradient, x);
            } else {
                oct_gradient(m, gradient, x, i);
            }
        }
        GEOM_MESH => {
            if m.mesh_octadr[i as usize] == -1 {
                panic!("sdf queries require needsdf=\"true\" on mesh {}", i);
            }
            oct_gradient(m, gradient, x, i);
        }
        _ => panic!("sdf collisions not available for geom type {}", r#type),
    }
}

// A geom's distance and gradient at one point: as geom_distance and geom_gradient, in one lookup for a
// plugin field that finds both at once.
fn geom_distance_gradient(gradient: &mut [f64; 3], m: &Model, d: &Data, p: Option<&dyn Sdf>, i: i32, x: &[f64; 3], r#type: i32) -> f64 {
    match (r#type, p) {
        (GEOM_SDF, Some(p)) => p.distance_gradient(gradient, x),
        _ => {
            geom_gradient(gradient, m, d, p, i, x, r#type);
            geom_distance(m, d, p, i, x, r#type)
        }
    }
}

// distance and gradient at one point, the same values as the two give, each geom's field looked up once.
fn distance_gradient(m: &Model, d: &Data, s: &SdfQuery<'_>, gradient: &mut [f64; 3], x: &[f64; 3]) -> f64 {
    let mut y = [0.0; 3];
    let mut grad1 = [0.0; 3];
    let mut grad2 = [0.0; 3];
    match s.r#type {
        SDFTYPE_SINGLE => geom_distance_gradient(gradient, m, d, s.plugin[0], s.id[0], x, s.geomtype[0]),
        SDFTYPE_COLLISION => {
            mul_mat_vec3(&mut y, s.relmat, x);
            add_to3(&mut y, s.relpos);
            let a = geom_distance_gradient(&mut grad1, m, d, s.plugin[0], s.id[0], x, s.geomtype[0]);
            let b = geom_distance_gradient(&mut grad2, m, d, s.plugin[1], s.id[1], &y, s.geomtype[1]);
            let input = grad2;
            mul_mat_tvec3(&mut grad2, s.relmat, &input);
            gradient[0] = grad1[0] + grad2[0];
            gradient[1] = grad1[1] + grad2[1];
            gradient[2] = grad1[2] + grad2[2];
            add_to_scl3(
                gradient,
                if a > b { &grad1 } else { &grad2 },
                if max(a, b) > 0.0 { 1.0 } else { -1.0 },
            );
            a + b + max(a, b).abs()
        }
        _ => {
            self::gradient(m, d, s, gradient, x);
            distance(m, d, s, x)
        }
    }
}

pub fn distance(m: &Model, d: &Data, s: &SdfQuery<'_>, x: &[f64; 3]) -> f64 {
    let mut y = [0.0; 3];
    match s.r#type {
        SDFTYPE_SINGLE => geom_distance(m, d, s.plugin[0], s.id[0], x, s.geomtype[0]),
        SDFTYPE_INTERSECTION => {
            mul_mat_vec3(&mut y, s.relmat, x);
            add_to3(&mut y, s.relpos);
            max(
                geom_distance(m, d, s.plugin[0], s.id[0], x, s.geomtype[0]),
                geom_distance(m, d, s.plugin[1], s.id[1], &y, s.geomtype[1]),
            )
        }
        SDFTYPE_MIDSURFACE => {
            mul_mat_vec3(&mut y, s.relmat, x);
            add_to3(&mut y, s.relpos);
            geom_distance(m, d, s.plugin[0], s.id[0], x, s.geomtype[0])
                - geom_distance(m, d, s.plugin[1], s.id[1], &y, s.geomtype[1])
        }
        SDFTYPE_COLLISION => {
            mul_mat_vec3(&mut y, s.relmat, x);
            add_to3(&mut y, s.relpos);
            let a = geom_distance(m, d, s.plugin[0], s.id[0], x, s.geomtype[0]);
            let b = geom_distance(m, d, s.plugin[1], s.id[1], &y, s.geomtype[1]);
            a + b + max(a, b).abs()
        }
        _ => panic!("SDF type not available"),
    }
}

pub fn gradient(m: &Model, d: &Data, s: &SdfQuery<'_>, gradient: &mut [f64; 3], x: &[f64; 3]) {
    let mut y = [0.0; 3];
    let mut grad1 = [0.0; 3];
    let mut grad2 = [0.0; 3];
    match s.r#type {
        SDFTYPE_INTERSECTION => {
            mul_mat_vec3(&mut y, s.relmat, x);
            add_to3(&mut y, s.relpos);
            let i = if geom_distance(m, d, s.plugin[0], s.id[0], x, s.geomtype[0])
                > geom_distance(m, d, s.plugin[1], s.id[1], &y, s.geomtype[1])
            {
                0
            } else {
                1
            };
            let point = [x, &y];
            geom_gradient(
                gradient,
                m,
                d,
                s.plugin[i],
                s.id[i],
                point[i],
                s.geomtype[i],
            );
            if i == 1 {
                let input = *gradient;
                mul_mat_tvec3(gradient, s.relmat, &input);
            }
        }
        SDFTYPE_MIDSURFACE => {
            mul_mat_vec3(&mut y, s.relmat, x);
            add_to3(&mut y, s.relpos);
            geom_gradient(&mut grad1, m, d, s.plugin[0], s.id[0], x, s.geomtype[0]);
            normalize3(&mut grad1);
            geom_gradient(&mut grad2, m, d, s.plugin[1], s.id[1], &y, s.geomtype[1]);
            let input = grad2;
            mul_mat_tvec3(&mut grad2, s.relmat, &input);
            normalize3(&mut grad2);
            sub3(gradient, &grad1, &grad2);
            normalize3(gradient);
        }
        SDFTYPE_COLLISION => {
            mul_mat_vec3(&mut y, s.relmat, x);
            add_to3(&mut y, s.relpos);
            let a = geom_distance(m, d, s.plugin[0], s.id[0], x, s.geomtype[0]);
            let b = geom_distance(m, d, s.plugin[1], s.id[1], &y, s.geomtype[1]);
            geom_gradient(&mut grad1, m, d, s.plugin[0], s.id[0], x, s.geomtype[0]);
            geom_gradient(&mut grad2, m, d, s.plugin[1], s.id[1], &y, s.geomtype[1]);
            let input = grad2;
            mul_mat_tvec3(&mut grad2, s.relmat, &input);
            gradient[0] = grad1[0] + grad2[0];
            gradient[1] = grad1[1] + grad2[1];
            gradient[2] = grad1[2] + grad2[2];
            add_to_scl3(
                gradient,
                if a > b { &grad1 } else { &grad2 },
                if max(a, b) > 0.0 { 1.0 } else { -1.0 },
            );
        }
        SDFTYPE_SINGLE => geom_gradient(gradient, m, d, s.plugin[0], s.id[0], x, s.geomtype[0]),
        _ => panic!("SDF type not available"),
    }
}

pub fn get_sdf(m: &Model, geom: i32) -> Option<&dyn Sdf> {
    let instance = m.geom_plugin[geom as usize];
    if instance < 0 {
        None
    } else {
        Some(m.sdf_plugins[instance as usize].0.as_ref())
    }
}

fn map_pose(
    xpos1: &[f64; 3],
    xquat1: &[f64; 4],
    xpos2: &[f64; 3],
    xquat2: &[f64; 4],
    pos12: &mut [f64; 3],
    mat12: &mut [f64; 9],
) {
    let mut negpos = [0.0; 3];
    let mut negquat = [0.0; 4];
    let mut quat12 = [0.0; 4];
    neg_pose(&mut negpos, &mut negquat, xpos2, xquat2);
    mul_pose(pos12, &mut quat12, &negpos, &negquat, xpos1, xquat1);
    quat2mat(mat12, &quat12);
}

fn isknown(points: &[f64], x: &[f64; 3], cnt: usize) -> bool {
    for i in 0..cnt {
        if dist3(x, at3(points, 3 * i)) < MINVAL {
            return true;
        }
    }
    false
}

fn add_pre_contact(
    points: &mut [f64],
    con: &mut PreContact,
    x: &[f64; 3],
    pos2: &[f64; 3],
    quat2: &[f64; 4],
    dist: f64,
    cnt: usize,
    m: &Model,
    s: &SdfQuery<'_>,
    d: &Data,
    flip_normal: i32,
) -> usize {
    if dist > 0.0 || isknown(points, x, cnt) {
        return cnt;
    } else {
        copy3(at3_mut(points, 3 * cnt), x);
    }
    let mut norm = [0.0; 3];
    let mut vec = [0.0; 3];
    gradient(m, d, s, &mut norm, x);
    let norm_len = normalize3(&mut norm);
    if !(norm_len >= MINVAL) {
        return cnt;
    }
    if flip_normal == 0 {
        let input = norm;
        scl3(&mut norm, &input, -1.0);
    }
    con.dist = dist;
    rot_vec_quat(&mut con.normal, &norm, quat2);
    let scl = if flip_normal != 0 {
        -0.5 * dist
    } else {
        0.5 * dist
    };
    scl3(&mut vec, &con.normal, scl);
    rot_vec_quat(&mut con.pos, x, quat2);
    zero3(&mut con.tangent);
    add_to3(&mut con.pos, pos2);
    add_to3(&mut con.pos, &vec);
    cnt + 1
}

fn step_frank_wolfe(
    x: &mut [f64; 3],
    corners: &[f64],
    ncorners: usize,
    m: &Model,
    sdf: &SdfQuery<'_>,
    d: &Data,
) -> f64 {
    for step in 0..m.opt.sdf_iterations {
        let mut best = MAXVAL;
        let mut s = [0.0; 3];
        let mut grad = [0.0; 3];
        let dist = distance_gradient(m, d, sdf, &mut grad, x);
        for i in 0..ncorners {
            let fun = dot3(at3(corners, 3 * i), &grad);
            if fun < best {
                best = fun;
                copy3(&mut s, at3(corners, 3 * i));
            }
        }
        // At the corner it steps to, the walk stays there for its remaining steps (the same gradient picks the
        // same corner): their distance is the one at hand.
        if s == *x {
            return dist;
        }
        sub_from3(&mut s, x);
        add_to_scl3(x, &s, 2.0 / (step as f64 + 2.0));
    }
    distance(m, d, sdf, x)
}

fn max_vert_dist(triangle: &[f64; 9], p: &[f64; 3]) -> f64 {
    let mut diff = [0.0; 3];
    let mut max_dist2 = 0.0;
    for i in 0..3 {
        sub3(&mut diff, at3(triangle, 3 * i), p);
        max_dist2 = max(max_dist2, dot3(&diff, &diff));
    }
    max_dist2.sqrt()
}

fn triangle_intersect(triangle: &[f64; 9], m: &Model, sdf: &SdfQuery<'_>, d: &Data) -> bool {
    let mut edges = [0.0; 6];
    let mut normal = [0.0; 3];
    let mut center = [0.0; 3];
    let mut v = [0.0; 9];
    let mut cross_mat = [0.0; 9]; // C cross array; avoid the function name.
    let mut centroid = [0.0; 3];
    let mut p = [0.0; 3];
    let k_distance_scl = 10.0;
    let k_min_height = 0.1;
    scl3(&mut centroid, at3(triangle, 0), 1.0 / 3.0);
    add_to_scl3(&mut centroid, at3(triangle, 3), 1.0 / 3.0);
    add_to_scl3(&mut centroid, at3(triangle, 6), 1.0 / 3.0);
    let dist_at_centroid = distance(m, d, sdf, &centroid);
    let h = -dist_at_centroid / k_distance_scl;
    if h.abs() < k_min_height {
        return dist_at_centroid < max_vert_dist(triangle, &centroid);
    }
    sub3(at3_mut(&mut edges, 0), at3(triangle, 3), at3(triangle, 0));
    sub3(at3_mut(&mut edges, 3), at3(triangle, 6), at3(triangle, 0));
    cross(&mut normal, at3(&edges, 0), at3(&edges, 3));
    normalize3(&mut normal);
    copy3(&mut p, &centroid);
    add_to_scl3(&mut p, &normal, -h);
    sub3(at3_mut(&mut v, 0), at3(triangle, 0), &p);
    sub3(at3_mut(&mut v, 3), at3(triangle, 3), &p);
    sub3(at3_mut(&mut v, 6), at3(triangle, 6), &p);
    cross(at3_mut(&mut cross_mat, 0), at3(&v, 3), at3(&v, 6));
    cross(at3_mut(&mut cross_mat, 3), at3(&v, 6), at3(&v, 0));
    cross(at3_mut(&mut cross_mat, 6), at3(&v, 0), at3(&v, 3));
    scl3(
        &mut center,
        at3(&cross_mat, 0),
        dot3(at3(&v, 0), at3(&v, 0)),
    );
    add_to_scl3(
        &mut center,
        at3(&cross_mat, 3),
        dot3(at3(&v, 3), at3(&v, 3)),
    );
    add_to_scl3(
        &mut center,
        at3(&cross_mat, 6),
        dot3(at3(&v, 6), at3(&v, 6)),
    );
    let denom = 2.0 * dot3(at3(&v, 0), at3(&cross_mat, 0));
    if denom.abs() < MINVAL {
        return dist_at_centroid < max_vert_dist(triangle, &centroid);
    }
    let input = center;
    scl3(&mut center, &input, 1.0 / denom);
    let r = dot3(&center, &center).sqrt();
    add_to3(&mut center, &p);
    distance(m, d, sdf, &center) < r
}

fn box_intersect(
    bvh: &[f64; 6],
    offset: &[f64; 3],
    rotation: &[f64; 9],
    m: &Model,
    s: &SdfQuery<'_>,
    d: &Data,
) -> bool {
    let mut candidate = [0.0; 3];
    let r = norm3(at3(bvh, 3));
    mul_mat_vec3(&mut candidate, rotation, at3(bvh, 0));
    add_to3(&mut candidate, offset);
    distance(m, d, s, &candidate) < r
}

fn select_fps(
    candidate: &[f64],
    dist: &[f64],
    ncandidate: usize,
    selected_indices: &mut [usize],
    max_select: usize,
) -> usize {
    if ncandidate == 0 {
        return 0;
    }
    let mut selected = [false; MAXCONPAIR];
    let mut min_dist2 = [0.0; MAXCONPAIR];
    for i in 0..ncandidate {
        min_dist2[i] = MAXVAL;
    }
    let mut best = 0_i32;
    let mut bestval = -dist[0];
    for i in 1..ncandidate {
        if -dist[i] > bestval {
            bestval = -dist[i];
            best = i as i32;
        }
    }
    let mut nselected = 0;
    while nselected < max_select && nselected < MAXCONPAIR && best >= 0 {
        selected[best as usize] = true;
        selected_indices[nselected] = best as usize;
        nselected += 1;
        let bestpos = at3(candidate, 3 * best as usize);
        let mut nextbest = -1;
        let mut nextbestdist = -1.0;
        for i in 0..ncandidate {
            if selected[i] {
                continue;
            }
            let dx = candidate[3 * i] - bestpos[0];
            let dy = candidate[3 * i + 1] - bestpos[1];
            let dz = candidate[3 * i + 2] - bestpos[2];
            let d2 = dx * dx + dy * dy + dz * dz;
            if d2 < min_dist2[i] {
                min_dist2[i] = d2;
            }
            if min_dist2[i] > nextbestdist {
                nextbestdist = min_dist2[i];
                nextbest = i as i32;
            }
        }
        best = nextbest;
    }
    nselected
}

// Deviation from the reference (engine_collision_sdf.c processSdfCorners: each of sdf_initpoints starts is a
// candidate when it penetrates, and `if (*ncandidate >= mjMAXCONPAIR) break;` ends the search for the whole
// mesh): there the slots fill in the BVH's traversal order from the first faces met, deep or not, and later
// faces are not looked at. Frank-Wolfe's first step has step size 1, so a face's starts mostly land on the
// same corner and fill several slots with it: a finely meshed part is pulled through a wall on the few
// contacts of its first faces. Here a face takes one start (the corner the others find again), and
// `keep_deepest` holds the mjMAXCONPAIR deepest faces wherever they come in the traversal.
fn process_sdf_corners(corners: &[f64; 9], m: &Model, d: &Data, sdf: &SdfQuery<'_>) -> Option<([f64; 3], f64)> {
    if !triangle_intersect(corners, m, sdf, d) {
        return None;
    }
    let mut u = halton(1, 2);
    let mut v = halton(1, 3);
    if u + v > 1.0 {
        u = 1.0 - u;
        v = 1.0 - v;
    }
    let b0 = 1.0 - u - v;
    let b1 = u;
    let b2 = v;
    let mut x = [
        b0 * corners[0] + b1 * corners[3] + b2 * corners[6],
        b0 * corners[1] + b1 * corners[4] + b2 * corners[7],
        b0 * corners[2] + b1 * corners[5] + b2 * corners[8],
    ];
    let depth = step_frank_wolfe(&mut x, corners, 3, m, sdf, d);
    (depth < 0.0).then_some((x, depth))
}

/// Holds `x` at `depth` among the mjMAXCONPAIR deepest candidates: in a free slot, else in place of the
/// shallowest when it is deeper. Whether it was kept.
fn keep_deepest(candidate: &mut [f64], dist: &mut [f64], ncandidate: &mut usize, x: &[f64; 3], depth: f64) -> bool {
    let slot = if *ncandidate < MAXCONPAIR {
        *ncandidate += 1;
        *ncandidate - 1
    } else {
        let mut shallowest = 0;
        for i in 1..MAXCONPAIR {
            if dist[i] > dist[shallowest] {
                shallowest = i;
            }
        }
        if depth >= dist[shallowest] {
            return false;
        }
        shallowest
    };
    copy3(at3_mut(candidate, 3 * slot), x);
    dist[slot] = depth;
    true
}

struct MeshSdfContext<'a, 'b> {
    m: &'a Model,
    d: &'a Data,
    sdf: &'a SdfQuery<'a>,
    offset: &'a [f64; 3],
    rotation: &'a [f64; 9],
    vertadr: usize,
    faceadr: usize,
    candidate: &'b mut [f64],
    dist: &'b mut [f64],
    ncandidate: &'b mut usize,
}

fn process_one_face(
    faceid: i32,
    bvh_active: Option<&mut [u8]>,
    node: usize,
    ctx: &mut MeshSdfContext<'_, '_>,
) -> bool {
    let mut corners = [0.0; 9];
    let m = ctx.m;
    let face = ctx.faceadr + faceid as usize;
    for v in 0..3 {
        let vec = [
            m.mesh_vert[3 * (ctx.vertadr + m.mesh_face[3 * face + v] as usize)] as f64,
            m.mesh_vert[3 * (ctx.vertadr + m.mesh_face[3 * face + v] as usize) + 1] as f64,
            m.mesh_vert[3 * (ctx.vertadr + m.mesh_face[3 * face + v] as usize) + 2] as f64,
        ];
        mul_mat_vec3(at3_mut(&mut corners, 3 * v), ctx.rotation, &vec);
        add_to3(at3_mut(&mut corners, 3 * v), ctx.offset);
    }
    let kept = process_sdf_corners(&corners, m, ctx.d, ctx.sdf)
        .is_some_and(|(x, depth)| keep_deepest(ctx.candidate, ctx.dist, ctx.ncandidate, &x, depth));
    if let Some(bvh_active) = bvh_active {
        if *ctx.ncandidate > 0 {
            bvh_active[node] = 1;
        }
    }
    kept
}

fn traverse_bvh(
    bvh: &[f64],
    nodeid: &[i32],
    child: &[i32],
    mut bvh_active: Option<&mut [u8]>,
    offset: &[f64; 3],
    rotation: &[f64; 9],
    m: &Model,
    d: &Data,
    sdf: &SdfQuery<'_>,
    callback: &mut dyn FnMut(i32, usize) -> bool,
) {
    let mut stack = [0; 64];
    let mut nstack = 0;
    stack[nstack] = 0;
    nstack += 1;
    while nstack != 0 {
        nstack -= 1;
        let node = stack[nstack];
        if nodeid[node] != -1 {
            if box_intersect(at6(bvh, 6 * node), offset, rotation, m, sdf, d) {
                let active = callback(nodeid[node], node);
                if let Some(bvh_active) = bvh_active.as_deref_mut() {
                    if active {
                        bvh_active[node] = 1;
                    }
                }
            }
            continue;
        }
        if !box_intersect(at6(bvh, 6 * node), offset, rotation, m, sdf, d) {
            continue;
        }
        if let Some(bvh_active) = bvh_active.as_deref_mut() {
            bvh_active[node] = 1;
        }
        for i in 0..2 {
            if child[2 * node + i] != -1 {
                if nstack >= 64 {
                    panic!("BVH stack depth exceeded.");
                }
                stack[nstack] = child[2 * node + i] as usize;
                nstack += 1;
            }
        }
    }
}

fn mesh_face_callback(face_id: i32, node: usize, ctx: &mut MeshSdfContext<'_, '_>) -> bool {
    process_one_face(face_id, None, node, ctx)
}

pub fn hfield_sdf(
    _m: &Model,
    _d: &mut Data,
    _con: &mut [PreContact],
    _g1: i32,
    _g2: i32,
    _margin: f64,
) -> usize {
    crate::util_errmem::warning_text("HField vs SDF collision not yet supported!");
    0
}

pub fn mesh_sdf(
    m: &Model,
    d: &mut Data,
    con: &mut [PreContact],
    g1: i32,
    g2: i32,
    _margin: f64,
) -> usize {
    let pos1 = *at3(&d.geom_xpos, 3 * g1 as usize);
    let quat1 = *at4(&d.geom_xquat, 4 * g1 as usize);
    let pos2 = *at3(&d.geom_xpos, 3 * g2 as usize);
    let sdf_quat = *at4(&d.geom_xquat, 4 * g2 as usize);
    let mut offset = [0.0; 3];
    let mut rotation = [0.0; 9];
    let mut points = [0.0; 3 * MAXCONPAIR];
    let mut dist2 = [0.0; MAXCONPAIR];
    let mut candidate = [0.0; 3 * MAXCONPAIR];
    let mut cnt = 0;
    let mut ncandidate = 0;
    let mut instance = m.geom_plugin[g2 as usize];
    let sdf_ptr = if instance == -1 { None } else { get_sdf(m, g2) };
    instance = if instance == -1 {
        m.geom_dataid[g2 as usize]
    } else {
        instance
    };
    let sdf = SdfQuery {
        id: &[instance],
        r#type: SDFTYPE_SINGLE,
        plugin: &[sdf_ptr],
        geomtype: &[GEOM_SDF],
        relpos: &[0.0; 3],
        relmat: &[0.0; 9],
    };
    map_pose(&pos1, &quat1, &pos2, &sdf_quat, &mut offset, &mut rotation);
    // Detach the visualization array while callbacks borrow Data, then restore it unchanged in size.
    let mut active = std::mem::take(&mut d.bvh_active);
    {
        let mut ctx = MeshSdfContext {
            m,
            d,
            sdf: &sdf,
            offset: &offset,
            rotation: &rotation,
            vertadr: m.mesh_vertadr[m.geom_dataid[g1 as usize] as usize] as usize,
            faceadr: m.mesh_faceadr[m.geom_dataid[g1 as usize] as usize] as usize,
            candidate: &mut candidate,
            dist: &mut dist2,
            ncandidate: &mut ncandidate,
        };
        let bvhadr = m.mesh_bvhadr[m.geom_dataid[g1 as usize] as usize];
        if bvhadr >= 0 {
            let bvhadr = bvhadr as usize;
            let nodeid = &m.bvh_nodeid[bvhadr..];
            let bvh = &m.bvh_aabb[6 * bvhadr..];
            let child = &m.bvh_child[2 * bvhadr..];
            // mjVisual.global.bvactive is the i32 at byte offset 48 in the stored C record.
            let bvactive = i32::from_le_bytes(m.vis[48..52].try_into().unwrap()) != 0;
            let bvh_active = if bvactive {
                Some(&mut active[bvhadr..])
            } else {
                None
            };
            traverse_bvh(
                bvh,
                nodeid,
                child,
                bvh_active,
                &offset,
                &rotation,
                m,
                d,
                &sdf,
                &mut |face_id, node| mesh_face_callback(face_id, node, &mut ctx),
            );
        }
    }
    d.bvh_active = active;
    if ncandidate <= MAXCONPAIR {
        for i in 0..ncandidate {
            cnt = add_pre_contact(
                &mut points,
                &mut con[cnt],
                at3(&candidate, 3 * i),
                &pos2,
                &sdf_quat,
                dist2[i],
                cnt,
                m,
                &sdf,
                d,
                0,
            );
        }
        return cnt;
    }
    let mut selected_indices = [0; MAXCONPAIR];
    let nselected = select_fps(
        &candidate,
        &dist2,
        ncandidate,
        &mut selected_indices,
        MAXCONPAIR,
    );
    for i in 0..nselected {
        let idx = selected_indices[i];
        cnt = add_pre_contact(
            &mut points,
            &mut con[cnt],
            at3(&candidate, 3 * idx),
            &pos2,
            &sdf_quat,
            dist2[idx],
            cnt,
            m,
            &sdf,
            d,
            0,
        );
    }
    cnt
}

pub fn sdf(
    m: &Model,
    d: &mut Data,
    con: &mut [PreContact],
    g1: i32,
    g2: i32,
    _margin: f64,
) -> usize {
    let pos1 = at3(&d.geom_xpos, 3 * g1 as usize);
    let quat1 = at4(&d.geom_xquat, 4 * g1 as usize);
    let (pos2, quat2) = (*at3(&d.geom_xpos, 3 * g2 as usize), *at4(&d.geom_xquat, 4 * g2 as usize));
    let size1 = at6(&m.geom_aabb, 6 * g1 as usize);
    let size2 = at6(&m.geom_aabb, 6 * g2 as usize);
    let mut vec1 = [0.0; 3];
    let mut vec2 = [0.0; 3];
    let mut aabb1 = [MAXVAL, MAXVAL, MAXVAL, -MAXVAL, -MAXVAL, -MAXVAL];
    let mut aabb2 = [MAXVAL, MAXVAL, MAXVAL, -MAXVAL, -MAXVAL, -MAXVAL];
    let mut aabb = [MAXVAL, MAXVAL, MAXVAL, -MAXVAL, -MAXVAL, -MAXVAL];
    if m.geom_type[g2 as usize] != GEOM_SDF {
        panic!("geom is not an SDF");
    }
    let mut offset21 = [0.0; 3];
    let mut rotation21 = [0.0; 9];
    let mut rotation12 = [0.0; 9];
    let mut offset12 = [0.0; 3];
    let mut offset2 = [0.0; 3];
    let mut rotation2 = [0.0; 9];
    map_pose(pos1, quat1, pos1, quat1, &mut offset2, &mut rotation2);
    map_pose(&pos2, &quat2, pos1, quat1, &mut offset21, &mut rotation21);
    map_pose(pos1, quat1, &pos2, &quat2, &mut offset12, &mut rotation12);
    for i in 0..8 {
        vec1[0] = if i & 1 != 0 {
            size1[0] + size1[3]
        } else {
            size1[0] - size1[3]
        };
        vec1[1] = if i & 2 != 0 {
            size1[1] + size1[4]
        } else {
            size1[1] - size1[4]
        };
        vec1[2] = if i & 4 != 0 {
            size1[2] + size1[5]
        } else {
            size1[2] - size1[5]
        };
        vec2[0] = if i & 1 != 0 {
            size2[0] + size2[3]
        } else {
            size2[0] - size2[3]
        };
        vec2[1] = if i & 2 != 0 {
            size2[1] + size2[4]
        } else {
            size2[1] - size2[4]
        };
        vec2[2] = if i & 4 != 0 {
            size2[2] + size2[5]
        } else {
            size2[2] - size2[5]
        };
        let input = vec2;
        mul_mat_vec3(&mut vec2, &rotation21, &input);
        add_to3(&mut vec2, &offset21);
        for k in 0..3 {
            aabb1[k] = min(aabb1[k], vec1[k]);
            aabb1[3 + k] = max(aabb1[3 + k], vec1[k]);
            aabb2[k] = min(aabb2[k], vec2[k]);
            aabb2[3 + k] = max(aabb2[3 + k], vec2[k]);
        }
    }
    for k in 0..3 {
        aabb[k] = max(aabb1[k], aabb2[k]);
        aabb[3 + k] = min(aabb1[3 + k], aabb2[3 + k]);
    }
    if aabb[3] < aabb[0] || aabb[4] < aabb[1] || aabb[5] < aabb[2] {
        return 0;
    }
    let mut instance = [0; 2];
    let mut sdf_ptr: [Option<&dyn Sdf>; 2] = [None; 2];
    let geomtypes = [m.geom_type[g2 as usize], m.geom_type[g1 as usize]];
    instance[0] = m.geom_plugin[g2 as usize];
    sdf_ptr[0] = if instance[0] == -1 {
        None
    } else {
        get_sdf(m, g2)
    };
    if m.geom_type[g1 as usize] == GEOM_SDF {
        instance[1] = m.geom_plugin[g1 as usize];
        sdf_ptr[1] = if instance[1] == -1 {
            None
        } else {
            get_sdf(m, g1)
        };
    } else {
        instance[1] = g1;
        sdf_ptr[1] = None;
    }
    // Plugin reset only resets visualization; the Sdf interface has no visualization state.
    instance[0] = if instance[0] == -1 {
        m.geom_dataid[g2 as usize]
    } else {
        instance[0]
    };
    instance[1] = if instance[1] == -1 {
        m.geom_dataid[g1 as usize]
    } else {
        instance[1]
    };
    let sdf = SdfQuery {
        id: &instance,
        relpos: &offset21,
        relmat: &rotation21,
        plugin: &sdf_ptr,
        geomtype: &geomtypes,
        r#type: SDFTYPE_COLLISION,
    };
    // Deviation from the reference (engine_collision_sdf.c mjc_SDF, for an SDF against another SDF or a
    // primitive, the sphere, capsule, ellipsoid, cylinder and box: each of sdf_initpoints Halton starts in the
    // boxes' overlap walked down d1 + d2 + |max(d1, d2)| by stepGradient, then one contact per start that ends
    // inside, at max(d1, d2), the normal from the midsurface's gradient; see the header for why): our own
    // search. It starts from the pair's touches of its last collision and where the bound on their steps
    // stopped its walks then, from the centre of the boxes' overlap and from the reference's Halton starts in it
    // (sdf_initpoints in each cube of the thinner geom's thickness along the overlap's longest side), each walked
    // until it settles at the deepest point of the two fields' overlap on their midsurface (`walk_midsurface`:
    // sdf_iterations bounds its steps, not its end); one touch per point where they overlap, at d1 + d2, points
    // nearer than a hundredth of the overlap's size taken as one (the deeper kept); contacts at the touches and
    // at the ends of their extent along the midsurface (`touch_extent`), and of more than the driver takes for
    // the pair, the deepest and those farthest apart.
    let to_g2 = |p: [f64; 3]| {
        let (mut y, mut x) = ([0.0; 3], [0.0; 3]);
        mul_mat_vec3(&mut y, &rotation2, &p);
        add_to3(&mut y, &offset2);
        mul_mat_vec3(&mut x, &rotation12, &y);
        add_to3(&mut x, &offset12);
        x
    };
    let size = ((aabb[3] - aabb[0]).powi(2) + (aabb[4] - aabb[1]).powi(2) + (aabb[5] - aabb[2]).powi(2)).sqrt();
    let mut seeds = [[0.0; 3]; MAXCONPAIR];
    let mut nseed = 0;
    for &(a, b, p) in &d.sdf_seeds {
        if a == g1 && b == g2 && nseed < MAXCONPAIR {
            seeds[nseed] = p;
            nseed += 1;
        }
    }
    let centre = to_g2([(aabb[0] + aabb[3]) / 2.0, (aabb[1] + aabb[4]) / 2.0, (aabb[2] + aabb[5]) / 2.0]);
    // sdf_initpoints starts in each cube of the thinner geom's thickness along the overlap's longest side (a pin
    // in its hole gets them all along it, its foot as many as its whole box would have), the thickness taken no
    // less than a thousandth of the overlap's size
    let thick = (3..6).map(|k| 2.0 * min(size1[k], size2[k])).fold(MAXVAL, f64::min).max(1e-3 * size);
    let longest = (0..3).map(|k| aabb[3 + k] - aabb[k]).fold(0.0, f64::max);
    let nstart = m.opt.sdf_initpoints.max(0) * (longest / thick).round().max(1.0) as i32;
    let halton_starts = (0..nstart).map(|j| {
        to_g2([
            aabb[0] + (aabb[3] - aabb[0]) * halton(j, 2),
            aabb[1] + (aabb[4] - aabb[1]) * halton(j, 3),
            aabb[2] + (aabb[5] - aabb[2]) * halton(j, 5),
        ])
    });
    // the driver's count for the pair (the reference's one per start)
    let cap = crate::collision_driver::max_contact(m, g1, g2, false).min(con.len());
    // where the walks settled in the overlap (the touches), then those and the touches' extents
    let mut touches = [([0.0; 3], 0.0, [0.0; 3]); MAXCONPAIR];
    let mut ntouch = 0;
    // the ends of the walks the bound on their steps stopped, which the next collision carries on from there
    let mut carried = [[0.0; 3]; MAXCONPAIR];
    let mut ncarried = 0;
    for x in seeds[..nseed].iter().copied().chain(std::iter::once(centre)).chain(halton_starts) {
        let walk = walk_midsurface(x, m, d, &sdf, m.opt.sdf_iterations, size);
        if walk.ended == Walked::Bounded {
            if d.sdf_walks_bounded == 0 {
                crate::util_errmem::warning_text(&format!(
                    "SDF collision: a walk of geoms {g1} and {g2} took sdf_iterations ({}) steps without settling; it carries on in the next collision. Time = {:.4}.",
                    m.opt.sdf_iterations, d.time
                ));
            }
            d.sdf_walks_bounded += 1;
            if ncarried < MAXCONPAIR && !carried[..ncarried].iter().any(|c| dist3(c, &walk.end) < 1e-2 * size) {
                carried[ncarried] = walk.end;
                ncarried += 1;
            }
        }
        if let Some(touch) = walk.best.filter(|b| b.1 < 0.0) {
            keep_touch(&mut touches, &mut ntouch, touch, size);
        }
    }
    // each touch's extent: from it, the farthest points of the overlap both ways toward the touch farthest from
    // it and both ways across that, along the midsurface; a face that meets another, or a line, so gets contacts
    // over all of it, not only at its deepest point (on which it would rock or pivot). The deepest first; a touch
    // within the extent of one before it (on its midsurface, within its ends each way) is of the same face or
    // line, not walked again.
    let mut found = touches;
    let mut cnt = ntouch;
    let mut order: [usize; MAXCONPAIR] = std::array::from_fn(|i| i);
    order[..ntouch].sort_by(|&i, &j| touches[i].1.total_cmp(&touches[j].1));
    // the extents walked: from where, its normal, the two ways, and how far each way reaches (+along, -along,
    // +across, -across)
    let mut spans = [([0.0; 3], [0.0; 3], [0.0; 3], [0.0; 3], [0.0; 4]); MAXCONPAIR];
    let mut nspan = 0;
    for &k in &order[..ntouch] {
        let (x, _, n) = touches[k];
        let apart = 1e-2 * size;
        let within = |span: &([f64; 3], [f64; 3], [f64; 3], [f64; 3], [f64; 4])| {
            let mut v = [0.0; 3];
            sub3(&mut v, &x, &span.0);
            let (u, w) = (dot3(&v, &span.2), dot3(&v, &span.3));
            dot3(&v, &span.1).abs() < apart && u < span.4[0] + apart && -u < span.4[1] + apart && w < span.4[2] + apart && -w < span.4[3] + apart
        };
        if spans[..nspan].iter().any(within) {
            continue;
        }
        let mut along = [0.0; 3];
        if let Some(j) = (0..ntouch).max_by(|&i, &j| dist3(&touches[i].0, &x).total_cmp(&dist3(&touches[j].0, &x))) {
            sub3(&mut along, &touches[j].0, &x);
            let input = along;
            add_to_scl3(&mut along, &n, -dot3(&input, &n));
        }
        if normalize3(&mut along) < apart {
            // no other touch apart from it: across the normal from its axis least along it
            let least = (0..3).min_by(|&i, &j| n[i].abs().total_cmp(&n[j].abs())).unwrap_or(0);
            let mut axis = [0.0; 3];
            axis[least] = 1.0;
            cross(&mut along, &n, &axis);
            normalize3(&mut along);
        }
        let mut across = [0.0; 3];
        cross(&mut across, &n, &along);
        let mut reach = [0.0; 4];
        for (way, (t, sign)) in [(along, 1.0), (along, -1.0), (across, 1.0), (across, -1.0)].into_iter().enumerate() {
            let mut dir = [0.0; 3];
            scl3(&mut dir, &t, sign);
            if let Some(end) = touch_extent(x, &dir, m, d, &sdf, size) {
                let mut v = [0.0; 3];
                sub3(&mut v, &end.0, &x);
                reach[way] = dot3(&v, &dir).max(0.0);
                keep_touch(&mut found, &mut cnt, end, size);
            }
        }
        spans[nspan] = (x, n, along, across, reach);
        nspan += 1;
    }
    // more than the driver takes: the deepest, then each time the one farthest from those taken, so that they
    // span the touch (the deepest alone may all lie at one end of a line of touch, the other end let through)
    if cnt > cap && cap > 0 {
        let deepest = (0..cnt).min_by(|&a, &b| found[a].1.total_cmp(&found[b].1)).unwrap_or(0);
        found.swap(0, deepest);
        for taken in 1..cap {
            let apart = |i: usize| (0..taken).map(|t| dist3(&found[i].0, &found[t].0)).fold(MAXVAL, f64::min);
            let k = (taken..cnt).max_by(|&a, &b| apart(a).total_cmp(&apart(b))).unwrap_or(taken);
            found.swap(taken, k);
        }
        cnt = cap;
    }
    d.sdf_seeds.retain(|&(a, b, _)| a != g1 || b != g2);
    d.sdf_seeds.extend(touches[..ntouch].iter().map(|f| (g1, g2, f.0)));
    d.sdf_seeds.extend(carried[..ncarried].iter().filter(|c| !touches[..ntouch].iter().any(|f| dist3(&f.0, c) < 1e-2 * size)).map(|&c| (g1, g2, c)));
    for (k, &(x, dist, normal)) in found[..cnt].iter().enumerate() {
        let c = &mut con[k];
        c.dist = dist;
        rot_vec_quat(&mut c.normal, &normal, &quat2);
        rot_vec_quat(&mut c.pos, &x, &quat2);
        add_to3(&mut c.pos, &pos2);
        zero3(&mut c.tangent);
    }
    cnt
}

/// The two fields of a pair at `x` (the second's frame): each one's distance and gradient (in that frame).
fn pair_fields(m: &Model, d: &Data, s: &SdfQuery<'_>, x: &[f64; 3]) -> (f64, [f64; 3], f64, [f64; 3]) {
    let (mut ga, mut gb, mut y) = ([0.0; 3], [0.0; 3], [0.0; 3]);
    let a = geom_distance_gradient(&mut ga, m, d, s.plugin[0], s.id[0], x, s.geomtype[0]);
    mul_mat_vec3(&mut y, s.relmat, x);
    add_to3(&mut y, s.relpos);
    let mut gy = [0.0; 3];
    let b = geom_distance_gradient(&mut gy, m, d, s.plugin[1], s.id[1], &y, s.geomtype[1]);
    mul_mat_tvec3(&mut gb, s.relmat, &gy);
    (a, ga, b, gb)
}

/// Keeps a point where the two fields overlap (point, depth, normal) among `found[..*cnt]`: one nearer than a
/// hundredth of the overlap's `size` to a kept one is the same touch (the deeper kept), and of more than
/// MAXCONPAIR the shallowest gives way to a deeper one.
fn keep_touch(found: &mut [([f64; 3], f64, [f64; 3]); MAXCONPAIR], cnt: &mut usize, touch: ([f64; 3], f64, [f64; 3]), size: f64) {
    if let Some(k) = (0..*cnt).find(|&k| dist3(&found[k].0, &touch.0) < 1e-2 * size) {
        if touch.1 < found[k].1 {
            found[k] = touch;
        }
    } else if *cnt < MAXCONPAIR {
        found[*cnt] = touch;
        *cnt += 1;
    } else if let Some(k) = (0..*cnt).max_by(|&a, &b| found[a].1.total_cmp(&found[b].1)).filter(|&k| touch.1 < found[k].1) {
        found[k] = touch;
    }
}

/// How closely the two fields' surfaces face each other (their gradients' dot product, negated) where a touch
/// extends: a face's touch ends at its edge, where one field's gradient turns from the other's.
const TOUCH_FACING: f64 = 0.98;

/// Ours, not the reference's: from `x`, a touch (where the two fields overlap, on their midsurface, the pair's
/// second geom's frame), the farthest point along `dir` (along the midsurface) to which the overlap keeps on
/// the line from it, with the surfaces facing each other there (TOUCH_FACING): steps from `x` double from a
/// thousandth of the overlap's `size` while they stay in the overlap, then halve between the farthest point in
/// it and the nearest out until they are under that. The point levelled onto the midsurface, the fields' gap
/// there (below zero) and the normal; none where the overlap ends within the first step.
fn touch_extent(x: [f64; 3], dir: &[f64; 3], m: &Model, d: &Data, s: &SdfQuery<'_>, size: f64) -> Option<([f64; 3], f64, [f64; 3])> {
    let tol = 1e-3 * size;
    let inside = |r: f64| {
        let mut p = x;
        add_to_scl3(&mut p, dir, r);
        let (a, ga, b, gb) = pair_fields(m, d, s, &p);
        if !(a.is_finite() && b.is_finite()) || a + b >= 0.0 || dot3(&ga, &gb) > -TOUCH_FACING {
            return None;
        }
        let mut n = [gb[0] - ga[0], gb[1] - ga[1], gb[2] - ga[2]];
        let span = normalize3(&mut n);
        if span < MINVAL {
            return None;
        }
        add_to_scl3(&mut p, &n, (a - b) / span);
        Some((p, a + b, n))
    };
    // the farthest point found in the overlap, how far along it is, and how far the nearest point out
    let (mut end, mut reach, mut out) = (None, 0.0, None::<f64>);
    let mut step = tol;
    loop {
        let r = match out {
            Some(o) if o - reach < tol => break,
            Some(o) => (reach + o) / 2.0,
            None if reach + step > size => break,
            None => reach + step,
        };
        match inside(r) {
            Some(at) => {
                end = Some(at);
                reach = r;
                step *= 2.0;
            }
            None => out = Some(r),
        }
    }
    end
}

/// How a walk of the midsurface ended.
#[derive(Clone, Copy, PartialEq)]
enum Walked {
    /// at its deepest point, its step shorter than a thousandth of the overlap's size
    Settled,
    /// at the bound on its steps, still going: the next collision carries it on from there
    Bounded,
    /// off the fields (a distance not finite) or nowhere to go (no gradient)
    Lost,
}

struct Walk {
    /// the deepest point it passed where the surfaces face each other, levelled: there, the fields' gap (their
    /// distances' sum, the depth where it is below zero) and the normal
    best: Option<([f64; 3], f64, [f64; 3])>,
    /// where it stopped
    end: [f64; 3],
    ended: Walked,
}

/// Ours, not the reference's: from `start` (the pair's second geom's frame) to the deepest point of the two
/// fields' overlap near it, or where they come nearest, on their midsurface. Where the surfaces nearest face each
/// other, each step levels the two distances along the midsurface's normal and goes down their sum along the
/// midsurface, by the step length the last two steps' gradients give (none longer than the overlap's `size`); a
/// point no deeper than the deepest so far (the larger distance least) sends it back there with half the step
/// taken from it. Where they do not face each other (a point in a hollow of both), it goes to the farther
/// surface. It ends where its step is shorter than a thousandth of `size` (settled), or after `bound` steps,
/// still going (the caller carries it on in the next collision), or where a distance is not finite or has no
/// gradient (lost). Its deepest point, levelled, is where the fields' gap is least: below zero, they overlap by
/// it, the contact's depth, along the normal out of the first geom into the second.
fn walk_midsurface(start: [f64; 3], m: &Model, d: &Data, s: &SdfQuery<'_>, bound: i32, size: f64) -> Walk {
    let tol = 1e-3 * size;
    let mut x = start;
    // the deepest point so far (the larger distance least): there, its level, depth, normal, larger distance
    // and the step taken from it
    let mut best: Option<([f64; 3], f64, f64, [f64; 3], f64, [f64; 3])> = None;
    let mut last: Option<([f64; 3], [f64; 3])> = None;
    let mut alpha = 1.0;
    let mut ended = Walked::Bounded;
    let mut steps = 0;
    while steps < bound.max(1) {
        steps += 1;
        let (a, ga, b, gb) = pair_fields(m, d, s, &x);
        if !(a.is_finite() && b.is_finite()) {
            ended = Walked::Lost;
            break;
        }
        let facing = dot3(&ga, &gb) < 0.0;
        if let Some(bst) = best.as_mut().filter(|bst| !facing || max(a, b) >= bst.4) {
            // no deeper than the deepest point so far: back there, half the step taken from it
            let input = bst.5;
            scl3(&mut bst.5, &input, 0.5);
            if norm3(&bst.5) < tol {
                ended = Walked::Settled;
                break;
            }
            alpha *= 0.5;
            x = bst.0;
            add_to3(&mut x, &bst.5);
            continue;
        }
        if !facing {
            // the two surfaces nearest do not face each other: toward the farther one by its distance (no
            // farther than its surface, the fields being distances)
            let (far, mut g) = if a > b { (a, ga) } else { (b, gb) };
            if normalize3(&mut g) < MINVAL {
                ended = Walked::Lost;
                break;
            }
            let reach = min(far.abs().max(tol), size);
            add_to_scl3(&mut x, &g, if far > 0.0 { -reach } else { reach });
            last = None;
            continue;
        }
        let mut n = [gb[0] - ga[0], gb[1] - ga[1], gb[2] - ga[2]];
        let span = normalize3(&mut n);
        if span < MINVAL {
            ended = Walked::Lost;
            break;
        }
        // along the normal to where the two distances are level
        let level = (a - b) / span;
        // their sum's gradient along the midsurface
        let mut g = [ga[0] + gb[0], ga[1] + gb[1], ga[2] + gb[2]];
        let gn = dot3(&g, &n);
        add_to_scl3(&mut g, &n, -gn);
        if let Some((x0, g0)) = last {
            let mut sx = [x[0] - x0[0], x[1] - x0[1], x[2] - x0[2]];
            let sn = dot3(&sx, &n);
            add_to_scl3(&mut sx, &n, -sn);
            let sy = [g[0] - g0[0], g[1] - g0[1], g[2] - g0[2]];
            let curve = dot3(&sx, &sy);
            if curve > MINVAL {
                alpha = min(dot3(&sx, &sx) / curve, size);
            }
        }
        last = Some((x, g));
        let mut step = [level * n[0] - alpha * g[0], level * n[1] - alpha * g[1], level * n[2] - alpha * g[2]];
        let len = norm3(&step);
        if len > size {
            let input = step;
            scl3(&mut step, &input, size / len);
        }
        best = Some((x, level, a + b, n, max(a, b), step));
        if len < tol {
            ended = Walked::Settled;
            break;
        }
        add_to3(&mut x, &step);
    }
    let best = best.map(|(at, level, gap, n, _, _)| {
        let mut p = at;
        add_to_scl3(&mut p, &n, level);
        (p, gap, n)
    });
    Walk { best, end: x, ended }
}

// flexElemCallback and mjc_FlexSDF are left untranslated with flex.
// Their future collision-driver call sites must use unported!("mjc_FlexSDF").
