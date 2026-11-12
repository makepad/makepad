//! Ray intersections with geoms: the primitive, mesh, height field and SDF rays, the scene ray
//! (`ray`) and the many-ray cast from one point (`multi_ray`) the rangefinder sensor uses.
//!
//! Translated from `src/engine/engine_ray.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::collision_sdf::{distance, get_sdf, gradient, SdfQuery};
use crate::data::Data;
use crate::inline::{at3, at3_mut, at4, at6, at9};
use crate::model::Model;
use crate::types::*;
use crate::util_blas as u;
use crate::util_errmem::error;
use crate::util_misc::{max, min, round};
use crate::util_spatial::cross;

/// mjNGROUP: the number of geom groups a ray's group filter covers.
pub const NGROUP: usize = 6;

fn ray_map(pos: &[f64; 3], mat: &[f64; 9], pnt: &[f64; 3], vec: &[f64; 3], lpnt: &mut [f64; 3], lvec: &mut [f64; 3]) {
    let dif = [pnt[0] - pos[0], pnt[1] - pos[1], pnt[2] - pos[2]];
    lpnt[0] = mat[0]*dif[0] + mat[3]*dif[1] + mat[6]*dif[2];
    lpnt[1] = mat[1]*dif[0] + mat[4]*dif[1] + mat[7]*dif[2];
    lpnt[2] = mat[2]*dif[0] + mat[5]*dif[1] + mat[8]*dif[2];
    lvec[0] = mat[0]*vec[0] + mat[3]*vec[1] + mat[6]*vec[2];
    lvec[1] = mat[1]*vec[0] + mat[4]*vec[1] + mat[7]*vec[2];
    lvec[2] = mat[2]*vec[0] + mat[5]*vec[1] + mat[8]*vec[2];
}

// map to azimuth angle in spherical coordinates
fn longitude(vec: &[f64; 3]) -> f64 {
    vec[1].atan2(vec[0])
}

// map to elevation angle in spherical coordinates
fn latitude(vec: &[f64; 3]) -> f64 {
    (vec[0]*vec[0] + vec[1]*vec[1]).sqrt().atan2(vec[2])
}

// whether a ray skips the geom: on the excluded body, invisible (by its colour or its material's),
// static when static geoms are left out, or in a group the group filter leaves out
fn ray_eliminate(m: &Model, geomid: usize, geomgroup: Option<&[u8; NGROUP]>, flg_static: bool, bodyexclude: i32) -> bool {
    let bodyid = m.geom_bodyid[geomid];
    if bodyid == bodyexclude {
        return true;
    }
    let matid = m.geom_matid[geomid];
    if matid < 0 && m.geom_rgba[4*geomid+3] == 0.0 {
        return true;
    }
    if matid >= 0 && m.mat_rgba[4*matid as usize+3] == 0.0 {
        return true;
    }
    if !flg_static && m.body_weldid[bodyid as usize] == 0 {
        return true;
    }
    let Some(geomgroup) = geomgroup else { return false };
    let groupid = (NGROUP as i32 - 1).min(0.max(m.geom_group[geomid]));
    geomgroup[groupid as usize] == 0
}

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

// basis vectors of the plane normal to a local ray direction, for ray_triangle
fn ray_basis(lvec: &[f64; 3], b0: &mut [f64; 3], b1: &mut [f64; 3]) {
    *b0 = [1.0; 3];
    if lvec[0].abs() >= lvec[1].abs() && lvec[0].abs() >= lvec[2].abs() { b0[0] = 0.0; }
    else if lvec[1].abs() >= lvec[2].abs() { b0[1] = 0.0; } else { b0[2] = 0.0; }
    u::add_scl3(b1, b0, lvec, -u::dot3(lvec, b0)/u::dot3(lvec, lvec));
    u::normalize3(b1); cross(b0, b1, lvec); u::normalize3(b0);
}

/// mj_rayHfield: the ray's distance to a height field geom, or -1; the normal at the hit if asked.
pub fn ray_hfield(m: &Model, d: &Data, geomid: i32, pnt: &[f64; 3], vec: &[f64; 3], mut normal: Option<&mut [f64; 3]>) -> f64 {
    if let Some(normal) = normal.as_deref_mut() { u::zero3(normal); }
    let g = geomid as usize;
    if m.geom_type[g] != GEOM_HFIELD { error("geom with hfield type expected"); }
    let hid = m.geom_dataid[g] as usize;
    let nrow = m.hfield_nrow[hid];
    let ncol = m.hfield_ncol[hid];
    let size = at4(&m.hfield_size, 4*hid);
    let data = &m.hfield_data[m.hfield_adr[hid] as usize..];
    let want = normal.is_some();

    // the base box under the surface and the top box holding it
    let base_size = [size[0], size[1], size[3]*0.5];
    let xmat = at9(&d.geom_xmat, 9*g);
    let xpos = at3(&d.geom_xpos, 3*g);
    let base_pos = [xpos[0] - xmat[2]*size[3]*0.5, xpos[1] - xmat[5]*size[3]*0.5, xpos[2] - xmat[8]*size[3]*0.5];
    let top_size = [size[0], size[1], size[2]*0.5];
    let top_pos = [xpos[0] + xmat[2]*size[2]*0.5, xpos[1] + xmat[5]*size[2]*0.5, xpos[2] + xmat[8]*size[2]*0.5];

    let mut normal_base = [0.0; 3];
    let mut x = ray_box(&base_pos, xmat, &base_size, pnt, vec, None, if want { Some(&mut normal_base) } else { None });

    // done if the top box is missed or the grid is degenerate
    let mut all = [0.0; 6];
    let top_intersect = ray_box(&top_pos, xmat, &top_size, pnt, vec, Some(&mut all), None);
    if top_intersect < 0.0 || nrow < 2 || ncol < 2 {
        if let Some(normal) = normal { if x >= 0.0 { u::copy3(normal, &normal_base); } }
        return x;
    }

    let mut lpnt = [0.0; 3]; let mut lvec = [0.0; 3];
    ray_map(xpos, xmat, pnt, vec, &mut lpnt, &mut lvec);
    let mut b0 = [0.0; 3]; let mut b1 = [0.0; 3];
    ray_basis(&lvec, &mut b0, &mut b1);

    // the ray's segment inside the top box
    let mut seg = [0.0, top_intersect];
    for i in 0..6 {
        if all[i] > seg[1] { seg[0] = top_intersect; seg[1] = all[i]; }
    }

    // the segment's ends in grid cells, padded by one
    let dx = (2.0*size[0]) / (ncol-1) as f64;
    let dy = (2.0*size[1]) / (nrow-1) as f64;
    let mut sx = [0.0; 2]; let mut sy = [0.0; 2];
    for i in 0..2 {
        sx[i] = (lpnt[0] + seg[i]*lvec[0] + size[0]) / dx;
        sy[i] = (lpnt[1] + seg[i]*lvec[1] + size[1]) / dy;
    }
    let lo = |a: f64, b: f64| if a < b { a } else { b };
    let hi = |a: f64, b: f64| if a > b { a } else { b };
    let cmin = 0.max(lo(sx[0], sx[1]).floor() as i32 - 1);
    let cmax = (ncol-1).min(hi(sx[0], sx[1]).ceil() as i32 + 1);
    let rmin = 0.max(lo(sy[0], sy[1]).floor() as i32 - 1);
    let rmax = (nrow-1).min(hi(sy[0], sy[1]).ceil() as i32 + 1);

    // the local normal starts as the base box's
    let mut normal_local = [0.0; 3];
    if want && x >= 0.0 { u::mul_mat_tvec3(&mut normal_local, xmat, &normal_base); }

    let h = |k: i32| data[k as usize] as f64 * size[2];
    for r in rmin..rmax {
        for c in cmin..cmax {
            let (rf, cf) = (r as f64, c as f64);
            let mut normal_tri = [0.0; 3];
            // first triangle, wound counterclockwise so its normal points up
            let va = [
                [dx*cf-size[0], dy*rf-size[1], h(r*ncol+c)],
                [dx*(cf+1.0)-size[0], dy*rf-size[1], h(r*ncol+(c+1))],
                [dx*(cf+1.0)-size[0], dy*(rf+1.0)-size[1], h((r+1)*ncol+(c+1))],
            ];
            let sol = ray_triangle(&va, &lpnt, &lvec, &b0, &b1, if want { Some(&mut normal_tri) } else { None });
            if sol >= 0.0 && (x < 0.0 || sol < x) {
                x = sol;
                if want { normal_local = normal_tri; }
            }
            // second triangle
            let vb = [
                [dx*cf-size[0], dy*rf-size[1], h(r*ncol+c)],
                [dx*(cf+1.0)-size[0], dy*(rf+1.0)-size[1], h((r+1)*ncol+(c+1))],
                [dx*cf-size[0], dy*(rf+1.0)-size[1], h((r+1)*ncol+c)],
            ];
            let sol = ray_triangle(&vb, &lpnt, &lvec, &b0, &b1, if want { Some(&mut normal_tri) } else { None });
            if sol >= 0.0 && (x < 0.0 || sol < x) {
                x = sol;
                if want { normal_local = normal_tri; }
            }
        }
    }

    // the top box's sides, where the ray passes under the edge profile
    for i in 0..4 {
        if all[i] >= 0.0 && (all[i] < x || x < 0.0) {
            let z = (lpnt[2] + all[i]*lvec[2]) / size[2];
            let (y, y0, z0, z1);
            if i < 2 {
                // side normal to the x axis
                y = (lpnt[1] + all[i]*lvec[1] + size[1]) / dy;
                y0 = hi(0.0, lo((nrow-2) as f64, y.floor()));
                let col = if i == 1 { ncol-1 } else { 0 };
                z0 = data[(round(y0)*ncol + col) as usize] as f64;
                z1 = data[(round(y0+1.0)*ncol + col) as usize] as f64;
            } else {
                // side normal to the y axis
                y = (lpnt[0] + all[i]*lvec[0] + size[0]) / dx;
                y0 = hi(0.0, lo((ncol-2) as f64, y.floor()));
                let row = if i == 3 { (nrow-1)*ncol } else { 0 };
                z0 = data[(round(y0) + row) as usize] as f64;
                z1 = data[(round(y0+1.0) + row) as usize] as f64;
            }
            if z < z0*(y0+1.0-y) + z1*(y-y0) {
                x = all[i];
                if want {
                    normal_local = [0.0; 3];
                    match i {
                        0 => normal_local[0] = -1.0,
                        1 => normal_local[0] = 1.0,
                        2 => normal_local[1] = -1.0,
                        _ => normal_local[1] = 1.0,
                    }
                }
            }
        }
    }

    // the normal in the global frame
    if let Some(normal) = normal {
        if x >= 0.0 { u::mul_mat_vec3(normal, xmat, &normal_local); }
    }
    x
}

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
    let mut b0 = [0.0; 3]; let mut b1 = [0.0; 3];
    ray_basis(&lvec, &mut b0, &mut b1);
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

// mj_raySdf: the ray's distance to a signed distance field geom by ray marching, or -1; the normal
// at the hit if asked. The reference resets the field plugin's evaluation counter here; our fields keep
// none.
fn ray_sdf(m: &Model, d: &Data, g: i32, pnt: &[f64; 3], vec: &[f64; 3], mut normal: Option<&mut [f64; 3]>) -> f64 {
    if let Some(normal) = normal.as_deref_mut() { u::zero3(normal); }
    let gi = g as usize;
    let mut distance_total = 0.0;
    let mut p = [0.0; 3];
    let k_min_dist = 1e-7;

    // exclude by the bounding box
    if ray_box(at3(&d.geom_xpos, 3*gi), at9(&d.geom_xmat, 9*gi), at3(&m.geom_size, 3*gi), pnt, vec, None, None) < 0.0 {
        return -1.0;
    }

    // the field: a plugin, or the mesh's octree
    let instance = m.geom_plugin[gi];
    let sdf_ptr = if instance == -1 { None } else { get_sdf(m, g) };
    let instance = if instance == -1 { m.geom_dataid[gi] } else { instance };
    let sdf = SdfQuery {
        plugin: &[sdf_ptr],
        id: &[instance],
        r#type: SDFTYPE_SINGLE,
        relpos: &[0.0; 3],
        relmat: &[0.0; 9],
        geomtype: &[GEOM_SDF],
    };

    // the ray in the local frame, with a unit direction
    let mut lpnt = [0.0; 3]; let mut lvec = [0.0; 3];
    ray_map(at3(&d.geom_xpos, 3*gi), at9(&d.geom_xmat, 9*gi), pnt, vec, &mut lpnt, &mut lvec);
    let norm = u::normalize3(&mut lvec);
    if norm < MINVAL {
        return -1.0;
    }

    // ray marching
    for _ in 0..40 {
        u::add_scl3(&mut p, &lpnt, &lvec, distance_total);
        let dist = distance(m, d, &sdf, &p).abs();
        distance_total += dist;
        if dist.abs() < k_min_dist {
            if let Some(normal) = normal {
                u::add_scl3(&mut p, &lpnt, &lvec, distance_total);
                gradient(m, d, &sdf, normal, &p);
                u::normalize3(normal);
                let old = *normal; u::mul_mat_vec3(normal, at9(&d.geom_xmat, 9*gi), &old);
            }
            return distance_total / norm;
        }
        if dist > 1e6 {
            // no intersection
            break;
        }
    }
    -1.0
}

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

// mj_rayFlex and mju_raySkin are omitted: they serve picking and drawing, and no engine ray casts
// against flexes or skins.

// whether a point is inside a geom's oriented bounding box
fn point_in_box(aabb: &[f64; 6], xpos: &[f64; 3], xmat: &[f64; 9], pnt: &[f64; 3]) -> bool {
    let mut point = [0.0; 3];
    u::sub3(&mut point, pnt, xpos);
    let old = point; u::mul_mat_tvec3(&mut point, xmat, &old);
    u::sub_from3(&mut point, at3(aabb, 0));
    for j in 0..3 {
        if point[j].abs() > aabb[3+j] {
            return false;
        }
    }
    true
}

// the ray against one geom, by its type
fn ray_dispatch(m: &Model, d: &Data, i: usize, pnt: &[f64; 3], vec: &[f64; 3], normal: Option<&mut [f64; 3]>) -> f64 {
    match m.geom_type[i] {
        GEOM_MESH => ray_mesh(m, d, i as i32, pnt, vec, normal),
        GEOM_HFIELD => ray_hfield(m, d, i as i32, pnt, vec, normal),
        GEOM_SDF => ray_sdf(m, d, i as i32, pnt, vec, normal),
        t => ray_geom(at3(&d.geom_xpos, 3*i), Some(at9(&d.geom_xmat, 9*i)), at3(&m.geom_size, 3*i), pnt, vec, t, normal),
    }
}

/// mj_ray: intersect the ray `pnt + x*vec`, `x >= 0`, with the visible geoms not on body `bodyexclude`
/// (`geomgroup` and `flg_static` as the visualizer's options; no group filter skips none). Returns the
/// distance `x` to the nearest surface, or -1 when the ray hits nothing; the hit geom (-1 without a hit)
/// and the surface normal there if asked.
pub fn ray(m: &Model, d: &Data, pnt: &[f64; 3], vec: &[f64; 3], geomgroup: Option<&[u8; NGROUP]>, flg_static: bool,
    bodyexclude: i32, mut geomid: Option<&mut i32>, mut normal: Option<&mut [f64; 3]>) -> f64 {
    let mut normal_local = [0.0; 3];
    let want = normal.is_some();
    if u::norm3(vec) < MINVAL { error("vector length is too small"); }
    let mut dist = -1.0;
    if let Some(geomid) = geomid.as_deref_mut() { *geomid = -1; }
    if let Some(normal) = normal.as_deref_mut() { u::zero3(normal); }
    for i in 0..m.ngeom {
        if !ray_eliminate(m, i, geomgroup, flg_static, bodyexclude) {
            let newdist = ray_dispatch(m, d, i, pnt, vec, if want { Some(&mut normal_local) } else { None });
            if newdist >= 0.0 && (newdist < dist || dist < 0.0) {
                dist = newdist;
                if let Some(geomid) = geomid.as_deref_mut() { *geomid = i as i32; }
                if let Some(normal) = normal.as_deref_mut() { u::copy3(normal, &normal_local); }
            }
        }
    }
    dist
}

/// mju_multiRayPrepare: for rays from `pnt`, each geom's exclusion flag (`geom_eliminate`, one per geom)
/// and, for geoms of bodies with a bounding volume, the azimuth and elevation bounds of its bounding box
/// as seen from `pnt` (`geom_ba`, four per geom: azimuth and elevation minima, then maxima). Geoms
/// whose bounding sphere lies beyond `cutoff` are excluded. The reference's `ray_xmat`, which must be
/// null, is left out.
pub fn multi_ray_prepare(m: &Model, d: &Data, pnt: &[f64; 3], geomgroup: Option<&[u8; NGROUP]>, flg_static: bool,
    bodyexclude: i32, cutoff: f64, geom_ba: &mut [f64], geom_eliminate: &mut [i32]) {
    for geomid in 0..m.ngeom {
        geom_eliminate[geomid] = ray_eliminate(m, geomid, geomgroup, flg_static, bodyexclude) as i32;
    }
    const OPEN: [f64; 4] = [-PI, 0.0, PI, PI];
    for b in 0..m.nbody {
        // no bounds without a bounding volume
        if m.body_bvhadr[b] == -1 {
            continue;
        }
        for i in 0..m.body_geomnum[b] as usize {
            let g = i + m.body_geomadr[b] as usize;
            let mut ab = [MAXVAL, MAXVAL, -MAXVAL, -MAXVAL];
            let aabb = at6(&m.geom_aabb, 6*g);
            let xpos = at3(&d.geom_xpos, 3*g);
            let xmat = at9(&d.geom_xmat, 9*g);
            if geom_eliminate[g] != 0 {
                continue;
            }
            // excluded when its bounding sphere is beyond the cutoff
            if m.geom_rbound[g] > 0.0 && u::dist3(xpos, pnt) > cutoff + m.geom_rbound[g] {
                geom_eliminate[g] = 1;
                continue;
            }
            if point_in_box(aabb, xpos, xmat, pnt) {
                geom_ba[4*g..4*g+4].copy_from_slice(&OPEN);
                continue;
            }
            // the box corners' spherical coordinates
            for v in 0..8 {
                let mut vert = [
                    if v & 1 != 0 { aabb[0]+aabb[3] } else { aabb[0]-aabb[3] },
                    if v & 2 != 0 { aabb[1]+aabb[4] } else { aabb[1]-aabb[4] },
                    if v & 4 != 0 { aabb[2]+aabb[5] } else { aabb[2]-aabb[5] },
                ];
                let mut corner = [0.0; 3];
                u::mul_mat_vec3(&mut corner, xmat, &vert);
                u::add_to3(&mut corner, xpos);
                u::sub3(&mut vert, &corner, pnt);
                let azimuth = longitude(&vert);
                let elevation = latitude(&vert);
                ab[0] = min(ab[0], azimuth);
                ab[1] = min(ab[1], elevation);
                ab[2] = max(ab[2], azimuth);
                ab[3] = max(ab[3], elevation);
            }
            // a distance-dependent angular margin bounds the faces' curvature between the corners
            let max_half = max(aabb[3], max(aabb[4], aabb[5]));
            let mut center = [0.0; 3];
            u::mul_mat_vec3(&mut center, xmat, at3(aabb, 0));
            u::add_to3(&mut center, xpos);
            let dist = u::dist3(pnt, &center);
            if dist > MINVAL {
                let margin = max_half.atan2(dist);
                ab[0] -= margin;
                ab[1] -= margin;
                ab[2] += margin;
                ab[3] += margin;
            }
            // no angular culling where the azimuth crosses its discontinuity or the elevation overflows
            if ab[2]-ab[0] > PI {
                ab = OPEN;
            }
            if ab[3]-ab[1] > PI {
                ab = OPEN;
            }
            geom_ba[4*g..4*g+4].copy_from_slice(&ab);
        }
    }
}

// mju_singleRay: one ray of a multi-ray cast, culled by bodies' bounding spheres and geoms' angular bounds
fn single_ray(m: &Model, d: &Data, pnt: &[f64; 3], vec: &[f64; 3], eliminate: &[i32], geom_ba: &[f64],
    mut geomid: Option<&mut i32>, mut normal: Option<&mut [f64; 3]>) -> f64 {
    let mut normal_local = [0.0; 3];
    let want = normal.is_some();
    let mut dist = -1.0;
    if let Some(geomid) = geomid.as_deref_mut() { *geomid = -1; }
    if let Some(normal) = normal.as_deref_mut() { u::zero3(normal); }
    let azimuth = longitude(vec);
    let elevation = latitude(vec);
    for b in 0..m.nbody {
        let bvhadr = m.body_bvhadr[b];
        // exclude the body by its bounding sphere
        if bvhadr != -1 {
            let pos = at3(&m.bvh_aabb, 6*bvhadr as usize);
            let size = at3(&m.bvh_aabb, 6*bvhadr as usize + 3);
            let ssz = size[0]*size[0] + size[1]*size[1] + size[2]*size[2];
            let mut center = [0.0; 3];
            u::mul_mat_vec3(&mut center, at9(&d.ximat, 9*b), pos);
            u::add_to3(&mut center, at3(&d.xipos, 3*b));
            if ray_sphere(&center, None, ssz, pnt, vec, None) < 0.0 {
                continue;
            }
        }
        for k in 0..m.body_geomnum[b] as usize {
            let i = m.body_geomadr[b] as usize + k;
            if eliminate[i] != 0 {
                continue;
            }
            // exclude the geom by its angular bounds
            if bvhadr != -1 {
                let (az_min, az_max) = (geom_ba[4*i], geom_ba[4*i+2]);
                let (el_min, el_max) = (geom_ba[4*i+1], geom_ba[4*i+3]);
                if elevation < el_min || elevation > el_max {
                    continue;
                }
                // the azimuth wraps around
                let az_center = (az_min + az_max) * 0.5;
                let az_half_width = (az_max - az_min) * 0.5;
                let mut az_diff = azimuth - az_center;
                if az_diff > PI { az_diff -= 2.0*PI; } else if az_diff < -PI { az_diff += 2.0*PI; }
                if az_diff.abs() > az_half_width {
                    continue;
                }
            }
            let newdist = ray_dispatch(m, d, i, pnt, vec, if want { Some(&mut normal_local) } else { None });
            if newdist >= 0.0 && (newdist < dist || dist < 0.0) {
                dist = newdist;
                if let Some(geomid) = geomid.as_deref_mut() { *geomid = i as i32; }
                if let Some(normal) = normal.as_deref_mut() { u::copy3(normal, &normal_local); }
            }
        }
    }
    dist
}

/// mj_multiRay: `nray` rays from `pnt` along `vec` (three per ray), as [`ray`] casts each one, sped up by
/// culling with bounding volumes; geoms whose bounding sphere lies beyond `cutoff` are left out. A ray
/// shorter than MINVAL hits nothing. Writes each ray's distance (-1 without a hit) and, if asked, the
/// hit geom (-1 without a hit) and the surface normal (three per ray).
pub fn multi_ray(m: &Model, d: &mut Data, pnt: &[f64; 3], vec: &[f64], geomgroup: Option<&[u8; NGROUP]>, flg_static: bool,
    bodyexclude: i32, mut geomid: Option<&mut [i32]>, dist: &mut [f64], mut normal: Option<&mut [f64]>, nray: usize, cutoff: f64) {
    let mut geom_ba = d.scratch.num(4*m.ngeom);
    let mut geom_eliminate = d.scratch.int(m.ngeom);
    multi_ray_prepare(m, d, pnt, geomgroup, flg_static, bodyexclude, cutoff, &mut geom_ba, &mut geom_eliminate);
    for i in 0..nray {
        let v = at3(vec, 3*i);
        if u::norm3(v) < MINVAL {
            dist[i] = -1.0;
            if let Some(geomid) = geomid.as_deref_mut() { geomid[i] = -1; }
            if let Some(normal) = normal.as_deref_mut() { u::zero3(at3_mut(normal, 3*i)); }
        } else {
            dist[i] = single_ray(m, d, pnt, v, &geom_eliminate, &geom_ba,
                geomid.as_deref_mut().map(|g| &mut g[i]), normal.as_deref_mut().map(|n| at3_mut(n, 3*i)));
        }
    }
    d.scratch.put_num(geom_ba);
    d.scratch.put_int(geom_eliminate);
}
