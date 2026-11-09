//! Primitive and mesh ray intersections used by continuous collision.
//!
//! Translated from `src/engine/engine_ray.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::data::Data;
use crate::inline::{at3, at6, at9};
use crate::model::Model;
use crate::types::*;
use crate::util_blas as u;
use crate::util_spatial::cross;

fn ray_map(pos: &[f64; 3], mat: &[f64; 9], pnt: &[f64; 3], vec: &[f64; 3], lpnt: &mut [f64; 3], lvec: &mut [f64; 3]) {
    let dif = [pnt[0] - pos[0], pnt[1] - pos[1], pnt[2] - pos[2]];
    lpnt[0] = mat[0]*dif[0] + mat[3]*dif[1] + mat[6]*dif[2];
    lpnt[1] = mat[1]*dif[0] + mat[4]*dif[1] + mat[7]*dif[2];
    lpnt[2] = mat[2]*dif[0] + mat[5]*dif[1] + mat[8]*dif[2];
    lvec[0] = mat[0]*vec[0] + mat[3]*vec[1] + mat[6]*vec[2];
    lvec[1] = mat[1]*vec[0] + mat[4]*vec[1] + mat[7]*vec[2];
    lvec[2] = mat[2]*vec[0] + mat[5]*vec[1] + mat[8]*vec[2];
}

// longitude, latitude and ray_eliminate are omitted with their unused ray callers.

fn ray_quad(a: f64, b: f64, c: f64, x: &mut [f64; 2]) -> f64 {
    let mut det = b*b - a*c;
    if det < 0.0 || a < MINVAL { x[0] = -1.0; x[1] = -1.0; return -1.0; }
    det = det.sqrt();
    x[0] = (-b-det)/a;
    x[1] = (-b+det)/a;
    if x[0] >= 0.0 { return x[0]; } else if x[1] >= 0.0 { return x[1]; }
    -1.0
}

pub fn ray_triangle(v: &[[f64; 3]; 3], lpnt: &[f64; 3], lvec: &[f64; 3], b0: &[f64; 3], b1: &[f64; 3], mut normal: Option<&mut [f64; 3]>) -> f64 {
    if let Some(normal) = normal.as_deref_mut() { u::zero3(normal); }
    let mut dif = [[0.0; 3]; 3];
    for i in 0..3 { for j in 0..3 { dif[i][j] = v[i][j] - lpnt[j]; } }
    let mut planar = [[0.0; 2]; 3];
    for i in 0..3 { planar[i][0] = u::dot3(b0, &dif[i]); planar[i][1] = u::dot3(b1, &dif[i]); }
    if (planar[0][0] > 0.0 && planar[1][0] > 0.0 && planar[2][0] > 0.0) ||
       (planar[0][0] < 0.0 && planar[1][0] < 0.0 && planar[2][0] < 0.0) ||
       (planar[0][1] > 0.0 && planar[1][1] > 0.0 && planar[2][1] > 0.0) ||
       (planar[0][1] < 0.0 && planar[1][1] < 0.0 && planar[2][1] < 0.0) { return -1.0; }
    let a = [planar[0][0]-planar[2][0], planar[1][0]-planar[2][0], planar[0][1]-planar[2][1], planar[1][1]-planar[2][1]];
    let b = [-planar[2][0], -planar[2][1]];
    let det = a[0]*a[3] - a[1]*a[2];
    if det.abs() < MINVAL { return -1.0; }
    let t0 = (a[3]*b[0] - a[1]*b[1]) / det;
    let t1 = (-a[2]*b[0] + a[0]*b[1]) / det;
    if t0 < 0.0 || t1 < 0.0 || t0+t1 > 1.0 { return -1.0; }
    u::sub3(&mut dif[0], &v[0], &v[2]);
    u::sub3(&mut dif[1], &v[1], &v[2]);
    u::sub3(&mut dif[2], lpnt, &v[2]);
    let mut nrm = [0.0; 3]; cross(&mut nrm, &dif[0], &dif[1]);
    let denom = u::dot3(lvec, &nrm);
    if denom.abs() < MINVAL { return -1.0; }
    let x = -u::dot3(&dif[2], &nrm) / denom;
    if x < 0.0 { return -1.0; }
    if let Some(normal) = normal { u::normalize3(&mut nrm); u::copy3(normal, &nrm); }
    x
}

fn ray_plane(pos: &[f64; 3], mat: &[f64; 9], size: &[f64; 3], pnt: &[f64; 3], vec: &[f64; 3], mut normal: Option<&mut [f64; 3]>) -> f64 {
    if let Some(normal) = normal.as_deref_mut() { u::zero3(normal); }
    let mut lpnt = [0.0; 3]; let mut lvec = [0.0; 3];
    ray_map(pos, mat, pnt, vec, &mut lpnt, &mut lvec);
    if lvec[2] > -MINVAL { return -1.0; }
    let x = -lpnt[2]/lvec[2];
    if x < 0.0 { return -1.0; }
    let p0 = lpnt[0] + x*lvec[0]; let p1 = lpnt[1] + x*lvec[1];
    if (size[0] <= 0.0 || p0.abs() <= size[0]) && (size[1] <= 0.0 || p1.abs() <= size[1]) {
        if let Some(normal) = normal { normal[0] = mat[2]; normal[1] = mat[5]; normal[2] = mat[8]; }
        x
    } else { -1.0 }
}

fn ray_sphere(pos: &[f64; 3], _mat: Option<&[f64; 9]>, dist_sqr: f64, pnt: &[f64; 3], vec: &[f64; 3], normal: Option<&mut [f64; 3]>) -> f64 {
    let dif = [pnt[0]-pos[0], pnt[1]-pos[1], pnt[2]-pos[2]];
    let a = vec[0]*vec[0] + vec[1]*vec[1] + vec[2]*vec[2];
    let b = vec[0]*dif[0] + vec[1]*dif[1] + vec[2]*dif[2];
    let c = dif[0]*dif[0] + dif[1]*dif[1] + dif[2]*dif[2] - dist_sqr;
    let mut xx = [0.0; 2]; let x = ray_quad(a, b, c, &mut xx);
    if let Some(normal) = normal {
        if x < 0.0 { u::zero3(normal); } else {
            let mut s = [0.0; 3]; u::add_scl3(&mut s, pnt, vec, x); u::sub3(normal, &s, pos); u::normalize3(normal);
        }
    }
    x
}

fn ray_capsule(pos: &[f64; 3], mat: &[f64; 9], size: &[f64; 3], pnt: &[f64; 3], vec: &[f64; 3], normal: Option<&mut [f64; 3]>) -> f64 {
    let ssz = size[0] + size[1];
    if ray_sphere(pos, None, ssz * ssz, pnt, vec, None) < 0.0 {
        if let Some(normal) = normal { u::zero3(normal); } return -1.0;
    }
    let mut lpnt = [0.0; 3]; let mut lvec = [0.0; 3];
    ray_map(pos, mat, pnt, vec, &mut lpnt, &mut lvec);
    let mut x = -1.0; let mut xx = [0.0; 2];
    let mut type_ = 0;
    let mut a = lvec[0]*lvec[0] + lvec[1]*lvec[1];
    let mut b = lvec[0]*lpnt[0] + lvec[1]*lpnt[1];
    let mut c = lpnt[0]*lpnt[0] + lpnt[1]*lpnt[1] - size[0]*size[0];
    let sol = ray_quad(a, b, c, &mut xx);
    if sol >= 0.0 && (lpnt[2]+sol*lvec[2]).abs() <= size[1] {
        if x < 0.0 || sol < x { x = sol; type_ = 0; }
    }
    let mut ldif = [lpnt[0], lpnt[1], lpnt[2]-size[1]];
    a = lvec[0]*lvec[0] + lvec[1]*lvec[1] + lvec[2]*lvec[2];
    b = lvec[0]*ldif[0] + lvec[1]*ldif[1] + lvec[2]*ldif[2];
    c = ldif[0]*ldif[0] + ldif[1]*ldif[1] + ldif[2]*ldif[2] - size[0]*size[0];
    ray_quad(a, b, c, &mut xx);
    for i in 0..2 {
        if xx[i] >= 0.0 && lpnt[2]+xx[i]*lvec[2] >= size[1] {
            if x < 0.0 || xx[i] < x { x = xx[i]; type_ = 1; }
        }
    }
    ldif[2] = lpnt[2]+size[1];
    b = lvec[0]*ldif[0] + lvec[1]*ldif[1] + lvec[2]*ldif[2];
    c = ldif[0]*ldif[0] + ldif[1]*ldif[1] + ldif[2]*ldif[2] - size[0]*size[0];
    ray_quad(a, b, c, &mut xx);
    for i in 0..2 {
        if xx[i] >= 0.0 && lpnt[2]+xx[i]*lvec[2] <= -size[1] {
            if x < 0.0 || xx[i] < x { x = xx[i]; type_ = -1; }
        }
    }
    if let Some(normal) = normal {
        if x < 0.0 { u::zero3(normal); } else {
            normal[0] = lpnt[0] + lvec[0] * x;
            normal[1] = lpnt[1] + lvec[1] * x;
            normal[2] = if type_ == 0 { 0.0 } else { lpnt[2] + lvec[2] * x - size[1] * type_ as f64 };
            u::normalize3(normal);
            let old = *normal; u::mul_mat_vec3(normal, mat, &old);
        }
    }
    x
}

fn ray_ellipsoid(pos: &[f64; 3], mat: &[f64; 9], size: &[f64; 3], pnt: &[f64; 3], vec: &[f64; 3], normal: Option<&mut [f64; 3]>) -> f64 {
    let mut lpnt = [0.0; 3]; let mut lvec = [0.0; 3];
    ray_map(pos, mat, pnt, vec, &mut lpnt, &mut lvec);
    let s = [1.0/(size[0]*size[0]), 1.0/(size[1]*size[1]), 1.0/(size[2]*size[2])];
    let a = s[0]*lvec[0]*lvec[0] + s[1]*lvec[1]*lvec[1] + s[2]*lvec[2]*lvec[2];
    let b = s[0]*lvec[0]*lpnt[0] + s[1]*lvec[1]*lpnt[1] + s[2]*lvec[2]*lpnt[2];
    let c = s[0]*lpnt[0]*lpnt[0] + s[1]*lpnt[1]*lpnt[1] + s[2]*lpnt[2]*lpnt[2] - 1.0;
    let mut xx = [0.0; 2]; let x = ray_quad(a, b, c, &mut xx);
    if let Some(normal) = normal {
        if x < 0.0 { u::zero3(normal); } else {
            let mut l = [0.0; 3]; u::add_scl3(&mut l, &lpnt, &lvec, x);
            normal[0] = s[0] * l[0]; normal[1] = s[1] * l[1]; normal[2] = s[2] * l[2];
            u::normalize3(normal); let old = *normal; u::mul_mat_vec3(normal, mat, &old);
        }
    }
    x
}

fn ray_cylinder(pos: &[f64; 3], mat: &[f64; 9], size: &[f64; 3], pnt: &[f64; 3], vec: &[f64; 3], normal: Option<&mut [f64; 3]>) -> f64 {
    let ssz = size[0]*size[0] + size[1]*size[1];
    if ray_sphere(pos, None, ssz, pnt, vec, None) < 0.0 {
        if let Some(normal) = normal { u::zero3(normal); } return -1.0;
    }
    let mut lpnt = [0.0; 3]; let mut lvec = [0.0; 3];
    ray_map(pos, mat, pnt, vec, &mut lpnt, &mut lvec);
    let mut x = -1.0; let mut type_ = 0;
    if lvec[2].abs() > MINVAL {
        for side in (-1..=1).step_by(2) {
            let sol = (side as f64*size[1]-lpnt[2])/lvec[2];
            if sol >= 0.0 {
                let p0 = lpnt[0] + sol*lvec[0]; let p1 = lpnt[1] + sol*lvec[1];
                if p0*p0 + p1*p1 <= size[0]*size[0] {
                    if x < 0.0 || sol < x { x = sol; type_ = side; }
                }
            }
        }
    }
    let a = lvec[0]*lvec[0] + lvec[1]*lvec[1];
    let b = lvec[0]*lpnt[0] + lvec[1]*lpnt[1];
    let c = lpnt[0]*lpnt[0] + lpnt[1]*lpnt[1] - size[0]*size[0];
    let mut xx = [0.0; 2]; let sol = ray_quad(a, b, c, &mut xx);
    if sol >= 0.0 && (lpnt[2]+sol*lvec[2]).abs() <= size[1] {
        if x < 0.0 || sol < x { x = sol; type_ = 0; }
    }
    if let Some(normal) = normal {
        if x < 0.0 { u::zero3(normal); } else {
            if type_ == 0 {
                normal[0] = lpnt[0] + lvec[0] * x;
                normal[1] = lpnt[1] + lvec[1] * x;
                normal[2] = 0.0;
                u::normalize3(normal);
            } else { normal[0] = 0.0; normal[1] = 0.0; normal[2] = type_ as f64; }
            let old = *normal; u::mul_mat_vec3(normal, mat, &old);
        }
    }
    x
}

fn ray_box(pos: &[f64; 3], mat: &[f64; 9], size: &[f64; 3], pnt: &[f64; 3], vec: &[f64; 3], mut all: Option<&mut [f64; 6]>, mut normal: Option<&mut [f64; 3]>) -> f64 {
    if let Some(all) = all.as_deref_mut() { *all = [-1.0; 6]; }
    if let Some(normal) = normal.as_deref_mut() { u::zero3(normal); }
    let ssz = size[0]*size[0] + size[1]*size[1] + size[2]*size[2];
    if ray_sphere(pos, None, ssz, pnt, vec, None) < 0.0 { return -1.0; }
    let iface = [[1, 2], [0, 2], [0, 1]];
    let mut lpnt = [0.0; 3]; let mut lvec = [0.0; 3];
    ray_map(pos, mat, pnt, vec, &mut lpnt, &mut lvec);
    let mut x = -1.0; let mut face_side = 0; let mut face_axis = -1;
    for i in 0..3 {
        if lvec[i].abs() > MINVAL {
            for side in (-1..=1).step_by(2) {
                let sol = (side as f64*size[i]-lpnt[i])/lvec[i];
                if sol >= 0.0 {
                    let p0 = lpnt[iface[i][0]] + sol*lvec[iface[i][0]];
                    let p1 = lpnt[iface[i][1]] + sol*lvec[iface[i][1]];
                    if p0.abs() <= size[iface[i][0]] && p1.abs() <= size[iface[i][1]] {
                        if x < 0.0 || sol < x { x = sol; face_axis = i as i32; face_side = side; }
                        if let Some(all) = all.as_deref_mut() { all[2*i+(side+1) as usize/2] = sol; }
                    }
                }
            }
        }
    }
    if let Some(normal) = normal {
        if x >= 0.0 {
            let mut n_local = [0.0; 3]; n_local[face_axis as usize] = face_side as f64;
            u::mul_mat_vec3(normal, mat, &n_local);
        }
    }
    x
}

// mj_rayHfield is omitted: continuous geom pairs do not call it.

pub fn ray_slab(aabb: &[f64; 6], xpos: &[f64; 3], xmat: &[f64; 9], pnt: &[f64; 3], vec: &[f64; 3]) -> i32 {
    let mut tmin = 0.0; let mut tmax = f64::INFINITY;
    let min = [aabb[0] - aabb[3], aabb[1] - aabb[4], aabb[2] - aabb[5]];
    let max = [aabb[0] + aabb[3], aabb[1] + aabb[4], aabb[2] + aabb[5]];
    let mut src = [0.0; 3]; let mut dir = [0.0; 3];
    ray_map(xpos, xmat, pnt, vec, &mut src, &mut dir);
    let invdir = [1.0 / dir[0], 1.0 / dir[1], 1.0 / dir[2]];
    for d in 0..3 {
        let t1 = (min[d] - src[d]) * invdir[d]; let t2 = (max[d] - src[d]) * invdir[d];
        let minval = if t1 < t2 { t1 } else { t2 }; let maxval = if t1 < t2 { t2 } else { t1 };
        tmin = if tmin > minval { tmin } else { minval };
        tmax = if tmax < maxval { tmax } else { maxval };
    }
    (tmin <= tmax) as i32
}

pub fn ray_tree(m: &Model, d: &Data, id: i32, pnt: &[f64; 3], vec: &[f64; 3], mut normal: Option<&mut [f64; 3]>) -> f64 {
    if let Some(normal) = normal.as_deref_mut() { u::zero3(normal); }
    // mark_active and bvh_active writes are visualization only, outside this port.
    let id = id as usize;
    let meshid = m.geom_dataid[id];
    if meshid == -1 { panic!("mesh id of geom {} is -1", meshid); }
    let meshid = meshid as usize;
    let bvhadr = m.mesh_bvhadr[meshid] as usize;
    let faceid = &m.bvh_nodeid[bvhadr..]; let bvh = &m.bvh_aabb[6*bvhadr..]; let child = &m.bvh_child[2*bvhadr..];
    let mut stack = [0; MAXTREEDEPTH]; let mut nstack = 0;
    stack[nstack] = 0; nstack += 1;
    let mut lpnt = [0.0; 3]; let mut lvec = [0.0; 3];
    ray_map(at3(&d.geom_xpos, 3*id), at9(&d.geom_xmat, 9*id), pnt, vec, &mut lpnt, &mut lvec);
    let mut b0 = [1.0; 3]; let mut b1 = [0.0; 3];
    if lvec[0].abs() >= lvec[1].abs() && lvec[0].abs() >= lvec[2].abs() { b0[0] = 0.0; }
    else if lvec[1].abs() >= lvec[2].abs() { b0[1] = 0.0; } else { b0[2] = 0.0; }
    u::add_scl3(&mut b1, &b0, &lvec, -u::dot3(&lvec, &b0)/u::dot3(&lvec, &lvec));
    u::normalize3(&mut b1); cross(&mut b0, &b1, &lvec); u::normalize3(&mut b0);
    let mut x = -1.0; let mut normal_local = [0.0; 3];
    while nstack != 0 {
        nstack -= 1; let node = stack[nstack];
        let intersect = ray_slab(at6(bvh, 6*node), at3(&d.geom_xpos, 3*id), at9(&d.geom_xmat, 9*id), pnt, vec);
        if intersect == 0 { continue; }
        if faceid[node] != -1 {
            let face = (faceid[node] + m.mesh_faceadr[meshid]) as usize;
            let vf = [
                &m.mesh_vert[3*(m.mesh_face[3*face] + m.mesh_vertadr[meshid]) as usize..],
                &m.mesh_vert[3*(m.mesh_face[3*face+1] + m.mesh_vertadr[meshid]) as usize..],
                &m.mesh_vert[3*(m.mesh_face[3*face+2] + m.mesh_vertadr[meshid]) as usize..],
            ];
            let mut v = [[0.0; 3]; 3];
            for i in 0..3 { for j in 0..3 { v[i][j] = vf[i][j] as f64; } }
            let sol = ray_triangle(&v, &lpnt, &lvec, &b0, &b1, if normal.is_some() { Some(&mut normal_local) } else { None });
            if sol >= 0.0 && (x < 0.0 || sol < x) {
                x = sol;
                if let Some(normal) = normal.as_deref_mut() { u::copy3(normal, &normal_local); }
            }
            continue;
        }
        for i in 0..2 {
            if child[2*node+i] != -1 {
                if nstack >= MAXTREEDEPTH { panic!("BVH stack depth exceeded in geom {}.", id); }
                stack[nstack] = child[2*node+i] as usize; nstack += 1;
            }
        }
    }
    if let Some(normal) = normal {
        if x >= 0.0 { let old = *normal; u::mul_mat_vec3(normal, at9(&d.geom_xmat, 9*id), &old); }
    }
    x
}

// mj_raySdf is omitted: continuous geom pairs do not call it.

pub fn ray_mesh(m: &Model, d: &Data, id: i32, pnt: &[f64; 3], vec: &[f64; 3], mut normal: Option<&mut [f64; 3]>) -> f64 {
    if let Some(normal) = normal.as_deref_mut() { u::zero3(normal); }
    if m.geom_type[id as usize] != GEOM_MESH { panic!("geom with mesh type expected"); }
    if ray_box(at3(&d.geom_xpos, 3*id as usize), at9(&d.geom_xmat, 9*id as usize), at3(&m.geom_size, 3*id as usize), pnt, vec, None, None) < 0.0 { return -1.0; }
    ray_tree(m, d, id, pnt, vec, normal)
}

pub fn ray_geom(pos: &[f64; 3], mat: Option<&[f64; 9]>, size: &[f64; 3], pnt: &[f64; 3], vec: &[f64; 3], geomtype: i32, normal: Option<&mut [f64; 3]>) -> f64 {
    match geomtype {
        GEOM_PLANE => ray_plane(pos, mat.expect("plane orientation"), size, pnt, vec, normal),
        GEOM_SPHERE => ray_sphere(pos, mat, size[0] * size[0], pnt, vec, normal),
        GEOM_CAPSULE => ray_capsule(pos, mat.expect("capsule orientation"), size, pnt, vec, normal),
        GEOM_ELLIPSOID => ray_ellipsoid(pos, mat.expect("ellipsoid orientation"), size, pnt, vec, normal),
        GEOM_CYLINDER => ray_cylinder(pos, mat.expect("cylinder orientation"), size, pnt, vec, normal),
        GEOM_BOX => ray_box(pos, mat.expect("box orientation"), size, pnt, vec, None, normal),
        _ => panic!("unexpected geom type {}", geomtype),
    }
}

// mj_rayFlex, mju_raySkin, mj_ray, mju_multiRayPrepare and mj_multiRay are omitted
// with their private helpers point_in_box and mju_singleRay: none is called by
// continuous geom-pair collision.
