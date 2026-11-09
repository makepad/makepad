//! Convex supports, hulls, contacts and directed Hausdorff distance.
//!
//! Translated from `src/engine/engine_collision_convex.c` and its header of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::collision_gjk::{ccd, ccd_size, CcdConfig, CcdObj, CcdStatus, CcdSupport};
use crate::data::{Data, PreContact};
use crate::inline::{self as inl, at3, at3_mut, at4, at9};
use crate::memory::Scratch;
use crate::model::{disabled, Model};
use crate::types::*;
use crate::util_blas as u;
use crate::util_misc::{max, min};
use crate::util_spatial::make_frame;

pub const MESH_HILLCLIMB_MIN: usize = 10;
const MINVAL2: f64 = MINVAL * MINVAL;

// The ccd_buffer/mjc_setCCDBuffer pair is replaced by Scratch's owned pool.
// prism_firstdir and _libccd_wrapper are omitted with libccd.

// find penetration info between two geoms; returns number of collisions found
fn penetration(m: &Model, scratch: &mut Scratch, obj1: &mut CcdObj<'_>, obj2: &mut CcdObj<'_>, con: &mut [PreContact], nconmax: usize, margin: f64) -> usize {
    if disabled(m, DSBL_NATIVECCD) { crate::unported!("libccd"); }
    let npolygonmax = if disabled(m, DSBL_MULTICCD) { 0 } else { m.npolygonmax };
    let nmeshdegmax = if disabled(m, DSBL_MULTICCD) { 0 } else { m.nmeshdegmax };
    let size = ccd_size(npolygonmax, nmeshdegmax, m.opt.ccd_iterations as usize);
    let mut buffer = scratch.ccd_buffer(size);
    let mut config = CcdConfig {
        max_iterations: m.opt.ccd_iterations as usize,
        tolerance: m.opt.ccd_tolerance,
        max_contacts: nconmax,
        dist_cutoff: 0.0,
        npolygonmax: m.npolygonmax,
        nmeshdegmax: m.nmeshdegmax,
        buffer: Some(&mut buffer),
    };
    let mut status = CcdStatus::default();
    let mut ncon = 0;
    if ccd(&mut config, &mut status, obj1, obj2) < 0.0 {
        let nwitness = status.nx;
        for i in 0..nwitness {
            con[i].dist = margin + status.dist[i];
            inl::add3(&mut con[i].pos, at3(&status.x1, 3*i), at3(&status.x2, 3*i));
            let pos = con[i].pos;
            u::scl3(&mut con[i].pos, &pos, 0.5);
            inl::sub3(&mut con[i].normal, at3(&status.x1, 3*i), at3(&status.x2, 3*i));
            u::normalize3(&mut con[i].normal);
            inl::zero3(&mut con[i].tangent);
        }
        ncon = nwitness;
    }
    scratch.put_ccd_buffer(buffer, size);
    ncon
}

// mjccd_center is omitted: libccd is not translated.
pub fn center(res: &mut [f64; 3], obj: &CcdObj<'_>) {
    let g = obj.geom;
    let f = obj.flex;
    let e = obj.elem;
    let v = obj.vert;
    if obj.geom_type == GEOM_HFIELD {
        u::zero3(res);
        for i in 0..6 { inl::add_to3(res, &obj.data.hfield.prism[i]); }
        let old = *res;
        u::scl3(res, &old, 1.0/6.0);
        return;
    }
    if g >= 0 { inl::copy3(res, &obj.pos); return; }
    if e >= 0 { inl::copy3(res, at3(obj.data.flex.aabb, 0)); return; }
    if f >= 0 { inl::copy3(res, at3(obj.data.flex.vert_xpos, 3*v as usize)); }
}

// transform a vector from global to local frame
fn mul_mat_tvec3(res: &mut [f64; 3], mat: &[f64; 9], dir: &[f64; 3]) {
    res[0] = mat[0]*dir[0] + mat[3]*dir[1] + mat[6]*dir[2];
    res[1] = mat[1]*dir[0] + mat[4]*dir[1] + mat[7]*dir[2];
    res[2] = mat[2]*dir[0] + mat[5]*dir[1] + mat[8]*dir[2];
}
fn local_to_global(res: &mut [f64; 3], mat: &[f64; 9], dir: &[f64; 3], pos: &[f64; 3]) {
    res[0] = mat[0]*dir[0] + mat[1]*dir[1] + mat[2]*dir[2];
    res[1] = mat[3]*dir[0] + mat[4]*dir[1] + mat[5]*dir[2];
    res[2] = mat[6]*dir[0] + mat[7]*dir[1] + mat[8]*dir[2];
    res[0] += pos[0];
    res[1] += pos[1];
    res[2] += pos[2];
}

pub fn point_support(res: &mut [f64; 3], obj: &CcdObj<'_>, _dir: &[f64; 3]) {
    inl::copy3(res, &obj.pos);
}
fn sphere_support(res: &mut [f64; 3], obj: &CcdObj<'_>, dir: &[f64; 3]) {
    let pos = &obj.pos;
    let radius = obj.size[0];
    res[0] = radius*dir[0] + pos[0];
    res[1] = radius*dir[1] + pos[1];
    res[2] = radius*dir[2] + pos[2];
}

pub fn line_support(res: &mut [f64; 3], obj: &CcdObj<'_>, dir: &[f64; 3]) {
    let mat = &obj.mat;
    let pos = &obj.pos;
    let length = obj.size[1];
    let dot = mat[2] * dir[0] + mat[5] * dir[1] + mat[8] * dir[2];
    let scl = if dot >= 0.0 { length } else { -length };
    res[0] = mat[2] * scl + pos[0];
    res[1] = mat[5] * scl + pos[1];
    res[2] = mat[8] * scl + pos[2];
}
fn capsule_support(res: &mut [f64; 3], obj: &CcdObj<'_>, dir: &[f64; 3]) {
    let mat = &obj.mat;
    let pos = &obj.pos;
    let radius = obj.size[0];
    let length = obj.size[1];
    let mut local_dir = [0.0; 3];
    let mut local_supp = [0.0; 3];
    mul_mat_tvec3(&mut local_dir, mat, dir);
    local_supp[0] = local_dir[0] * radius;
    local_supp[1] = local_dir[1] * radius;
    local_supp[2] = local_dir[2] * radius;
    local_supp[2] += if local_dir[2] >= 0.0 { length } else { -length };
    local_to_global(res, mat, &local_supp, pos);
}
// Public for the direct support-function proof.
pub fn ellipsoid_support(res: &mut [f64; 3], obj: &CcdObj<'_>, dir: &[f64; 3]) {
    let mat = &obj.mat;
    let pos = &obj.pos;
    let size = &obj.size;
    let mut local_dir = [0.0; 3];
    let mut local_supp = [0.0; 3];
    mul_mat_tvec3(&mut local_dir, mat, dir);
    local_supp[0] = local_dir[0] * size[0];
    local_supp[1] = local_dir[1] * size[1];
    local_supp[2] = local_dir[2] * size[2];
    let norm2 = local_supp[0]*local_supp[0] + local_supp[1]*local_supp[1] + local_supp[2]*local_supp[2];
    if norm2 < MINVAL2 {
        res[0] = mat[0]*size[0] + pos[0];
        res[1] = mat[3]*size[0] + pos[1];
        res[2] = mat[6]*size[0] + pos[2];
        return;
    }
    let norm_inv = 1.0/norm2.sqrt();
    local_supp[0] *= norm_inv * size[0];
    local_supp[1] *= norm_inv * size[1];
    local_supp[2] *= norm_inv * size[2];
    local_to_global(res, mat, &local_supp, pos);
}
fn cylinder_support(res: &mut [f64; 3], obj: &CcdObj<'_>, dir: &[f64; 3]) {
    let mat = &obj.mat;
    let pos = &obj.pos;
    let size = &obj.size;
    let mut local_dir = [0.0; 3];
    let mut local_supp = [0.0; 3];
    mul_mat_tvec3(&mut local_dir, mat, dir);
    let n2 = local_dir[0]*local_dir[0] + local_dir[1]*local_dir[1];
    let scl = if n2 >= MINVAL2 { size[0] / n2.sqrt() } else { 0.0 };
    local_supp[0] = scl * local_dir[0];
    local_supp[1] = scl * local_dir[1];
    local_supp[2] = if local_dir[2] >= 0.0 { size[1] } else { -size[1] };
    obj.vertindex.set(if local_dir[2] >= 0.0 { 0 } else { 1 });
    local_to_global(res, mat, &local_supp, pos);
}
// Public for the direct support-function proof.
pub fn box_support(res: &mut [f64; 3], obj: &CcdObj<'_>, dir: &[f64; 3]) {
    let mat = &obj.mat;
    let pos = &obj.pos;
    let size = &obj.size;
    let mut local_dir = [0.0; 3];
    let mut local_supp = [0.0; 3];
    mul_mat_tvec3(&mut local_dir, mat, dir);
    local_supp[0] = if local_dir[0] >= 0.0 { size[0] } else { -size[0] };
    local_supp[1] = if local_dir[1] >= 0.0 { size[1] } else { -size[1] };
    local_supp[2] = if local_dir[2] >= 0.0 { size[2] } else { -size[2] };
    obj.vertindex.set(if local_supp[0] > 0.0 { 1 } else { 0 });
    obj.vertindex.set(obj.vertindex.get() | if local_supp[1] > 0.0 { 2 } else { 0 });
    obj.vertindex.set(obj.vertindex.get() | if local_supp[2] > 0.0 { 4 } else { 0 });
    local_to_global(res, mat, &local_supp, pos);
}
fn dot3f(a: &[f64; 3], b: &[f32]) -> f64 {
    a[0]*b[0] as f64 + a[1]*b[1] as f64 + a[2]*b[2] as f64
}
fn mesh_support(res: &mut [f64; 3], obj: &CcdObj<'_>, dir: &[f64; 3]) {
    let mat = &obj.mat;
    let pos = &obj.pos;
    let verts = obj.data.mesh.vert;
    let nverts = obj.data.mesh.nvert;
    let mut local_dir = [0.0; 3];
    mul_mat_tvec3(&mut local_dir, mat, dir);
    let mut max = -(f32::MAX as f64);
    let mut imax = 0;
    if obj.vertindex.get() >= 0 {
        imax = obj.vertindex.get() as usize;
        max = dot3f(&local_dir, &verts[3*imax..]);
    }
    for i in 0..nverts {
        let vdot = dot3f(&local_dir, &verts[3*i..]);
        if vdot > max { max = vdot; imax = i; }
    }
    obj.vertindex.set(imax as i32);
    local_dir[0] = verts[3*imax] as f64;
    local_dir[1] = verts[3*imax+1] as f64;
    local_dir[2] = verts[3*imax+2] as f64;
    local_to_global(res, mat, &local_dir, pos);
}
fn hillclimb_support(res: &mut [f64; 3], obj: &CcdObj<'_>, dir: &[f64; 3]) {
    let numvert = obj.data.mesh.graph[0] as usize;
    let vert_edgeadr = &obj.data.mesh.graph[2..];
    let vert_globalid = &obj.data.mesh.graph[2+numvert..];
    let edge_localid = &obj.data.mesh.graph[2+2*numvert..];
    let verts = obj.data.mesh.vert;
    let pos = &obj.pos;
    let mat = &obj.mat;
    let mut local_dir = [0.0; 3];
    mul_mat_tvec3(&mut local_dir, mat, dir);
    let mut prev = -1;
    let mut imax;
    let cx = (local_dir[0] > 0.4) as i32 - (local_dir[0] < -0.4) as i32 + 1;
    let cy = (local_dir[1] > 0.4) as i32 - (local_dir[1] < -0.4) as i32 + 1;
    let cz = (local_dir[2] > 0.4) as i32 - (local_dir[2] < -0.4) as i32 + 1;
    let grid_idx = obj.data.mesh.extrema[(cx*9+cy*3+cz) as usize];
    if obj.meshindex.get() >= 0 {
        let cached_dot = dot3f(&local_dir, &verts[3*vert_globalid[obj.meshindex.get() as usize] as usize..]);
        let seed_dot = dot3f(&local_dir, &verts[3*vert_globalid[grid_idx as usize] as usize..]);
        imax = if seed_dot > cached_dot { grid_idx } else { obj.meshindex.get() };
    } else { imax = grid_idx; }
    let mut max = dot3f(&local_dir, &verts[3*vert_globalid[imax as usize] as usize..]);
    while imax != prev {
        prev = imax;
        let mut i = vert_edgeadr[imax as usize] as usize;
        while edge_localid[i] >= 0 {
            let subidx = edge_localid[i];
            let vdot = dot3f(&local_dir, &verts[3*vert_globalid[subidx as usize] as usize..]);
            if vdot > max { max = vdot; imax = subidx; }
            i += 1;
        }
    }
    obj.meshindex.set(imax);
    imax = vert_globalid[imax as usize];
    obj.vertindex.set(imax);
    local_dir[0] = verts[3*imax as usize] as f64;
    local_dir[1] = verts[3*imax as usize+1] as f64;
    local_dir[2] = verts[3*imax as usize+2] as f64;
    local_to_global(res, mat, &local_dir, pos);
}
fn prism_support(res: &mut [f64; 3], obj: &CcdObj<'_>, dir: &[f64; 3]) {
    let prism = &obj.data.hfield.prism;
    let istart = if dir[2] < 0.0 { 0 } else { 3 };
    let mut ibest = istart;
    let mut best = u::dot3(&prism[istart], dir);
    for i in 1..3 {
        let tmp = u::dot3(&prism[istart+i], dir);
        if tmp > best { ibest = istart+i; best = tmp; }
    }
    inl::copy3(res, &prism[ibest]);
}
fn flex_support(res: &mut [f64; 3], obj: &CcdObj<'_>, dir: &[f64; 3]) {
    let f = obj.flex as usize;
    let dim = obj.data.flex.dim[f] as usize;
    if obj.elem >= 0 {
        let edata = obj.data.flex.elem;
        let vert = obj.data.flex.vert_xpos;
        inl::copy3(res, at3(vert, 3*edata[0] as usize));
        let mut best = u::dot3(res, dir);
        for i in 1..=dim {
            let dot = u::dot3(at3(vert, 3*edata[i] as usize), dir);
            if dot > best { best = dot; inl::copy3(res, at3(vert, 3*edata[i] as usize)); }
        }
        inl::add_to_scl3(res, dir, obj.data.flex.xradius[f] + 0.5*obj.margin);
    } else {
        let vert = at3(obj.data.flex.vert_xpos, 3*obj.vert as usize);
        inl::add_scl3(res, vert, dir, obj.data.flex.xradius[f] + 0.5*obj.margin);
    }
}

// mjccd_support (including its AVX implementation) is omitted with libccd.
impl CcdSupport {
    /// Calls the support function this tag names (C: `obj->support(res, obj, dir)`).
    pub fn call(self, res: &mut [f64; 3], obj: &CcdObj<'_>, dir: &[f64; 3]) {
        match self {
            CcdSupport::Null => panic!("invalid ccd object or null support function"),
            CcdSupport::Point => point_support(res, obj, dir),
            CcdSupport::Sphere => sphere_support(res, obj, dir),
            CcdSupport::Line => line_support(res, obj, dir),
            CcdSupport::Capsule => capsule_support(res, obj, dir),
            CcdSupport::Ellipsoid => ellipsoid_support(res, obj, dir),
            CcdSupport::Cylinder => cylinder_support(res, obj, dir),
            CcdSupport::Box => box_support(res, obj, dir),
            CcdSupport::Mesh => mesh_support(res, obj, dir),
            CcdSupport::Hillclimb => hillclimb_support(res, obj, dir),
            CcdSupport::Prism => prism_support(res, obj, dir),
            CcdSupport::Flex => flex_support(res, obj, dir),
        }
    }
}
fn init_ccd_obj_support<'a>(obj: &mut CcdObj<'a>, m: &'a Model, dataid: i32) {
    match obj.geom_type {
        GEOM_ELLIPSOID => obj.support = CcdSupport::Ellipsoid,
        GEOM_MESH | GEOM_SDF => {
            if dataid >= 0 {
                let id = dataid as usize;
                let graphadr = m.mesh_graphadr[id];
                let vertadr = m.mesh_vertadr[id] as usize;
                let polyadr = m.mesh_polyadr[id] as usize;
                if graphadr < 0 || (m.mesh_vertnum[id] as usize) < MESH_HILLCLIMB_MIN {
                    obj.data.mesh.graph = &[];
                    obj.data.mesh.extrema = &[];
                    obj.support = CcdSupport::Mesh;
                } else {
                    obj.data.mesh.graph = &m.mesh_graph[graphadr as usize..];
                    obj.data.mesh.extrema = &m.mesh_extrema[27*id..];
                    obj.support = CcdSupport::Hillclimb;
                }
                obj.data.mesh.vert = &m.mesh_vert[3*vertadr..];
                obj.data.mesh.nvert = m.mesh_vertnum[id] as usize;
                obj.data.mesh.mpolymapadr = &m.mesh_polymapadr[vertadr..];
                obj.data.mesh.mpolymapnum = &m.mesh_polymapnum[vertadr..];
                obj.data.mesh.polymap = &m.mesh_polymap;
                obj.data.mesh.polynormal = &m.mesh_polynormal[3*polyadr..];
                obj.data.mesh.polyvertadr = &m.mesh_polyvertadr[polyadr..];
                obj.data.mesh.polyvertnum = &m.mesh_polyvertnum[polyadr..];
                obj.data.mesh.polyvert = &m.mesh_polyvert;
                obj.data.mesh.mesh_polynum = m.mesh_polynum[id] as usize;
            } else { obj.support = CcdSupport::Null; }
        }
        GEOM_SPHERE => obj.support = CcdSupport::Sphere,
        GEOM_CAPSULE => obj.support = CcdSupport::Capsule,
        GEOM_CYLINDER => obj.support = CcdSupport::Cylinder,
        GEOM_BOX => obj.support = CcdSupport::Box,
        GEOM_HFIELD => {
            obj.center = center;
            obj.support = CcdSupport::Prism;
            if dataid >= 0 {
                let id = dataid as usize;
                obj.data.hfield.hfield_nrow = m.hfield_nrow[id] as usize;
                obj.data.hfield.hfield_ncol = m.hfield_ncol[id] as usize;
                u::copy(&mut obj.size, &m.hfield_size[4*id..], 4);
                obj.data.hfield.hfield_data = &m.hfield_data[m.hfield_adr[id] as usize..];
            }
        }
        _ => obj.support = CcdSupport::Null,
    }
}
pub fn init_ccd_obj<'a>(obj: &mut CcdObj<'a>, m: &'a Model, d: Option<&'a Data>, g: i32, margin: f64) {
    obj.geom = g;
    obj.margin = margin;
    obj.center = center;
    obj.vertindex.set(-1);
    obj.meshindex.set(-1);
    obj.nsupp = 0;
    obj.flex = -1;
    obj.elem = -1;
    obj.vert = -1;
    u::zero4(&mut obj.rotate);
    obj.rotate[0] = 1.0;
    if g >= 0 {
        let g = g as usize;
        u::copy(&mut obj.size, &m.geom_size[3*g..], 3);
        if let Some(d) = d {
            u::copy(&mut obj.pos, &d.geom_xpos[3*g..], 3);
            u::copy(&mut obj.mat, &d.geom_xmat[9*g..], 9);
        }
        obj.geom_type = m.geom_type[g];
        init_ccd_obj_support(obj, m, m.geom_dataid[g]);
    } else {
        obj.geom_type = GEOM_FLEX;
        obj.data.flex.dim = &m.flex_dim;
        obj.support = CcdSupport::Flex;
        obj.data.flex.aabb = d.map_or(&[], |d| d.flexelem_aabb.as_slice());
        obj.data.flex.elemadr = &m.flex_elemadr;
        obj.data.flex.vert_xpos = d.map_or(&[], |d| d.flexvert_xpos.as_slice());
        obj.data.flex.vertadr = &m.flex_vertadr;
        obj.data.flex.xradius = &m.flex_radius;
        obj.data.flex.elemdataadr = &m.flex_elemdataadr;
        obj.data.flex.elem = &m.flex_elem;
    }
}
pub fn init_ccd_obj_site<'a>(obj: &mut CcdObj<'a>, m: &'a Model, d: &'a Data, s: i32, margin: f64) {
    obj.geom = -1;
    obj.margin = margin;
    obj.center = center;
    obj.vertindex.set(-1);
    obj.meshindex.set(-1);
    obj.nsupp = 0;
    obj.flex = -1;
    obj.elem = -1;
    obj.vert = -1;
    u::zero4(&mut obj.rotate);
    obj.rotate[0] = 1.0;
    if s >= 0 && (s as usize) < m.nsite {
        let s = s as usize;
        let dataid = m.site_dataid[s];
        inl::copy3(at3_mut(&mut obj.size, 0), at3(&m.site_size, 3*s));
        obj.size[3] = 0.0;
        obj.geom_type = m.site_type[s];
        if obj.geom_type == GEOM_MESH && dataid >= 0 {
            let mut xquat = [0.0; 4];
            inl::mul_quat(&mut xquat, at4(&d.site_xquat, 4*s), at4(&m.mesh_quat, 4*dataid as usize));
            inl::quat2mat(&mut obj.mat, &xquat);
            inl::mul_mat_vec3(&mut obj.pos, at9(&d.site_xmat, 9*s), at3(&m.mesh_pos, 3*dataid as usize));
            inl::add_to3(&mut obj.pos, at3(&d.site_xpos, 3*s));
        } else {
            inl::copy3(&mut obj.pos, at3(&d.site_xpos, 3*s));
            inl::copy9(&mut obj.mat, at9(&d.site_xmat, 9*s));
        }
        init_ccd_obj_support(obj, m, dataid);
    }
}
fn set_ccd_obj_flex(obj: &mut CcdObj<'_>, flex: i32, elem: i32, vert: i32) {
    obj.flex = flex;
    obj.elem = elem;
    obj.vert = vert;
    if flex >= 0 {
        let f = flex as usize;
        obj.data.flex.vert_xpos = &obj.data.flex.vert_xpos[3*obj.data.flex.vertadr[f] as usize..];
        if elem >= 0 {
            let dim = obj.data.flex.dim[f];
            obj.data.flex.aabb = &obj.data.flex.aabb[6*(obj.data.flex.elemadr[f]+elem) as usize..];
            obj.data.flex.elem = &obj.data.flex.elem[(obj.data.flex.elemdataadr[f]+elem*(dim+1)) as usize..];
        }
    }
}
fn is_distinct_contact(con: &[PreContact], ncon: usize, tolerance: f64) -> bool {
    let last_pos = &con[ncon-1].pos;
    for i in 0..ncon-1 { if u::dist3(&con[i].pos, last_pos) <= tolerance { return false; } }
    true
}
fn rotate_frame(origin: &[f64; 3], rot: &[f64; 9], xmat: &mut [f64; 9], xpos: &mut [f64; 3]) {
    let mut mat = [0.0; 9];
    let mut vec = [0.0; 3];
    let mut rel = [0.0; 3];
    u::mul_mat_mat3(&mut mat, rot, xmat);
    u::copy(xmat, &mat, 9);
    inl::sub3(&mut rel, origin, xpos);
    u::mul_mat_vec3(&mut vec, rot, &rel);
    u::sub_from3(&mut vec, &rel);
    inl::sub_from3(xpos, &vec);
}
fn max_contacts(m: &Model, obj1: &CcdObj<'_>, obj2: &CcdObj<'_>) -> usize {
    if disabled(m, DSBL_NATIVECCD) { return 1; }
    let type1 = obj1.geom_type;
    let type2 = obj2.geom_type;
    if type1 == GEOM_BOX && type2 == GEOM_BOX { return 8; }
    if disabled(m, DSBL_MULTICCD) { return 1; }
    let hasface1 = type1 == GEOM_BOX || type1 == GEOM_MESH || type1 == GEOM_CYLINDER;
    let hasface2 = type2 == GEOM_BOX || type2 == GEOM_MESH || type2 == GEOM_CYLINDER;
    let hasedge1 = type1 == GEOM_CAPSULE;
    let hasedge2 = type2 == GEOM_CAPSULE;
    if hasface1 && hasface2 { return 4; }
    if (hasedge1 && hasface2) || (hasface1 && hasedge2) || (hasedge1 && hasedge2) { return 2; }
    1
}

pub fn convex(m: &Model, d: &mut Data, con: &mut [PreContact], g1: i32, g2: i32, margin: f64) -> usize {
    // Move the scratch pool out while CCD objects borrow Data's geometry.
    // The closure keeps every C early return inside the pool's return scope.
    let mut scratch = std::mem::take(&mut d.scratch);
    let result = (|| {
        let mut obj1 = CcdObj::default();
        let mut obj2 = CcdObj::default();
        init_ccd_obj(&mut obj1, m, Some(d), g1, margin);
        init_ccd_obj(&mut obj2, m, Some(d), g2, margin);
        let max_contacts = max_contacts(m, &obj1, &obj2);
        let isellipsoid1 = obj1.geom_type == GEOM_ELLIPSOID || obj1.geom_type == GEOM_SPHERE;
        let isellipsoid2 = obj2.geom_type == GEOM_ELLIPSOID || obj2.geom_type == GEOM_SPHERE;
        let mut ncon = penetration(m, &mut scratch, &mut obj1, &mut obj2, con, max_contacts, margin);
        if disabled(m, DSBL_NATIVECCD) && ncon != 0 && g1 >= 0 && g2 >= 0 { fix_normal(m, d, &mut con[0], g1, g2); }
        if max_contacts > 1 || isellipsoid1 || isellipsoid2 { return ncon; }
        if ncon == 1 && !disabled(m, DSBL_MULTICCD) && !isellipsoid1 && !isellipsoid2 {
            let relative_tolerance = 1e-3;
            let perturbation_angle = 1e-3;
            let mut frame = [0.0; 9];
            inl::copy3(at3_mut(&mut frame, 0), &con[0].normal);
            u::zero(&mut frame[3..], 6);
            make_frame(&mut frame);
            let tolerance = relative_tolerance * min(m.geom_rbound[g1 as usize], m.geom_rbound[g2 as usize]);
            let axes = [at3(&frame, 3), at3(&frame, 6)];
            let angles = [-perturbation_angle, perturbation_angle];
            for axis_id in 0..2 {
                for angle_id in 0..2 {
                    let axis = axes[axis_id];
                    let angle = angles[angle_id];
                    let mut quat = [0.0; 4];
                    let mut rot = [0.0; 9];
                    inl::axis_angle2quat(&mut quat, axis, angle);
                    crate::util_spatial::quat2mat(&mut rot, &quat);
                    rotate_frame(&con[0].pos, &rot, &mut obj1.mat, &mut obj1.pos);
                    let mut invrot = [0.0; 9];
                    u::transpose(&mut invrot, &rot, 3, 3);
                    rotate_frame(&con[0].pos, &invrot, &mut obj2.mat, &mut obj2.pos);
                    let n = penetration(m, &mut scratch, &mut obj1, &mut obj2, &mut con[ncon..], 1, margin);
                    if disabled(m, DSBL_NATIVECCD) && n != 0 && g1 >= 0 && g2 >= 0 { fix_normal(m, d, &mut con[ncon], g1, g2); }
                    if n != 0 && is_distinct_contact(con, ncon+1, tolerance) { con[ncon].dist = con[0].dist; ncon += 1; }
                    inl::copy3(&mut obj1.pos, at3(&d.geom_xpos, 3*g1 as usize));
                    inl::copy9(&mut obj1.mat, at9(&d.geom_xmat, 9*g1 as usize));
                    inl::copy3(&mut obj2.pos, at3(&d.geom_xpos, 3*g2 as usize));
                    inl::copy9(&mut obj2.mat, at9(&d.geom_xmat, 9*g2 as usize));
                }
            }
        }
        ncon
    })();
    d.scratch = scratch;
    result
}

fn sub3f(res: &mut [f64; 3], a: &[f32], b: &[f32]) {
    res[0] = a[0] as f64 - b[0] as f64;
    res[1] = a[1] as f64 - b[1] as f64;
    res[2] = a[2] as f64 - b[2] as f64;
}
fn area4f(hull: &[f32], idx: &[i32], a: usize, b: usize, c: usize, d: usize) -> f64 {
    let mut ca = [0.0; 3];
    let mut db = [0.0; 3];
    let mut cross = [0.0; 3];
    sub3f(&mut ca, &hull[3*idx[a] as usize..], &hull[3*idx[c] as usize..]);
    sub3f(&mut db, &hull[3*idx[b] as usize..], &hull[3*idx[d] as usize..]);
    inl::cross(&mut cross, &ca, &db);
    0.5 * u::norm3(&cross)
}
fn hull4f(res: &mut [usize; 4], hull: &[f32], idx: &[i32], nhull: usize, a: usize) -> usize {
    let mut b = (a+1)%nhull;
    let mut c = (a+2)%nhull;
    let mut d = (a+3)%nhull;
    *res = [a, b, c, d];
    if nhull <= 4 { return nhull; }
    let mut m = area4f(hull, idx, a, b, c, d);
    loop {
        let d_next = (d+1)%nhull;
        let mut m_next = area4f(hull, idx, a, b, c, d_next);
        if m_next <= m { break; }
        d = d_next;
        m = m_next;
        *res = [a, b, c, d];
        loop {
            let c_next = (c+1)%nhull;
            m_next = area4f(hull, idx, a, b, c_next, d);
            if m_next <= m { break; }
            c = c_next;
            m = m_next;
            *res = [a, b, c, d];
        }
        loop {
            let b_next = (b+1)%nhull;
            m_next = area4f(hull, idx, a, b_next, c, d);
            if m_next <= m { break; }
            b = b_next;
            m = m_next;
            *res = [a, b, c, d];
        }
    }
    4
}

pub fn plane_convex(m: &Model, d: &mut Data, con: &mut [PreContact], g1: i32, g2: i32, margin: f64) -> usize {
    let pos1 = at3(&d.geom_xpos, 3*g1 as usize);
    let mat1 = at9(&d.geom_xmat, 9*g1 as usize);
    let pos2 = at3(&d.geom_xpos, 3*g2 as usize);
    let mat2 = at9(&d.geom_xmat, 9*g2 as usize);
    let mut diff = [0.0; 3];
    let normal = [mat1[2], mat1[5], mat1[8]];
    let mut obj = CcdObj::default();
    init_ccd_obj(&mut obj, m, Some(d), g2, 0.0);
    let mut v = [0.0; 3];
    let dir = [-mat1[2], -mat1[5], -mat1[8]];
    obj.support.call(&mut v, &obj, &dir);
    inl::sub3(&mut diff, &v, pos1);
    let dist = u::dot3(&normal, &diff);
    if dist > margin { return 0; }
    con[0].dist = dist;
    inl::copy3(&mut con[0].pos, &v);
    inl::add_to_scl3(&mut con[0].pos, &normal, -0.5*dist);
    inl::copy3(&mut con[0].normal, &normal);
    inl::zero3(&mut con[0].tangent);
    if m.geom_dataid[g2 as usize] == -1 || obj.data.mesh.mesh_polynum == 0 || obj.vertindex.get() < 0 { return 1; }
    let vertadr = m.mesh_vertadr[m.geom_dataid[g2 as usize] as usize] as usize;
    let polyadr = m.mesh_polyadr[m.geom_dataid[g2 as usize] as usize] as usize;
    let polymapadr = m.mesh_polymapadr[vertadr+obj.vertindex.get() as usize] as usize;
    let polymapnum = m.mesh_polymapnum[vertadr+obj.vertindex.get() as usize] as usize;
    let mut best_poly = -1;
    let mut best_dot = 1.0;
    let mut local_normal = [0.0; 3];
    u::mul_mat_tvec3(&mut local_normal, mat2, &normal);
    for i in 0..polymapnum {
        let idx = m.mesh_polymap[polymapadr+i];
        let pn = at3(&m.mesh_polynormal, 3*(polyadr+idx as usize));
        let ndot = pn[0]*local_normal[0] + pn[1]*local_normal[1] + pn[2]*local_normal[2];
        if ndot < best_dot { best_dot = ndot; best_poly = idx; }
    }
    if best_poly < 0 { return 1; }
    let face_vertadr = m.mesh_polyvertadr[polyadr+best_poly as usize] as usize;
    let face_vertnum = m.mesh_polyvertnum[polyadr+best_poly as usize] as usize;
    let vertdata = &m.mesh_vert[3*vertadr..];
    let face_polyvert = &m.mesh_polyvert[face_vertadr..];
    let mut a = 0;
    for i in 0..face_vertnum {
        if face_polyvert[i] == obj.vertindex.get() { a = i; break; }
    }
    let mut ncon = 1;
    let mut indices = [0; 4];
    let n = hull4f(&mut indices, vertdata, face_polyvert, face_vertnum, a);
    for i in 1..n {
        let local_vert = [
            vertdata[3*face_polyvert[indices[i]] as usize] as f64,
            vertdata[3*face_polyvert[indices[i]] as usize+1] as f64,
            vertdata[3*face_polyvert[indices[i]] as usize+2] as f64,
        ];
        let mut pnt = [0.0; 3];
        local_to_global(&mut pnt, mat2, &local_vert, pos2);
        inl::sub3(&mut diff, &pnt, pos1);
        let vdist = u::dot3(&normal, &diff);
        inl::sub3(&mut diff, &pnt, pos2);
        if vdist > margin || u::dot3(&normal, &diff) > 0.0 { continue; }
        con[ncon].dist = vdist;
        inl::copy3(&mut con[ncon].pos, &pnt);
        inl::add_to_scl3(&mut con[ncon].pos, &normal, -0.5*vdist);
        inl::copy3(&mut con[ncon].normal, &normal);
        inl::zero3(&mut con[ncon].tangent);
        ncon += 1;
    }
    ncon
}

fn add_vert(obj: &mut CcdObj<'_>, x: f64, y: f64, z: f64) {
    let prism = &mut obj.data.hfield.prism;
    let old = prism[1]; inl::copy3(&mut prism[0], &old);
    let old = prism[2]; inl::copy3(&mut prism[1], &old);
    let old = prism[4]; inl::copy3(&mut prism[3], &old);
    let old = prism[5]; inl::copy3(&mut prism[4], &old);
    prism[5][0] = x; prism[2][0] = x;
    prism[5][1] = y; prism[2][1] = y;
    prism[5][2] = z;
}
fn add_prism_vert(obj: &mut CcdObj<'_>, r: i32, c: i32, i: i32, dx: f64, dy: f64) {
    let prism = &mut obj.data.hfield.prism;
    let old = prism[1]; inl::copy3(&mut prism[0], &old);
    let old = prism[2]; inl::copy3(&mut prism[1], &old);
    let old = prism[4]; inl::copy3(&mut prism[3], &old);
    let old = prism[5]; inl::copy3(&mut prism[4], &old);
    let dr = 1-i;
    prism[5][0] = dx*c as f64 - obj.size[0]; prism[2][0] = prism[5][0];
    prism[5][1] = dy*(r+dr) as f64 - obj.size[1]; prism[2][1] = prism[5][1];
    prism[5][2] = obj.data.hfield.hfield_data[(r+dr) as usize*obj.data.hfield.hfield_ncol+c as usize] as f64*obj.size[2];
}

pub fn convex_hfield(m: &Model, d: &mut Data, con: &mut [PreContact], g1: i32, g2: i32, margin: f64) -> usize {
    let mut scratch = std::mem::take(&mut d.scratch);
    let result = (|| {
        let pos1 = at3(&d.geom_xpos, 3*g1 as usize);
        let mat1 = at9(&d.geom_xmat, 9*g1 as usize);
        let pos2 = at3(&d.geom_xpos, 3*g2 as usize);
        let mat2 = at9(&d.geom_xmat, 9*g2 as usize);
        let hid = m.geom_dataid[g1 as usize] as usize;
        let nrow = m.hfield_nrow[hid];
        let ncol = m.hfield_ncol[hid];
        let size0 = m.hfield_size[4*hid];
        let size1 = m.hfield_size[4*hid+1];
        let size2 = m.hfield_size[4*hid+2];
        let size3 = m.hfield_size[4*hid+3];
        let mut local_pos = [pos2[0]-pos1[0], pos2[1]-pos1[1], pos2[2]-pos1[2]];
        let old = local_pos;
        u::mul_mat_tvec3(&mut local_pos, mat1, &old);
        let radius = m.geom_rbound[g2 as usize] + margin;
        if size0 < local_pos[0]-radius || -size0 > local_pos[0]+radius ||
            size1 < local_pos[1]-radius || -size1 > local_pos[1]+radius ||
            size2 < local_pos[2]-radius || -size3 > local_pos[2]+radius { return 0; }
        let mut obj1 = CcdObj::default();
        let mut obj2 = CcdObj::default();
        init_ccd_obj(&mut obj1, m, Some(d), g1, margin);
        init_ccd_obj(&mut obj2, m, Some(d), g2, 0.0);
        let mut mat = [0.0; 9];
        inl::mul_mat_tmat3(&mut mat, mat1, mat2);
        inl::copy9(&mut obj2.mat, &mat);
        inl::copy3(&mut obj2.pos, &local_pos);
        let mut local_dir = [0.0; 3];
        let mut res = [0.0; 3];
        local_dir[0] = 1.0;
        obj2.support.call(&mut res, &obj2, &local_dir);
        let mut xmax = res[0];
        local_dir[0] = -1.0;
        obj2.support.call(&mut res, &obj2, &local_dir);
        let mut xmin = res[0];
        local_dir[0] = 0.0;
        local_dir[1] = 1.0;
        obj2.support.call(&mut res, &obj2, &local_dir);
        let mut ymax = res[1];
        local_dir[1] = -1.0;
        obj2.support.call(&mut res, &obj2, &local_dir);
        let mut ymin = res[1];
        local_dir[1] = 0.0;
        local_dir[2] = 1.0;
        obj2.support.call(&mut res, &obj2, &local_dir);
        let mut zmax = res[2];
        local_dir[2] = -1.0;
        obj2.support.call(&mut res, &obj2, &local_dir);
        let mut zmin = res[2];
        xmin -= margin;
        xmax += margin;
        ymin -= margin;
        ymax += margin;
        zmin -= margin;
        zmax += margin;
        if xmin > size0 || xmax < -size0 || ymin > size1 || ymax < -size1 || zmin > size2 || zmax < -size3 { return 0; }
        let mut cmin = ((xmin+size0)/(2.0*size0)*(ncol-1) as f64).floor() as i32;
        let mut cmax = ((xmax+size0)/(2.0*size0)*(ncol-1) as f64).ceil() as i32;
        let mut rmin = ((ymin+size1)/(2.0*size1)*(nrow-1) as f64).floor() as i32;
        let mut rmax = ((ymax+size1)/(2.0*size1)*(nrow-1) as f64).ceil() as i32;
        cmin = 0.max(cmin);
        cmax = (ncol-1).min(cmax);
        rmin = 0.max(rmin);
        rmax = (nrow-1).min(rmax);
        obj2.margin = margin;
        let dx = (2.0*size0)/(ncol-1) as f64;
        let dy = (2.0*size1)/(nrow-1) as f64;
        obj1.data.hfield.prism[2][2] = -size3;
        obj1.data.hfield.prism[1][2] = -size3;
        obj1.data.hfield.prism[0][2] = -size3;
        let mut ncon = 0;
        'rows: for r in rmin..rmax {
            add_prism_vert(&mut obj1, r, cmin, 0, dx, dy);
            add_prism_vert(&mut obj1, r, cmin, 1, dx, dy);
            for c in cmin+1..=cmax {
                for i in 0..2 {
                    add_prism_vert(&mut obj1, r, c, i, dx, dy);
                    let prism = &obj1.data.hfield.prism;
                    if prism[3][2] < zmin && prism[4][2] < zmin && prism[5][2] < zmin { continue; }
                    if penetration(m, &mut scratch, &mut obj1, &mut obj2, &mut con[ncon..], 1, margin) != 0 {
                        inl::copy3(&mut local_dir, &con[ncon].normal);
                        inl::copy3(&mut local_pos, &con[ncon].pos);
                        inl::mul_mat_vec3(&mut con[ncon].normal, mat1, &local_dir);
                        inl::mul_mat_vec3(&mut con[ncon].pos, mat1, &local_pos);
                        inl::add_to3(&mut con[ncon].pos, pos1);
                        ncon += 1;
                        if ncon >= MAXCONPAIR { break 'rows; }
                    }
                }
            }
        }
        if disabled(m, DSBL_NATIVECCD) {
            for i in 0..ncon { fix_normal(m, d, &mut con[i], g1, g2); }
        }
        ncon
    })();
    d.scratch = scratch;
    result
}

// compute normal for point outside ellipsoid, using ray-projection SQP
fn ellipsoid_inside(nrm: &mut [f64; 3], pos: &[f64; 3], size: &[f64; 3]) -> i32 {
    let maxiter = 30;
    let tolerance = 1e-6;
    let s2inv = [1.0/(size[0]*size[0]), 1.0/(size[1]*size[1]), 1.0/(size[2]*size[2])];
    let c = pos[0]*pos[0]*s2inv[0] + pos[1]*pos[1]*s2inv[1] + pos[2]*pos[2]*s2inv[2] - 1.0;
    if c > 0.0 { return 0; }
    u::normalize3(nrm);
    for iter in 0..maxiter {
        let a = nrm[0]*nrm[0]*s2inv[0] + nrm[1]*nrm[1]*s2inv[1] + nrm[2]*nrm[2]*s2inv[2];
        let b = pos[0]*nrm[0]*s2inv[0] + pos[1]*nrm[1]*s2inv[1] + pos[2]*nrm[2]*s2inv[2];
        let det = b*b - a*c;
        if det < MINVAL || a < MINVAL { return (iter > 0) as i32; }
        let x = (-b + det.sqrt())/a;
        if x < 0.0 { return (iter > 0) as i32; }
        let mut pnt = [0.0; 3];
        inl::add_scl3(&mut pnt, pos, nrm, x);
        let mut newnrm = [pnt[0]*s2inv[0], pnt[1]*s2inv[1], pnt[2]*s2inv[2]];
        u::normalize3(&mut newnrm);
        let change = u::dist3(nrm, &newnrm);
        inl::copy3(nrm, &newnrm);
        if change < tolerance { break; }
    }
    1
}
// compute normal for point inside ellipsoid, using diagonal QCQP
fn ellipsoid_outside(nrm: &mut [f64; 3], pos: &[f64; 3], size: &[f64; 3]) -> i32 {
    let maxiter = 30;
    let tolerance = 1e-6;
    let s2 = [size[0]*size[0], size[1]*size[1], size[2]*size[2]];
    let ps2 = [pos[0]*pos[0]*s2[0], pos[1]*pos[1]*s2[1], pos[2]*pos[2]*s2[2]];
    let mut la = 0.0;
    for _iter in 0..maxiter {
        let r = [1.0/(s2[0]+la), 1.0/(s2[1]+la), 1.0/(s2[2]+la)];
        let val = ps2[0]*r[0]*r[0] + ps2[1]*r[1]*r[1] + ps2[2]*r[2]*r[2] - 1.0;
        if val < tolerance { break; }
        let deriv = -2.0*(ps2[0]*r[0]*r[0]*r[0] + ps2[1]*r[1]*r[1]*r[1] + ps2[2]*r[2]*r[2]*r[2]);
        if deriv > -MINVAL { break; }
        let delta = -val/deriv;
        if delta < tolerance { break; }
        la += delta;
    }
    nrm[0] = pos[0]/(s2[0]+la);
    nrm[1] = pos[1]/(s2[1]+la);
    nrm[2] = pos[2]/(s2[2]+la);
    u::normalize3(nrm);
    1
}

pub fn fix_normal(m: &Model, d: &Data, con: &mut PreContact, g1: i32, g2: i32) {
    let gid = [g1, g2];
    let mut type_ = [GEOM_NONE; 2];
    for i in 0..2 {
        type_[i] = if gid[i] < 0 { GEOM_NONE } else { m.geom_type[gid[i] as usize] };
        if type_[i] != GEOM_SPHERE && type_[i] != GEOM_CAPSULE && type_[i] != GEOM_ELLIPSOID && type_[i] != GEOM_CYLINDER { type_[i] = GEOM_NONE; }
    }
    if type_[0] == GEOM_NONE && type_[1] == GEOM_NONE { return; }
    let mut normal = [con.normal, [-con.normal[0], -con.normal[1], -con.normal[2]]];
    let mut processed = [0; 2];
    for i in 0..2 {
        if type_[i] != GEOM_NONE {
            let mat = at9(&d.geom_xmat, 9*gid[i] as usize);
            let size = at3(&m.geom_size, 3*gid[i] as usize);
            let mut dif = [0.0; 3];
            let mut pos1 = [0.0; 3];
            let mut nrm = [0.0; 3];
            u::sub3(&mut dif, &con.pos, at3(&d.geom_xpos, 3*gid[i] as usize));
            u::mul_mat_tvec3(&mut pos1, mat, &dif);
            u::mul_mat_tvec3(&mut nrm, mat, &normal[i]);
            match type_[i] {
                GEOM_SPHERE => { inl::copy3(&mut nrm, &pos1); processed[i] = 1; }
                GEOM_CAPSULE => {
                    if pos1[2] < -size[1] { nrm[2] = pos1[2]+size[1]; }
                    else if pos1[2] > size[1] { nrm[2] = pos1[2]-size[1]; }
                    else { nrm[2] = 0.0; }
                    nrm[0] = pos1[0];
                    nrm[1] = pos1[1];
                    processed[i] = 1;
                }
                GEOM_ELLIPSOID => {
                    if !(size[0] < MINVAL || size[1] < MINVAL || size[2] < MINVAL) {
                        let dst1 = pos1[0]*pos1[0]/(size[0]*size[0]) + pos1[1]*pos1[1]/(size[1]*size[1]) + pos1[2]*pos1[2]/(size[2]*size[2]);
                        processed[i] = if dst1 <= 1.0 { ellipsoid_inside(&mut nrm, &pos1, size) } else { ellipsoid_outside(&mut nrm, &pos1, size) };
                    }
                }
                GEOM_CYLINDER => {
                    // C's switch breaks skip only this arm, not the enclosing loop.
                    if !(pos1[2].abs() > 0.95*size[1]) {
                        let dst1 = (size[1]-pos1[2].abs()).abs();
                        let dst2 = (size[0]-u::norm(&pos1, 2)).abs();
                        if !(dst1 < 0.25*dst2) {
                            nrm[0] = pos1[0];
                            nrm[1] = pos1[1];
                            nrm[2] = 0.0;
                            processed[i] = 1;
                        }
                    }
                }
                _ => {}
            }
            if processed[i] != 0 {
                u::normalize3(&mut nrm);
                inl::mul_mat_vec3(&mut normal[i], mat, &nrm);
            }
        }
    }
    if processed[0] != 0 && processed[1] != 0 {
        inl::sub3(&mut con.normal, &normal[0], &normal[1]);
        u::normalize3(&mut con.normal);
    } else if processed[0] != 0 { inl::copy3(&mut con.normal, &normal[0]); }
    else if processed[1] != 0 { inl::scl3(&mut con.normal, &normal[1], -1.0); }
}

pub fn convex_elem(m: &Model, d: &mut Data, con: &mut [PreContact], g1: i32, f1: i32, e1: i32, v1: i32, f2: i32, e2: i32, margin: f64) -> usize {
    let mut scratch = std::mem::take(&mut d.scratch);
    let result = (|| {
        let mut obj1 = CcdObj::default();
        let mut obj2 = CcdObj::default();
        init_ccd_obj(&mut obj1, m, Some(d), g1, margin);
        init_ccd_obj(&mut obj2, m, Some(d), -1, margin);
        set_ccd_obj_flex(&mut obj1, f1, e1, v1);
        set_ccd_obj_flex(&mut obj2, f2, e2, -1);
        let ncon = penetration(m, &mut scratch, &mut obj1, &mut obj2, con, 1, margin);
        if ncon != 0 && !disabled(m, DSBL_NATIVECCD) {
            let mut isflex2d = false;
            if f1 >= 0 && m.flex_dim[f1 as usize] == 2 { isflex2d = true; }
            if f2 >= 0 && m.flex_dim[f2 as usize] == 2 { isflex2d = true; }
            if isflex2d {
                for i in 0..ncon { fix_normal(m, d, &mut con[i], g1, -1); }
            }
        }
        ncon
    })();
    d.scratch = scratch;
    result
}

pub fn hfield_elem(m: &Model, d: &mut Data, con: &mut [PreContact], g: i32, f: i32, e: i32, margin: f64) -> usize {
    let mut scratch = std::mem::take(&mut d.scratch);
    let result = (|| {
        let mut vec = [0.0; 3];
        let mut obj1 = CcdObj::default();
        init_ccd_obj(&mut obj1, m, Some(d), g, margin);
        let hid = m.geom_dataid[g as usize] as usize;
        let nrow = m.hfield_nrow[hid];
        let ncol = m.hfield_ncol[hid];
        let hpos = at3(&d.geom_xpos, 3*g as usize);
        let hmat = at9(&d.geom_xmat, 9*g as usize);
        let hsize = at4(&m.hfield_size, 4*hid);
        let hdata = &m.hfield_data[m.hfield_adr[hid] as usize..];
        let dim = m.flex_dim[f as usize] as usize;
        let edata = &m.flex_elem[m.flex_elemdataadr[f as usize] as usize+e as usize*(dim+1)..];
        let local_edata = [0, 1, 2, 3];
        let mut evert = [0.0; 12];
        let mut ecenter = [0.0; 3];
        for i in 0..=dim {
            let v = at3(&d.flexvert_xpos, 3*(m.flex_vertadr[f as usize]+edata[i]) as usize);
            inl::sub3(&mut vec, v, hpos);
            inl::mul_mat_tvec3(at3_mut(&mut evert, 3*i), hmat, &vec);
        }
        inl::sub3(&mut vec, at3(&d.flexelem_aabb, 6*(m.flex_elemadr[f as usize]+e) as usize), hpos);
        inl::mul_mat_tvec3(&mut ecenter, hmat, &vec);
        let mut obj2 = CcdObj::default();
        init_ccd_obj(&mut obj2, m, Some(d), -1, margin);
        set_ccd_obj_flex(&mut obj2, f, e, -1);
        obj2.data.flex.elem = &local_edata;
        obj2.data.flex.vert_xpos = &evert;
        obj2.data.flex.aabb = &ecenter;
        let mut xmin = evert[0];
        let mut xmax = evert[0];
        let mut ymin = evert[1];
        let mut ymax = evert[1];
        let mut zmin = evert[2];
        let mut zmax = evert[2];
        for i in 1..=dim {
            xmin = min(xmin, evert[3*i]);
            xmax = max(xmax, evert[3*i]);
            ymin = min(ymin, evert[3*i+1]);
            ymax = max(ymax, evert[3*i+1]);
            zmin = min(zmin, evert[3*i+2]);
            zmax = max(zmax, evert[3*i+2]);
        }
        let expand = m.flex_radius[f as usize]+margin;
        xmin -= expand;
        xmax += expand;
        ymin -= expand;
        ymax += expand;
        zmin -= expand;
        zmax += expand;
        if xmin > hsize[0] || xmax < -hsize[0] || ymin > hsize[1] || ymax < -hsize[1] || zmin > hsize[2] || zmax < -hsize[3] { return 0; }
        let mut cmin = ((xmin+hsize[0])/(2.0*hsize[0])*(ncol-1) as f64).floor() as i32;
        let mut cmax = ((xmax+hsize[0])/(2.0*hsize[0])*(ncol-1) as f64).ceil() as i32;
        let mut rmin = ((ymin+hsize[1])/(2.0*hsize[1])*(nrow-1) as f64).floor() as i32;
        let mut rmax = ((ymax+hsize[1])/(2.0*hsize[1])*(nrow-1) as f64).ceil() as i32;
        cmin = 0.max(cmin);
        cmax = (ncol-1).min(cmax);
        rmin = 0.max(rmin);
        rmax = (nrow-1).min(rmax);
        let dx = (2.0*hsize[0])/(ncol-1) as f64;
        let dy = (2.0*hsize[1])/(nrow-1) as f64;
        let dr = [1, 0];
        obj1.data.hfield.prism[2][2] = -hsize[3];
        obj1.data.hfield.prism[1][2] = -hsize[3];
        obj1.data.hfield.prism[0][2] = -hsize[3];
        let mut cnt = 0;
        'rows: for r in rmin..rmax {
            let mut nvert = 0;
            for c in cmin..=cmax {
                for k in 0..2 {
                    add_vert(&mut obj1, dx*c as f64-hsize[0], dy*(r+dr[k]) as f64-hsize[1], hdata[((r+dr[k])*ncol+c) as usize] as f64*hsize[2]);
                    nvert += 1;
                    if nvert > 2 {
                        let prism = &obj1.data.hfield.prism;
                        if prism[3][2] < zmin && prism[4][2] < zmin && prism[5][2] < zmin { continue; }
                        if penetration(m, &mut scratch, &mut obj1, &mut obj2, &mut con[cnt..], 1, margin) != 0 {
                            inl::zero3(&mut con[cnt].tangent);
                            let old = con[cnt].normal;
                            u::mul_mat_vec3(&mut con[cnt].normal, hmat, &old);
                            let old = con[cnt].pos;
                            u::mul_mat_vec3(&mut con[cnt].pos, hmat, &old);
                            inl::add_to3(&mut con[cnt].pos, hpos);
                            cnt += 1;
                            if cnt >= MAXCONPAIR { break 'rows; }
                        }
                    }
                }
            }
        }
        cnt
    })();
    d.scratch = scratch;
    result
}

// Approximate lower bound of the directed Hausdorff distance between compact convex geoms.
pub fn hausdorff(obj1: &mut CcdObj<'_>, obj2: &mut CcdObj<'_>, nitermax: i32, stepsize: f64, tolerance: f64) -> f64 {
    if obj1.support == CcdSupport::Null || obj2.support == CcdSupport::Null {
        panic!("invalid ccd object or null support function");
    }
    let type1 = obj1.geom_type;
    let type2 = obj2.geom_type;
    if type1 < GEOM_SPHERE || type1 > GEOM_MESH || type2 < GEOM_SPHERE || type2 > GEOM_MESH {
        panic!("only compact convex shapes are supported, got types {} and {}", type1, type2);
    }
    let mut x_k = [[0.0; 3]; 6];
    let mut best_x = [[0.0; 3]; 6];
    let mut best_grad = [[0.0; 3]; 6];
    let mut best_val = [0.0; 6];
    let mut step = [0.0; 6];
    let mut active = [true; 6];
    for s in 0..6 {
        let axis = s/2;
        let sgn = if s%2 != 0 { -1.0 } else { 1.0 };
        x_k[s][0] = sgn*obj2.mat[axis];
        x_k[s][1] = sgn*obj2.mat[3+axis];
        x_k[s][2] = sgn*obj2.mat[6+axis];
        best_val[s] = -MAXVAL;
        step[s] = stepsize;
    }
    for _k in 0..nitermax {
        let mut any_active = false;
        for s in 0..6 {
            if !active[s] { continue; }
            let mut v1 = [0.0; 3];
            let mut v2 = [0.0; 3];
            let mut vert = [0.0; 3];
            obj1.support.call(&mut v1, obj1, &x_k[s]);
            obj2.support.call(&mut v2, obj2, &x_k[s]);
            inl::sub3(&mut vert, &v1, &v2);
            let val = u::dot3(&vert, &x_k[s]);
            let mut grad = [0.0; 3];
            let mut scaled_x = [0.0; 3];
            u::scl3(&mut scaled_x, &x_k[s], val);
            u::sub3(&mut grad, &vert, &scaled_x);
            let grad_norm = u::norm3(&grad);
            if val > best_val[s] {
                best_val[s] = val;
                u::copy3(&mut best_x[s], &x_k[s]);
                if grad_norm <= MINVAL || grad_norm <= tolerance*val.abs() { active[s] = false; continue; }
                u::scl3(&mut best_grad[s], &grad, 1.0/grad_norm);
            } else {
                step[s] *= 0.5;
                if step[s] < tolerance { active[s] = false; continue; }
                if grad_norm > MINVAL { u::add_to_scl3(&mut best_grad[s], &grad, 1.0/grad_norm); }
                let proj = u::dot3(&best_grad[s], &best_x[s]);
                u::add_to_scl3(&mut best_grad[s], &best_x[s], -proj);
                let avg_norm = u::norm3(&best_grad[s]);
                if avg_norm <= tolerance { active[s] = false; continue; }
                let old = best_grad[s];
                u::scl3(&mut best_grad[s], &old, 1.0/avg_norm);
            }
            u::copy3(&mut x_k[s], &best_x[s]);
            u::add_to_scl3(&mut x_k[s], &best_grad[s], step[s]);
            u::normalize3(&mut x_k[s]);
            any_active = true;
        }
        if !any_active { break; }
    }
    let mut max_val = best_val[0];
    for s in 1..6 { max_val = max(max_val, best_val[s]); }
    max_val
}
