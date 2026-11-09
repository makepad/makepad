//! Compile an explicit rigid-body specification into a physics model.
//!
//! Translated from `src/user/user_model.cc`, `src/user/user_objects.cc`,
//! `src/user/user_mesh.cc` and `src/user/user_util.cc` of the engine named in CREDITS.md (Apache-2.0,
//! Copyright 2021 DeepMind Technologies Limited); changed: translated to Rust.

use crate::model::{check_supported, make_model, Model, MODEL_SIZES};
use crate::model_spec::*;
use crate::types::*;
// mjuu_rotVecQuat has exactly the same scalar operations as mju_rotVecQuat.
use crate::util_spatial::rot_vec_quat;
use std::collections::HashMap;

const EPS: f64 = 1e-14;

// mjCBoundingVolume: body geom IDs are already fixed by ProcessLists. Keeping
// their values replaces the C pointers without changing their order or identity.
struct BoundingVolume {
    id: i32,
    contype: i32,
    conaffinity: i32,
    pos: [f64; 3],
    quat: Option<[f64; 4]>,
    aabb: [f64; 6],
}

struct BvElement {
    e: usize,
    lpos: [f64; 3],
}

struct BoundingVolumeHierarchy {
    bvh: Vec<f64>,
    child: Vec<i32>,
    nodeid: Vec<i32>,
    level: Vec<i32>,
    bvleaf: Vec<BoundingVolume>,
    ipos: [f64; 3],
    iquat: [f64; 4],
}

// The members' initializers of mjCBoundingVolumeHierarchy_: ipos_ = {0, 0, 0}, iquat_ = {1, 0, 0, 0}.
impl Default for BoundingVolumeHierarchy {
    fn default() -> Self {
        Self {
            bvh: Vec::new(), child: Vec::new(), nodeid: Vec::new(), level: Vec::new(),
            bvleaf: Vec::new(), ipos: [0.0; 3], iquat: [1.0, 0.0, 0.0, 0.0],
        }
    }
}

// mjCBoundingVolumeHierarchy, in user_objects.cc order. nodeid also represents
// nodeidptr: body geom IDs do not change after object_lists has assigned them.
impl BoundingVolumeHierarchy {
    fn set(&mut self, ipos_element: &[f64; 3], iquat_element: &[f64; 4]) {
        self.ipos = *ipos_element;
        self.iquat = *iquat_element;
    }

    fn allocate_bounding_volumes(&mut self, nleaf: usize) {
        self.bvh.clear();
        self.child.clear();
        self.nodeid.clear();
        self.level.clear();
        self.bvleaf.clear();
        self.bvleaf.reserve(nleaf);
    }

    // RemoveInactiveVolumes and the integer-ID AddBoundingVolume overload are
    // omitted with their non-body callers. Bodies always supply a quaternion.
    fn add_bounding_volume(&mut self, id: i32, g: &GeomSpec, aabb: &[f64; 6]) {
        self.bvleaf.push(BoundingVolume {
            id, contype: g.contype, conaffinity: g.conaffinity,
            pos: g.pos, quat: Some(g.quat), aabb: *aabb,
        });
    }

    // AddBoundingVolume as mjCMesh::SetBoundingVolume calls it: contype and conaffinity 1,
    // the face circumcenter as position, and no quaternion.
    fn add_bounding_volume_mesh(&mut self, id: i32, pos: &[f64; 3], aabb: &[f64; 6]) {
        self.bvleaf.push(BoundingVolume {
            id, contype: 1, conaffinity: 1, pos: *pos, quat: None, aabb: *aabb,
        });
    }

    fn create_bvh(&mut self) {
        let mut elements = Vec::new();
        self.make(&mut elements);
        self.make_bvh(&mut elements, 0);
    }

    fn make(&self, elements: &mut Vec<BvElement>) {
        elements.reserve(self.bvleaf.len());
        let qinv = [self.iquat[0], -self.iquat[1], -self.iquat[2], -self.iquat[3]];
        for (i, e) in self.bvleaf.iter().enumerate() {
            if e.conaffinity != 0 || e.contype != 0 {
                let mut element = BvElement { e: i, lpos: [0.0; 3] };
                let vert = [e.pos[0] - self.ipos[0], e.pos[1] - self.ipos[1], e.pos[2] - self.ipos[2]];
                rot_vec_quat(&mut element.lpos, &vert, &qinv);
                elements.push(element);
            }
        }
    }

    fn make_bvh(&mut self, elements: &mut [BvElement], lev: usize) -> i32 {
        let nelements = elements.len();
        if nelements == 0 { return -1; }
        // C's AAMM is aamm_node, distinct from each element's aamm.
        let mut aamm_node = [f64::MAX, f64::MAX, f64::MAX, -f64::MAX, -f64::MAX, -f64::MAX];
        let qinv = [self.iquat[0], -self.iquat[1], -self.iquat[2], -self.iquat[3]];
        for element in elements.iter() {
            let e = &self.bvleaf[element.e];
            let aamm = [
                e.aabb[0] - e.aabb[3], e.aabb[1] - e.aabb[4], e.aabb[2] - e.aabb[5],
                e.aabb[0] + e.aabb[3], e.aabb[1] + e.aabb[4], e.aabb[2] + e.aabb[5],
            ];
            for v in 0..8 {
                let mut vert = [
                    if v & 1 != 0 { aamm[3] } else { aamm[0] },
                    if v & 2 != 0 { aamm[4] } else { aamm[1] },
                    if v & 4 != 0 { aamm[5] } else { aamm[2] },
                ];
                // rotate to the body inertial frame if specified
                if let Some(quat) = &e.quat {
                    let mut box_ = [0.0; 3];
                    rot_vec_quat(&mut box_, &vert, quat);
                    box_[0] += e.pos[0] - self.ipos[0];
                    box_[1] += e.pos[1] - self.ipos[1];
                    box_[2] += e.pos[2] - self.ipos[2];
                    rot_vec_quat(&mut vert, &box_, &qinv);
                }
                aamm_node[0] = min(aamm_node[0], vert[0]);
                aamm_node[1] = min(aamm_node[1], vert[1]);
                aamm_node[2] = min(aamm_node[2], vert[2]);
                aamm_node[3] = max(aamm_node[3], vert[0]);
                aamm_node[4] = max(aamm_node[4], vert[1]);
                aamm_node[5] = max(aamm_node[5], vert[2]);
            }
        }
        for i in 0..3 {
            if (aamm_node[i] - aamm_node[i+3]).abs() < EPS {
                aamm_node[i] -= EPS;
                aamm_node[i+3] += EPS;
            }
        }
        let index = self.nodeid.len();
        self.child.extend_from_slice(&[-1, -1]);
        self.nodeid.push(-1);
        self.level.push(lev as i32);
        self.bvh.push((aamm_node[3] + aamm_node[0]) / 2.0);
        self.bvh.push((aamm_node[4] + aamm_node[1]) / 2.0);
        self.bvh.push((aamm_node[5] + aamm_node[2]) / 2.0);
        self.bvh.push((aamm_node[3] - aamm_node[0]) / 2.0);
        self.bvh.push((aamm_node[4] - aamm_node[1]) / 2.0);
        self.bvh.push((aamm_node[5] - aamm_node[2]) / 2.0);
        if nelements == 1 {
            self.nodeid[index] = self.bvleaf[elements[0].e].id;
            return index as i32;
        }
        let mut axis = 0;
        let edges = [aamm_node[3] - aamm_node[0], aamm_node[4] - aamm_node[1], aamm_node[5] - aamm_node[2]];
        if edges[1] >= edges[0] + EPS { axis = 1; }
        if edges[2] >= edges[axis] + EPS { axis = 2; }
        // The reference uses nth_element, not sort. Position keys can tie:
        // its pointer comparison orders them by their original bvleaf index.
        // Preserve that tie-break in Rust's median partition as well.
        let m = nelements / 2;
        elements.select_nth_unstable_by(m, |e1, e2| {
            if (e1.lpos[axis] - e2.lpos[axis]).abs() > EPS {
                if e1.lpos[axis] < e2.lpos[axis] { std::cmp::Ordering::Less }
                else { std::cmp::Ordering::Greater }
            } else { e1.e.cmp(&e2.e) }
        });
        if m > 0 { self.child[2*index] = self.make_bvh(&mut elements[..m], lev+1); }
        if m != nelements { self.child[2*index+1] = self.make_bvh(&mut elements[m..], lev+1); }
        if self.child[2*index] == -1 && self.child[2*index+1] == -1 {
            panic!("this should have been a leaf, nelements={nelements}");
        }
        if lev > MAXTREEDEPTH { eprintln!("max tree depth exceeded"); }
        index as i32
    }
}

// mjCBody::ComputeBVH.
fn compute_bvh(tree: &mut BoundingVolumeHierarchy, b: &BodySpec, aabb: &[[f64; 6]], geomadr: usize) {
    tree.allocate_bounding_volumes(b.geoms.len());
    if b.geoms.is_empty() { return; }
    tree.set(&b.ipos, &b.iquat);
    for (i, g) in b.geoms.iter().enumerate() {
        tree.add_bounding_volume((geomadr+i) as i32, g, &aabb[i]);
    }
    tree.create_bvh();
}

// std::max/min in the compiler retain the first operand on equality or NaN.
fn max(a: f64, b: f64) -> f64 { if a < b { b } else { a } }
fn min(a: f64, b: f64) -> f64 { if b < a { b } else { a } }

// mjuu_normvec.
fn normvec(vec: &mut [f64], n: usize) -> f64 {
    let mut nrm = 0.0;
    for i in 0..n { nrm += vec[i] * vec[i]; }
    if nrm < EPS { return 0.0; }
    nrm = nrm.sqrt();
    if (nrm - 1.0).abs() > EPS {
        for i in 0..n { vec[i] /= nrm; }
    }
    nrm
}

// mjuu_quat2mat.
fn quat2mat(res: &mut [f64; 9], quat: &[f64; 4]) {
    if *quat == [1.0, 0.0, 0.0, 0.0] {
        *res = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        return;
    }
    let q00 = quat[0] * quat[0];
    let q01 = quat[0] * quat[1];
    let q02 = quat[0] * quat[2];
    let q03 = quat[0] * quat[3];
    let q11 = quat[1] * quat[1];
    let q12 = quat[1] * quat[2];
    let q13 = quat[1] * quat[3];
    let q22 = quat[2] * quat[2];
    let q23 = quat[2] * quat[3];
    let q33 = quat[3] * quat[3];
    res[0] = q00 + q11 - q22 - q33;
    res[4] = q00 - q11 + q22 - q33;
    res[8] = q00 - q11 - q22 + q33;
    res[1] = 2.0 * (q12 - q03);
    res[2] = 2.0 * (q13 + q02);
    res[3] = 2.0 * (q12 + q03);
    res[5] = 2.0 * (q23 - q01);
    res[6] = 2.0 * (q13 - q02);
    res[7] = 2.0 * (q23 + q01);
}

// mjuu_mulquat.
fn mulquat(res: &mut [f64; 4], qa: &[f64; 4], qb: &[f64; 4]) {
    let mut tmp = [
        qa[0] * qb[0] - qa[1] * qb[1] - qa[2] * qb[2] - qa[3] * qb[3],
        qa[0] * qb[1] + qa[1] * qb[0] + qa[2] * qb[3] - qa[3] * qb[2],
        qa[0] * qb[2] - qa[1] * qb[3] + qa[2] * qb[0] + qa[3] * qb[1],
        qa[0] * qb[3] + qa[1] * qb[2] - qa[2] * qb[1] + qa[3] * qb[0],
    ];
    normvec(&mut tmp, 4);
    *res = tmp;
}

// mjuu_mulRMRT.
fn mul_rmrt(res: &mut [f64; 9], r: &[f64; 9], m: &[f64; 9]) {
    let mut tmp = [0.0; 9];
    for i in 0..3 {
        for j in 0..3 {
            tmp[3*i+j] = r[3*i]*m[j] + r[3*i+1]*m[3+j] + r[3*i+2]*m[6+j];
        }
    }
    for i in 0..3 {
        for j in 0..3 {
            res[3*i+j] = tmp[3*i]*r[3*j] + tmp[3*i+1]*r[3*j+1] + tmp[3*i+2]*r[3*j+2];
        }
    }
}

// mjuu_mulmat.
fn mulmat(res: &mut [f64; 9], a: &[f64; 9], b: &[f64; 9]) {
    let mut tmp = [0.0; 9];
    for i in 0..3 {
        for j in 0..3 {
            tmp[3*i+j] = a[3*i]*b[j] + a[3*i+1]*b[3+j] + a[3*i+2]*b[6+j];
        }
    }
    *res = tmp;
}

// mjuu_transposemat.
fn transposemat(res: &mut [f64; 9], mat: &[f64; 9]) {
    *res = [mat[0], mat[3], mat[6], mat[1], mat[4], mat[7], mat[2], mat[5], mat[8]];
}

// mjuu_crossvec.
fn crossvec(a: &mut [f64; 3], b: &[f64; 3], c: &[f64; 3]) {
    a[0] = b[1]*c[2] - b[2]*c[1];
    a[1] = b[2]*c[0] - b[0]*c[2];
    a[2] = b[0]*c[1] - b[1]*c[0];
}

// mjuu_z2quat.
fn z2quat(quat: &mut [f64; 4], vec: &[f64; 3]) {
    let mut axis = [0.0; 3];
    crossvec(&mut axis, &[0.0, 0.0, 1.0], vec);
    quat[1..].copy_from_slice(&axis);
    let s = normvec(&mut quat[1..], 3);
    if s < 1e-10 { quat[1] = 1.0; quat[2] = 0.0; quat[3] = 0.0; }
    let ang = s.atan2(vec[2]);
    quat[0] = (ang / 2.0).cos();
    quat[1] *= (ang / 2.0).sin();
    quat[2] *= (ang / 2.0).sin();
    quat[3] *= (ang / 2.0).sin();
}

// mjuu_eig3. D is d_mat to distinguish the matrix from other scalar locals.
fn eig3(eigval: &mut [f64; 3], eigvec: &mut [f64; 9], quat: &mut [f64; 4], mat: &[f64; 9], reltol: f64) -> usize {
    let mut d_mat = [0.0; 9];
    let mut tmp = [0.0; 9];
    let mut tmp2 = [0.0; 9];
    let mut scale = 0.0;
    for i in 0..9 { scale = max(scale, mat[i].abs()); }
    let tol = scale * max(reltol, 4e-15);
    *quat = [1.0, 0.0, 0.0, 0.0];
    let mut iter = 0;
    while iter < 500 {
        quat2mat(eigvec, quat);
        transposemat(&mut tmp2, eigvec);
        mulmat(&mut tmp, &tmp2, mat);
        mulmat(&mut d_mat, &tmp, eigvec);
        *eigval = [d_mat[0], d_mat[4], d_mat[8]];
        let (rk, ck, rotk) = if d_mat[1].abs() > d_mat[2].abs() && d_mat[1].abs() > d_mat[5].abs() {
            (0, 1, 2)
        } else if d_mat[2].abs() > d_mat[5].abs() { (0, 2, 1) } else { (1, 2, 0) };
        if d_mat[3*rk+ck].abs() <= tol { break; }
        let tau = (d_mat[4*ck] - d_mat[4*rk]) / (2.0 * d_mat[3*rk+ck]);
        let t = if tau >= 0.0 { 1.0 / (tau + (1.0 + tau*tau).sqrt()) }
            else { -1.0 / (-tau + (1.0 + tau*tau).sqrt()) };
        let h = t / (1.0 + (1.0 + t*t).sqrt());
        tmp[0] = 1.0 / (1.0 + h*h).sqrt();
        tmp[1] = 0.0; tmp[2] = 0.0; tmp[3] = 0.0;
        tmp[rotk+1] = (if rotk == 1 { h } else { -h }) * tmp[0];
        let old = *quat;
        mulquat(quat, &old, tmp[..4].try_into().unwrap());
        normvec(quat, 4);
        iter += 1;
    }
    let eps = scale * max(reltol, 1e-12);
    for j in 0..3 {
        let j1 = j % 2;
        if eigval[j1] + eps < eigval[j1+1] {
            eigval.swap(j1, j1+1);
            tmp[0] = 0.707106781186548;
            tmp[1] = 0.0; tmp[2] = 0.0; tmp[3] = 0.0;
            tmp[(j1+2)%3+1] = tmp[0];
            let old = *quat;
            mulquat(quat, &old, tmp[..4].try_into().unwrap());
            normvec(quat, 4);
        }
    }
    quat2mat(eigvec, quat);
    iter
}

// mjuu_fullInertia.
fn full_inertia(quat: &mut [f64; 4], inertia: &mut [f64; 3], fullinertia: &[f64; 6]) -> Result<(), String> {
    if fullinertia[0].is_nan() { return Ok(()); }
    let mut eigval = [0.0; 3];
    let mut eigvec = [0.0; 9];
    let mut quattmp = [0.0; 4];
    let full = [fullinertia[0], fullinertia[3], fullinertia[4], fullinertia[3], fullinertia[1], fullinertia[5], fullinertia[4], fullinertia[5], fullinertia[2]];
    eig3(&mut eigval, &mut eigvec, &mut quattmp, &full, 4e-15);
    *quat = quattmp;
    *inertia = eigval;
    if eigval[2] < EPS { return Err("inertia must have positive eigenvalues".into()); }
    Ok(())
}

// checksize.
fn checksize(size: &[f64; 3], r#type: i32) -> Result<(), String> {
    if r#type == GEOM_PLANE {
        if size[2] <= 0.0 { return Err("plane size(3) must be positive".into()); }
    } else {
        let n = match r#type { GEOM_MESH => 0, GEOM_SPHERE => 1, GEOM_CAPSULE | GEOM_CYLINDER => 2, _ => 3 };
        for i in 0..n {
            if size[i] <= 0.0 { return Err(format!("size {i} must be positive in geom")); }
        }
    }
    Ok(())
}

// islimited; checklimited has no work with this subset's autolimits=true.
fn islimited(limited: i32, range: &[f64; 2]) -> bool {
    limited == 1 || (limited == 2 && range[0] < range[1])
}

// mjCBody::Compile, explicit inertial portion. Geoms/joints/sites are compiled by
// compile_body in the original interleaved order.
fn compile_inertial(b: &mut BodySpec, id: usize, c: &CompilerSpec) -> Result<(), String> {
    normvec(&mut b.quat, 4);
    normvec(&mut b.iquat, 4);
    if !b.fullinertia[0].is_nan() {
        if b.inertia[0] != 0.0 || b.inertia[1] != 0.0 || b.inertia[2] != 0.0 {
            return Err("fullinertia and diagonal inertia cannot both be specified".into());
        }
        let mut mat = [0.0; 9];
        let mut full_body = [0.0; 9];
        quat2mat(&mut mat, &b.iquat);
        let f = b.fullinertia;
        let full = [f[0], f[3], f[4], f[3], f[1], f[5], f[4], f[5], f[2]];
        mul_rmrt(&mut full_body, &mat, &full);
        let f = [full_body[0], full_body[4], full_body[8], full_body[1], full_body[2], full_body[5]];
        if let Err(err) = full_inertia(&mut b.iquat, &mut b.inertia, &f) {
            if id == 0 || c.boundinertia <= 0.0 || b.inertia[2] < -EPS * max(1.0, b.inertia[0]) { return Err(err); }
        }
    }
    Ok(())
}

// mjCJoint::Compile.
fn compile_joint(j: &mut JointSpec) -> Result<usize, String> {
    if j.springdamper[0] != 0.0 || j.springdamper[1] != 0.0 {
        if j.springdamper[0] <= 0.0 || j.springdamper[1] <= 0.0 { return Err("when defined, springdamper values must be positive in joint".into()); }
    }
    if j.springyield < 0.0 { return Err("springyield cannot be negative in joint".into()); }
    if j.r#type == JNT_FREE { j.limited = 0; }
    if islimited(j.limited, &j.range) {
        if j.range[0] >= j.range[1] && j.r#type != JNT_BALL { return Err("range[0] should be smaller than range[1] in joint".into()); }
        if j.range[0] != 0.0 && j.r#type == JNT_BALL { return Err("range[0] should be 0 in ball joint".into()); }
    }
    if j.r#type == JNT_FREE || j.r#type == JNT_BALL { j.actfrclimited = 0; }
    if islimited(j.actfrclimited, &j.actfrcrange) && j.actfrcrange[0] >= j.actfrcrange[1] {
        return Err("actfrcrange[0] should be smaller than actfrcrange[1] in joint".into());
    }
    if j.r#type == JNT_FREE || j.r#type == JNT_BALL { j.axis = [0.0, 0.0, 1.0]; }
    if normvec(&mut j.axis, 3) < EPS { return Err("axis too small in joint".into()); }
    if j.r#type == JNT_FREE && j.springyield > 0.0 { return Err("springyield should not be defined in free joint".into()); }
    if j.r#type == JNT_FREE { j.pos = [0.0; 3]; }
    match j.r#type { JNT_FREE => Ok(6), JNT_BALL => Ok(3), JNT_SLIDE | JNT_HINGE => Ok(1), _ => Err("unknown joint type".into()) }
}

// mjuu_dot3.
fn dot3(a: &[f64], b: &[f64]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

// mjuu_dist3.
fn dist3(a: &[f64], b: &[f64]) -> f64 {
    ((a[0] - b[0]) * (a[0] - b[0]) + (a[1] - b[1]) * (a[1] - b[1]) + (a[2] - b[2]) * (a[2] - b[2])).sqrt()
}

// mjuu_mulvecmat (mjuu_mulvecmatT is not needed: its only use is the refquat branch of
// ApplyTransformations); the caller copies a vector that aliases the result.
fn mulvecmat(res: &mut [f64; 3], vec: &[f64; 3], mat: &[f64; 9]) {
    let tmp = [
        mat[0] * vec[0] + mat[1] * vec[1] + mat[2] * vec[2],
        mat[3] * vec[0] + mat[4] * vec[1] + mat[5] * vec[2],
        mat[6] * vec[0] + mat[7] * vec[1] + mat[8] * vec[2],
    ];
    *res = tmp;
}

// mjuu_frameaccum.
fn frameaccum(pos: &mut [f64; 3], quat: &mut [f64; 4], childpos: &[f64; 3], childquat: &[f64; 4]) {
    let mut mat = [0.0; 9];
    let mut vec = [0.0; 3];
    let mut qtmp = [0.0; 4];
    quat2mat(&mut mat, quat);
    mulvecmat(&mut vec, childpos, &mat);
    pos[0] += vec[0];
    pos[1] += vec[1];
    pos[2] += vec[2];
    mulquat(&mut qtmp, quat, childquat);
    *quat = qtmp;
}

// The compiled mesh asset, mjCMesh: the members the vertex-only path reads or fills.
// Texture coordinates, nodes, tetrahedra, user normals and the octree are never present.
struct Mesh {
    name: Option<String>,
    scale: [f64; 3],
    inertia: i32,
    vert: Vec<f32>,
    normal: Vec<f32>,
    face: Vec<i32>,
    facenormal: Vec<i32>,
    graph: Vec<i32>,
    center: Vec<f64>,
    polygons: Vec<Vec<i32>>,
    polygon_normals: Vec<f64>,
    polygon_map: Vec<Vec<i32>>,
    face_aabb: Vec<f64>,
    tree: BoundingVolumeHierarchy,
    pos: [f64; 3],
    quat: [f64; 4],
    aamm: [f64; 6],
    volume: f64,
    surface: f64,
}

// static triangle: area, surface normal and center.
fn triangle(normal: Option<&mut [f64; 3]>, center: Option<&mut [f64; 3]>, v1: &[f64], v2: &[f64], v3: &[f64]) -> f64 {
    let mut normal_local = [0.0; 3]; // if normal is nullptr
    let has_normal = normal.is_some();
    let normal_ptr = match normal { Some(n) => n, None => &mut normal_local };
    if let Some(center) = center {
        center[0] = (v1[0] + v2[0] + v3[0]) / 3.0;
        center[1] = (v1[1] + v2[1] + v3[1]) / 3.0;
        center[2] = (v1[2] + v2[2] + v3[2]) / 3.0;
    }
    let b = [v2[0] - v1[0], v2[1] - v1[1], v2[2] - v1[2]];
    let c = [v3[0] - v1[0], v3[1] - v1[1], v3[2] - v1[2]];
    crossvec(normal_ptr, &b, &c);
    let len = dot3(normal_ptr, normal_ptr).sqrt();
    if len < MINVAL { return 0.0; }
    if has_normal {
        normal_ptr[0] /= len;
        normal_ptr[1] /= len;
        normal_ptr[2] /= len;
    }
    0.5 * len
}

// mjCMesh::MakeGraph: the convex hull graph of the vertices. Where the C calls the hull library
// (its init, hull, triangulation and vertex-neighbour calls and its vertex and facet loops) this
// reads the Hull of crate::hull: its vertices ascending, its triangles outward, and each vertex's
// triangles. maxhullvert is outside the subset, so the library options it adds have no counterpart.
// Hull triangles are arrays of three, so the C's "did not return triangle" checks cannot fail;
// its orientation swap (the library's facets may be inside out) has no counterpart either.
fn make_graph(name: &str, dvert: &[f64], nvert: usize) -> Result<Vec<i32>, String> {
    // graph not needed for small meshes
    if nvert < 4 { return Ok(Vec::new()); }

    // check for colocated/collinear/coplanar vertices
    {
        // find second vertex that is distinct from vertex 0
        let mut v1: i32 = -1;
        let mut len1 = 0.0;
        for i in 1..nvert {
            len1 = dist3(&dvert[3*i..], dvert);
            if len1 > MINVAL { v1 = i as i32; break; }
        }

        // no second vertex found: all vertices are colocated
        if v1 < 0 {
            return Err(format!("mesh '{name}' has colocated vertices, cannot compute convex hull. Consider using a small sphere instead"));
        }
        let v1 = v1 as usize;

        // find first non-collinear triple to define a plane
        let edge1 = [dvert[3*v1] - dvert[0], dvert[3*v1+1] - dvert[1], dvert[3*v1+2] - dvert[2]];
        let mut normal = [0.0; 3];
        let mut collinear = true;
        for i in 1..nvert {
            if i == v1 { continue; }
            let edge2 = [dvert[3*i] - dvert[0], dvert[3*i+1] - dvert[1], dvert[3*i+2] - dvert[2]];
            let len2 = dot3(&edge2, &edge2).sqrt();
            if len2 < MINVAL { continue; }
            crossvec(&mut normal, &edge1, &edge2);
            let norm = dot3(&normal, &normal).sqrt();
            if norm > MINVAL * len1 * len2 {
                normal[0] /= norm;
                normal[1] /= norm;
                normal[2] /= norm;
                collinear = false;
                break;
            }
        }

        // vertices are collinear: cannot compute convex hull
        if collinear {
            return Err(format!("mesh '{name}' has collinear vertices, cannot compute convex hull. Consider using a thin capsule instead"));
        }

        // find first vertex that is not on the plane
        let d = dot3(&normal, dvert);
        let mut coplanar = true;
        for i in 0..nvert {
            if (dot3(&normal, &dvert[3*i..]) - d).abs() > MINVAL * len1 {
                coplanar = false;
                break;
            }
        }

        // vertices are coplanar: cannot compute convex hull
        if coplanar {
            return Err(format!("mesh '{name}' has coplanar vertices, cannot compute convex hull. Consider using a primitive geom type (plane or thin box) instead"));
        }
    }

    // construct convex hull; an error of the hull is the C's longjmp error handler
    let hull = crate::hull::convex_hull(&dvert[..3*nvert], nvert).map_err(|e| format!("mesh '{name}': convex hull error: {e}"))?;

    // allocate graph:
    //  numvert, numface, vert_edgeadr[numvert], vert_globalid[numvert],
    //  edge_localid[numvert+3*numface], face_globalid[3*numface]
    let numvert = hull.vertices.len();
    let numface = hull.faces.len();
    let szgraph = 2 + 3 * numvert + 6 * numface;
    let mut graph = vec![0_i32; szgraph];
    graph[0] = numvert as i32;
    graph[1] = numface as i32;

    // offsets of the C's pointers for convenience
    let vert_edgeadr = 2;
    let vert_globalid = 2 + numvert;
    let edge_localid = 2 + 2 * numvert;
    let face_globalid = 2 + 3 * numvert + 3 * numface;

    // fill in graph data
    let mut adr = 0;
    let mut ok = true;
    for (i, &pid) in hull.vertices.iter().enumerate() {
        // point id of this vertex, check
        if pid < 0 || pid as usize >= nvert {
            ok = false;
            break;
        }

        // save edge address and global id of this vertex
        graph[vert_edgeadr + i] = adr as i32;
        graph[vert_globalid + i] = pid;

        // process neighboring faces and their vertices
        let start = adr;
        for &facet in &hull.neighbors[i] {
            for &pid1 in &hull.faces[facet] {
                // point id of face vertex, check
                if pid1 < 0 || pid1 as usize >= nvert {
                    ok = false;
                    break;
                }

                // if different from vertex id, try to insert
                if pid != pid1 {
                    // check for previous record
                    let mut j = start;
                    while j < adr {
                        if pid1 == graph[edge_localid + j] { break; }
                        j += 1;
                    }

                    // not found: insert
                    if j >= adr {
                        graph[edge_localid + adr] = pid1;
                        adr += 1;
                    }
                }
            }
        }

        // insert separator, advance to next vertex
        graph[edge_localid + adr] = -1;
        adr += 1;
    }

    // size check: SHOULD NOT OCCUR (mju_error, which the compiler turns into its error)
    if adr != numvert + 3 * numface { return Err("engine error: Wrong size in convex hull graph".into()); }

    // add triangle data
    let mut adr = 0;
    for face in &hull.faces {
        for ii in 0..3 { graph[face_globalid + adr + ii] = face[ii]; }

        // advance to next triangle
        adr += 3;
    }

    // bad graph: delete
    if !ok {
        eprintln!("Could not construct convex hull graph");
        return Ok(Vec::new());
    }

    // replace global ids with local ids in edge data
    // invert vert_globalid: point id -> hull vertex index, -1 if not on hull.
    let mut hullid = vec![-1_i32; nvert];
    for adr in 0..numvert { hullid[graph[vert_globalid + adr] as usize] = adr as i32; }
    for i in 0..numvert + 3 * numface {
        if graph[edge_localid + i] >= 0 {
            let adr = hullid[graph[edge_localid + i] as usize];

            // make sure we found a match: SHOULD NOT OCCUR (mju_error, as above)
            if adr < 0 { return Err("engine error: Vertex id not found in convex hull".into()); }
            graph[edge_localid + i] = adr;
        }
    }
    Ok(graph)
}

// MeshPolygonKey.
fn mesh_polygon_key(v1: &[f64], v2: &[f64], v3: &[f64], angle_tol: f64) -> Option<(f64, f64)> {
    let diff12 = [v2[0] - v1[0], v2[1] - v1[1], v2[2] - v1[2]];
    let diff13 = [v3[0] - v1[0], v3[1] - v1[1], v3[2] - v1[2]];
    let mut normal = [0.0; 3];

    crossvec(&mut normal, &diff12, &diff13);
    let norm = dot3(&normal, &normal).sqrt();
    if norm < MINVAL { return None; }

    // atan2 is sensitive to sign of 0.0, adding 0.0 to enforcing only positive 0.0
    normal[0] = (normal[0] / norm) + 0.0;
    normal[1] = (normal[1] / norm) + 0.0;
    normal[2] = (normal[2] / norm) + 0.0;
    let mut rtheta = 0.0;
    let mut rphi = 0.0;

    // clamp normal to be in valid range for acos
    if normal[2].abs() > 1.0 - 1e-7 {
        if normal[2] < 0.0 { rphi = (PI / angle_tol).round(); }
        return Some((rtheta, rphi));
    }
    // rounded azimuthal and polar angles
    rtheta = (normal[1].atan2(normal[0]) / angle_tol).round();
    rphi = (normal[2].acos() / angle_tol).round();
    Some((rtheta, rphi))
}

// MeshPolygon: the helper class that merges the coplanar triangles of one orientation key.
struct MeshPolygon {
    edges: Vec<(i32, i32)>,
    // inserted faces do not necessarily share edges with the current polygon, so they're grouped as
    // islands until they can be combined with later face insertions
    islands: Vec<i32>,
    nisland: i32,
    normal: [f64; 3],
}

impl MeshPolygon {
    // The vertex positions of the C constructor are unused there and are not passed.
    fn new(v1i: i32, v2i: i32, v3i: i32, theta: f64, phi: f64) -> Self {
        Self {
            edges: vec![(v1i, v2i), (v2i, v3i), (v3i, v1i)],
            islands: vec![0, 0, 0],
            nisland: 1,
            normal: [theta.cos() * phi.sin(), theta.sin() * phi.sin(), phi.cos()],
        }
    }

    // MeshPolygon::CombineIslands: combine two islands when a newly inserted face connects them.
    fn combine_islands(&mut self, island1: &mut i32, island2: &mut i32) {
        // pick the smaller island
        if *island2 < *island1 { std::mem::swap(island1, island2); }

        // renumber the islands
        for k in 0..self.islands.len() {
            if self.islands[k] == *island2 { self.islands[k] = *island1; }
            else if self.islands[k] > *island2 { self.islands[k] -= 1; }
        }
    }

    // MeshPolygon::InsertFace: insert a triangular face into the polygon.
    fn insert_face(&mut self, v1: i32, v2: i32, v3: i32) {
        let mut add1 = true;
        let mut add2 = true;
        let mut add3 = true;
        let mut island = -1;

        // check if face can be attached via edge v1v2
        for i in 0..self.edges.len() {
            if self.edges[i].0 == v2 && self.edges[i].1 == v1 {
                add1 = false;
                island = self.islands[i];
                self.edges.remove(i);
                self.islands.remove(i);
                break;
            }
        }

        // check if face can be attached via edge v2v3
        for i in 0..self.edges.len() {
            if self.edges[i].0 == v3 && self.edges[i].1 == v2 {
                let mut island2 = self.islands[i];
                if island == -1 {
                    island = island2;
                } else if island2 != island {
                    self.nisland -= 1;
                    self.combine_islands(&mut island, &mut island2);
                }
                add2 = false;
                self.edges.remove(i);
                self.islands.remove(i);
                break;
            }
        }

        // check if face can be attached via edge v3v1
        for i in 0..self.edges.len() {
            if self.edges[i].0 == v1 && self.edges[i].1 == v3 {
                let mut island3 = self.islands[i];
                if island == -1 {
                    island = island3;
                } else if island3 != island {
                    self.nisland -= 1;
                    self.combine_islands(&mut island, &mut island3);
                }
                add3 = false;
                self.edges.remove(i);
                self.islands.remove(i);
                break;
            }
        }

        if island == -1 {
            island = self.nisland;
            self.nisland += 1;
        }

        // add only new edges to the polygon
        if add1 {
            self.edges.push((v1, v2));
            self.islands.push(island);
        }
        if add2 {
            self.edges.push((v2, v3));
            self.islands.push(island);
        }
        if add3 {
            self.edges.push((v3, v1));
            self.islands.push(island);
        }
    }

    // MeshPolygon::Paths: the transverse vertices of the polygon, multiple paths possible if not connected.
    fn paths(&self) -> Vec<Vec<i32>> {
        let mut paths = Vec::new();

        // go through each connected component of the polygon
        for i in 0..self.nisland {
            let mut path: Vec<i32> = Vec::new();

            // find starting vertex
            for j in 0..self.edges.len() {
                if self.islands[j] == i {
                    path.push(self.edges[j].0);
                    path.push(self.edges[j].1);
                    break;
                }
            }

            // SHOULD NOT OCCUR (See logic in MeshPolygon::CombineIslands)
            if path.is_empty() { continue; }

            // visit the next vertex given the current edge
            let mut next = *path.last().unwrap();
            for _l in 0..self.edges.len() {
                let mut finished = false;
                for k in 1..self.edges.len() {
                    if self.islands[k] == i && self.edges[k].0 == next {
                        next = self.edges[k].1;
                        if next == path[0] {
                            paths.push(path.clone());
                            finished = true;
                            break;
                        }
                        path.push(next);
                        break;
                    }
                }

                // back at start
                if finished { break; }
            }
        }
        paths
    }
}

impl Mesh {
    fn nvert(&self) -> usize { self.vert.len() / 3 }
    fn nnormal(&self) -> usize { self.normal.len() / 3 }
    fn nface(&self) -> usize { self.face.len() / 3 }
    fn npolygon(&self) -> usize { self.polygons.len() }
    fn npolygonvert(&self) -> usize { self.polygons.iter().map(|p| p.len()).sum() }
    fn npolygonmap(&self) -> usize { self.polygon_map.iter().map(|p| p.len()).sum() }

    // GraphFaces: the face_globalid block of the graph.
    fn graph_faces(&self) -> usize { 2 + 3 * (self.graph[0] as usize + self.graph[1] as usize) }

    // mjCMesh::CopyFromSpec and ProcessVertices(remove_repeated = false). The vertices
    // are finite-checked and widened as given. The instance map of a use without a gauge
    // is the identity, so InstanceDiagonal() leaves scale[i] *= 1 unchanged.
    fn from_spec(spec: &MeshSpec) -> Result<Self, String> {
        // face_ is empty for a vertex-only mesh, so its multiple-of-3 check passes
        if spec.vert.len() % 3 != 0 { return Err("vertex data must be a multiple of 3".into()); }
        for i in 0..spec.vert.len() / 3 {
            let v = &spec.vert[3*i..3*i+3];
            if !v[0].is_finite() || !v[1].is_finite() || !v[2].is_finite() {
                return Err(format!("vertex coordinate {i} is not finite"));
            }
        }
        Ok(Self {
            name: spec.name.clone(), scale: spec.scale, inertia: spec.inertia, vert: spec.vert.clone(),
            normal: Vec::new(), face: Vec::new(), facenormal: Vec::new(), graph: Vec::new(),
            center: Vec::new(), polygons: Vec::new(), polygon_normals: Vec::new(), polygon_map: Vec::new(),
            face_aabb: Vec::new(), tree: BoundingVolumeHierarchy::default(),
            pos: [0.0; 3], quat: [1.0, 0.0, 0.0, 0.0],
            aamm: [1e10, 1e10, 1e10, -1e10, -1e10, -1e10],
            volume: 0.0, surface: 0.0,
        })
    }

    fn name(&self) -> &str { self.name.as_deref().unwrap_or("") }

    // mjCMesh::TryCompile for a mesh without a file: CopyFromSpec, CheckInitialMesh, Process.
    // Without a file, a plugin or a cache there is no loading, so Process is the whole compile.
    fn compile(spec: &MeshSpec) -> Result<Self, String> {
        let mut mesh = Self::from_spec(spec)?;
        mesh.check_initial_mesh()?;
        mesh.process()?;
        Ok(mesh)
    }

    // mjCMesh::CheckInitialMesh; the normal, texcoord, face and texcoord-size checks
    // are vacuous without those arrays, and the vertex-index check has no faces to read.
    fn check_initial_mesh(&self) -> Result<(), String> {
        if self.vert.len() < 12 { return Err("at least 4 vertices required".into()); }
        Ok(())
    }

    // mjCMesh::ComputeVolume.
    fn compute_volume(&self, com: &mut [f64; 3], facecen: &[f64; 3], dvert: &[f64]) -> f64 {
        let mut normal = [0.0; 3];
        let mut center = [0.0; 3];
        let mut total_volume = 0.0;
        *com = [0.0; 3];
        let nf = if self.inertia == MESH_INERTIA_CONVEX { self.graph[1] as usize } else { self.nface() };
        let f = if self.inertia == MESH_INERTIA_CONVEX { &self.graph[self.graph_faces()..] } else { &self.face[..] };

        for i in 0..nf {
            let area = triangle(Some(&mut normal), Some(&mut center),
                &dvert[3 * f[3*i] as usize..], &dvert[3 * f[3*i+1] as usize..], &dvert[3 * f[3*i+2] as usize..]);

            // compute and add volume
            let vec = [center[0] - facecen[0], center[1] - facecen[1], center[2] - facecen[2]];
            let mut volume = dot3(&vec, &normal) * area / 3.0;

            // if legacy computation requested, then always positive
            if self.inertia == MESH_INERTIA_LEGACY { volume = volume.abs(); }

            // add pyramid com
            total_volume += volume;
            com[0] += volume * (center[0] * 3.0 / 4.0 + facecen[0] / 4.0);
            com[1] += volume * (center[1] * 3.0 / 4.0 + facecen[1] / 4.0);
            com[2] += volume * (center[2] * 3.0 / 4.0 + facecen[2] / 4.0);
        }

        // if volume is valid normalize CoM
        if total_volume >= MINVAL {
            com[0] /= total_volume;
            com[1] /= total_volume;
            com[2] /= total_volume;
        }
        total_volume
    }

    // mjCMesh::ComputeSurfaceArea.
    fn compute_surface_area(&self, com: &mut [f64; 3], facecen: &[f64; 3], dvert: &[f64]) -> f64 {
        let mut surface = 0.0;
        *com = [0.0; 3];
        for i in 0..self.nface() {
            let mut center = [0.0; 3];
            let area = triangle(None, Some(&mut center),
                &dvert[3 * self.face[3*i] as usize..], &dvert[3 * self.face[3*i+1] as usize..], &dvert[3 * self.face[3*i+2] as usize..]);

            // add pyramid com
            surface += area;
            com[0] += area * (center[0] * 3.0 / 4.0 + facecen[0] / 4.0);
            com[1] += area * (center[1] * 3.0 / 4.0 + facecen[1] / 4.0);
            com[2] += area * (center[2] * 3.0 / 4.0 + facecen[2] / 4.0);
        }

        // if area is valid normalize CoM
        if surface >= MINVAL {
            com[0] /= surface;
            com[1] /= surface;
            com[2] /= surface;
        }
        surface
    }

    // mjCMesh::ApplyTransformations. refpos and refquat keep their defaults, which leave the first
    // two blocks inactive; the instance map is the identity, so the non-diagonal block is inactive too.
    fn apply_transformations(&mut self, dvert: &mut [f64]) {
        // scale
        if self.scale[0] != 1.0 || self.scale[1] != 1.0 || self.scale[2] != 1.0 {
            for i in 0..self.nvert() {
                dvert[3*i] *= self.scale[0];
                dvert[3*i+1] *= self.scale[1];
                dvert[3*i+2] *= self.scale[2];
            }

            // normals are covectors: transform by the inverse scale
            for i in 0..self.nnormal() {
                self.normal[3*i] = (self.normal[3*i] as f64 / self.scale[0]) as f32;
                self.normal[3*i+1] = (self.normal[3*i+1] as f64 / self.scale[1]) as f32;
                self.normal[3*i+2] = (self.normal[3*i+2] as f64 / self.scale[2]) as f32;
            }
        }

        // normalize normals
        for i in 0..self.nnormal() {
            // compute length
            let len: f32 = self.normal[3*i] * self.normal[3*i] +
                self.normal[3*i+1] * self.normal[3*i+1] +
                self.normal[3*i+2] * self.normal[3*i+2];

            // rescale
            if len as f64 > MINVAL {
                let scl: f32 = 1.0 / len.sqrt();
                self.normal[3*i] *= scl;
                self.normal[3*i+1] *= scl;
                self.normal[3*i+2] *= scl;
            } else {
                self.normal[3*i] = 0.0;
                self.normal[3*i+1] = 0.0;
                self.normal[3*i+2] = 1.0;
            }
        }
    }

    // mjCMesh::ComputeFaceCentroid: find centroid of faces, return total area.
    fn compute_face_centroid(&self, facecen: &mut [f64; 3], dvert: &[f64]) -> f64 {
        let mut total_area = 0.0;
        for i in 0..self.nface() {
            let mut center = [0.0; 3];
            let area = triangle(None, Some(&mut center),
                &dvert[3 * self.face[3*i] as usize..], &dvert[3 * self.face[3*i+1] as usize..], &dvert[3 * self.face[3*i+2] as usize..]);

            // accumulate
            facecen[0] += area * center[0];
            facecen[1] += area * center[1];
            facecen[2] += area * center[2];
            total_area += area;
        }

        // finalize centroid of faces
        if total_area >= MINVAL {
            facecen[0] /= total_area;
            facecen[1] /= total_area;
            facecen[2] /= total_area;
        }
        total_area
    }

    // mjCMesh::Process.
    fn process(&mut self) -> Result<(), String> {
        let mut dvert: Vec<f64> = self.vert.iter().map(|&v| v as f64).collect();
        // The half-edge structure and its orientation check read the faces of the mesh: a
        // vertex-only mesh has none yet, so both do nothing.

        // make graph describing convex hull: a vertex-only mesh has no faces, so the hull is needed
        self.graph = make_graph(self.name(), &dvert, self.nvert())?;

        // no faces: copy from convex hull
        self.copy_graph();

        // no normals: make
        self.make_normal(&dvert);

        // facenormal might not exist if usernormal was specified: MakeNormal has made it, so the
        // size check against the faces and the copy from the faces both do nothing.

        if !self.graph.is_empty() {
            self.make_polygons(&dvert);
        } else {
            self.polygon_map.resize(self.nvert(), Vec::new());
        }

        // correct winding order for left-handed coordinate systems
        let righthand = self.scale[0] * self.scale[1] * self.scale[2] > 0.0;
        if !righthand {
            for i in (0..self.face.len()).step_by(3) { self.face.swap(i + 1, i + 2); }
            for i in (0..self.facenormal.len()).step_by(3) { self.facenormal.swap(i + 1, i + 2); }
            if !self.graph.is_empty() {
                let faces = self.graph_faces();
                for i in 0..self.graph[1] as usize { self.graph.swap(faces + 3*i + 1, faces + 3*i + 2); }
                for polygon in &mut self.polygons { polygon[1..].reverse(); }
            }
        }

        // user offset, rotation, scaling
        self.apply_transformations(&mut dvert);

        // find centroid of faces
        let mut facecen = [0.0; 3];
        if self.compute_face_centroid(&mut facecen, &dvert) < MINVAL {
            return Err(format!("mesh surface area is too small: {}", self.name()));
        }

        // compute inertia and transform mesh. The mesh is transformed such that it is
        // centered at the CoM and the axes are the principle axes of inertia
        let mut com = [0.0; 3];
        let mut inert = [0.0; 6];

        // compute CoM and volume/area
        if self.inertia == MESH_INERTIA_SHELL {
            self.surface = self.compute_surface_area(&mut com, &facecen, &dvert);
            if self.surface < MINVAL {
                return Err(format!("mesh surface area is too small: {}", self.name()));
            }
        } else {
            self.volume = self.compute_volume(&mut com, &facecen, &dvert);
            if self.volume < MINVAL {
                if self.volume < 0.0 {
                    return Err(format!("mesh volume is negative (misoriented triangles): {}", self.name()));
                } else {
                    return Err(format!("mesh volume is too small: {} . Try setting inertia to shell", self.name()));
                }
            }
        }

        // compute inertia
        let total_volume = self.compute_inertia(&mut inert, &com, &dvert);
        if self.inertia == MESH_INERTIA_SHELL {
            self.surface = total_volume;
        } else {
            self.volume = total_volume;
        }

        // get quaternion and diagonal inertia (vertices are float32, so stop at 1e-7 relative)
        let mut eigval = [0.0; 3];
        let mut eigvec = [0.0; 9];
        let mut quattmp = [0.0; 4];
        let full = [inert[0], inert[3], inert[4], inert[3], inert[1], inert[5], inert[4], inert[5], inert[2]];
        eig3(&mut eigval, &mut eigvec, &mut quattmp, &full, 1e-7);

        let inequality_atol = 1e-9;
        let inequality_rtol = 1e-6;

        // check eigval - SHOULD NOT OCCUR
        if eigval[2] <= 0.0 {
            return Err(format!("eigenvalue of mesh inertia must be positive: {}", self.name()));
        }

        if eigval[0] + eigval[1] < eigval[2] * (1.0 - inequality_rtol) - inequality_atol ||
            eigval[0] + eigval[2] < eigval[1] * (1.0 - inequality_rtol) - inequality_atol ||
            eigval[1] + eigval[2] < eigval[0] * (1.0 - inequality_rtol) - inequality_atol {
            return Err(format!("eigenvalues of mesh inertia violate A + B >= C: {}", self.name()));
        }

        // sizes of the equivalent inertia box (boxsz_): read only by FitGeom, outside the subset

        // needreorient_ is false only for a mesh made by marching cubes: here it is always true

        // transform CoM to origin
        for i in 0..self.nvert() {
            dvert[3*i] -= com[0];
            dvert[3*i+1] -= com[1];
            dvert[3*i+2] -= com[2];
        }
        self.rotate(&quattmp, &mut dvert);

        // save the pos and quat that was used to transform the mesh
        self.pos = com;
        self.quat = quattmp;

        // nodes (e.g. volumetric meshes) follow the same transformation as vertices: there are none

        // no radii: make
        self.make_center(&dvert);

        // recompute polygon normals
        self.make_polygon_normals(&dvert);

        // make bounding volume hierarchy
        self.face_aabb.clear();
        self.face_aabb.reserve(3 * self.face.len());
        self.tree.allocate_bounding_volumes(self.nface());
        for i in 0..self.nface() { self.set_bounding_volume(i, &dvert); }
        self.tree.create_bvh();

        // make octree: only for signed distance fields, which are outside the subset

        // narrow back to float
        for i in 0..dvert.len() { self.vert[i] = dvert[i] as f32; }
        Ok(())
    }

    // mjCMesh::ComputeInertia: compute abstract (unitless) inertia, recompute area / volume.
    fn compute_inertia(&self, inert: &mut [f64; 6], com: &[f64; 3], dvert: &[f64]) -> f64 {
        let mut total_volume = 0.0;

        // copy vertices to avoid modifying the original mesh
        let mut vert_centered: Vec<f64> = Vec::with_capacity(3 * self.nvert());

        // translate vertices to origin in order to compute inertia
        for i in 0..self.nvert() {
            vert_centered.push(dvert[3*i] - com[0]);
            vert_centered.push(dvert[3*i+1] - com[1]);
            vert_centered.push(dvert[3*i+2] - com[2]);
        }

        // accumulate products of inertia, recompute volume
        let k: [[usize; 2]; 6] = [[0, 0], [1, 1], [2, 2], [0, 1], [0, 2], [1, 2]];
        let mut p = [0.0; 6];
        let nf = if self.inertia == MESH_INERTIA_CONVEX { self.graph[1] as usize } else { self.nface() };
        let f = if self.inertia == MESH_INERTIA_CONVEX { &self.graph[self.graph_faces()..] } else { &self.face[..] };
        for i in 0..nf {
            let d = &vert_centered[3 * f[3*i] as usize..];
            let e = &vert_centered[3 * f[3*i+1] as usize..];
            let ff = &vert_centered[3 * f[3*i+2] as usize..];

            // get area, normal and center; update volume
            let mut normal = [0.0; 3];
            let mut center = [0.0; 3];
            let area = triangle(Some(&mut normal), Some(&mut center), d, e, ff);
            let mut volume = if self.inertia == MESH_INERTIA_SHELL { area } else { dot3(&center, &normal) * area / 3.0 };

            // if legacy computation requested, then always positive
            if self.inertia == MESH_INERTIA_LEGACY { volume = volume.abs(); }

            // apply formula, accumulate
            total_volume += volume;

            let c = if self.inertia == MESH_INERTIA_SHELL { 12 } else { 20 };
            for j in 0..6 {
                p[j] += volume /
                    c as f64 *
                    (2.0 * (d[k[j][0]] * d[k[j][1]] + e[k[j][0]] * e[k[j][1]] + ff[k[j][0]] * ff[k[j][1]]) +
                     d[k[j][0]] * e[k[j][1]] +
                     d[k[j][1]] * e[k[j][0]] +
                     d[k[j][0]] * ff[k[j][1]] +
                     d[k[j][1]] * ff[k[j][0]] +
                     e[k[j][0]] * ff[k[j][1]] +
                     e[k[j][1]] * ff[k[j][0]]);
            }
        }

        // convert from products of inertia to moments of inertia
        inert[0] = p[1] + p[2];
        inert[1] = p[0] + p[2];
        inert[2] = p[0] + p[1];
        inert[3] = -p[3];
        inert[4] = -p[4];
        inert[5] = -p[5];
        total_volume
    }

    // mjCMesh::Rotate.
    fn rotate(&mut self, quat: &[f64; 4], dvert: &mut [f64]) {
        let neg = [quat[0], -quat[1], -quat[2], -quat[3]];
        let mut mat = [0.0; 9];
        quat2mat(&mut mat, &neg);
        let nv = self.nvert();
        for i in 0..nv {
            let v = [dvert[3*i], dvert[3*i+1], dvert[3*i+2]];
            let mut res = [0.0; 3];
            mulvecmat(&mut res, &v, &mat);
            dvert[3*i..3*i+3].copy_from_slice(&res);

            self.aamm[0] = min(self.aamm[0], dvert[3*i]);
            self.aamm[3] = max(self.aamm[3], dvert[3*i]);
            self.aamm[1] = min(self.aamm[1], dvert[3*i+1]);
            self.aamm[4] = max(self.aamm[4], dvert[3*i+1]);
            self.aamm[2] = min(self.aamm[2], dvert[3*i+2]);
            self.aamm[5] = max(self.aamm[5], dvert[3*i+2]);
        }

        for i in 0..self.nnormal() {
            // normals
            let nrm = [self.normal[3*i] as f64, self.normal[3*i+1] as f64, self.normal[3*i+2] as f64];
            let mut res = [0.0; 3];
            mulvecmat(&mut res, &nrm, &mat);
            for j in 0..3 { self.normal[3*i+j] = res[j] as f32; }
        }
    }

    // mjCMesh::SetBoundingVolume: get bounding volume.
    fn set_bounding_volume(&mut self, faceid: usize, dvert: &[f64]) {
        let k_max_val = f64::MAX;
        let mut face_aamm = [k_max_val, k_max_val, k_max_val, -k_max_val, -k_max_val, -k_max_val];

        for j in 0..3 {
            let vertid = self.face[3 * faceid + j] as usize;
            face_aamm[0] = min(face_aamm[0], dvert[3 * vertid]);
            face_aamm[1] = min(face_aamm[1], dvert[3 * vertid + 1]);
            face_aamm[2] = min(face_aamm[2], dvert[3 * vertid + 2]);
            face_aamm[3] = max(face_aamm[3], dvert[3 * vertid]);
            face_aamm[4] = max(face_aamm[4], dvert[3 * vertid + 1]);
            face_aamm[5] = max(face_aamm[5], dvert[3 * vertid + 2]);
        }

        self.face_aabb.push(0.5 * (face_aamm[0] + face_aamm[3]));
        self.face_aabb.push(0.5 * (face_aamm[1] + face_aamm[4]));
        self.face_aabb.push(0.5 * (face_aamm[2] + face_aamm[5]));
        self.face_aabb.push(0.5 * (face_aamm[3] - face_aamm[0]));
        self.face_aabb.push(0.5 * (face_aamm[4] - face_aamm[1]));
        self.face_aabb.push(0.5 * (face_aamm[5] - face_aamm[2]));

        let center = [self.center[3 * faceid], self.center[3 * faceid + 1], self.center[3 * faceid + 2]];
        let aabb: [f64; 6] = self.face_aabb[6 * faceid..6 * faceid + 6].try_into().unwrap();
        self.tree.add_bounding_volume_mesh(faceid as i32, &center, &aabb);
    }

    // mjCMesh::CopyGraph: copy graph into face data.
    fn copy_graph(&mut self) {
        // only if face data is missing
        if !self.face.is_empty() { return; }

        // get info from graph, allocate
        let numvert = self.graph[0] as usize;
        self.face = vec![0; 3 * self.graph[1] as usize];

        // copy faces
        for i in 0..self.nface() {
            // address in graph
            let j = 2 + 3 * numvert + 3 * self.nface() + 3 * i;

            // copy
            self.face[3*i] = self.graph[j];
            self.face[3*i+1] = self.graph[j+1];
            self.face[3*i+2] = self.graph[j+2];
        }
    }

    // mjCMesh::MakeNormal.
    fn make_normal(&mut self, dvert: &[f64]) {
        // only if normal data is missing: it is always missing here

        // allocate and clear normals
        self.normal = vec![0.0; 3 * self.nvert()];

        if self.facenormal.is_empty() { self.facenormal = vec![0; 3 * self.nface()]; }

        // loop over faces, accumulate vertex normals
        for i in 0..self.nface() {
            // get vertex ids
            let mut vertid = [0usize; 3];
            for j in 0..3 { vertid[j] = self.face[3*i+j] as usize; }

            // get triangle edges
            let mut vec01 = [0.0; 3];
            let mut vec02 = [0.0; 3];
            for j in 0..3 {
                vec01[j] = dvert[3 * vertid[1] + j] - dvert[3 * vertid[0] + j];
                vec02[j] = dvert[3 * vertid[2] + j] - dvert[3 * vertid[0] + j];
            }

            // compute face normal
            let mut nrm = [0.0; 3];
            crossvec(&mut nrm, &vec01, &vec02);
            let area = normvec(&mut nrm, 3);

            // add normal to each vertex with weight = area
            for j in 0..3 {
                // float += double: the sum is formed in double, then narrowed
                for k in 0..3 { self.normal[3 * vertid[j] + k] = (self.normal[3 * vertid[j] + k] as f64 + nrm[k] * area) as f32; }
                self.facenormal[3*i+j] = vertid[j] as i32;
            }
        }

        // remove large-angle faces: smoothnormal is false (outside the subset)
        {
            // allocate removal and clear
            let mut nremove = vec![0.0_f32; 3 * self.nnormal()];

            // remove contributions from faces at large angles with vertex normal
            for i in 0..self.nface() {
                // get vertex ids
                let mut vertid = [0usize; 3];
                for j in 0..3 { vertid[j] = self.face[3*i+j] as usize; }

                // get triangle edges
                let mut vec01 = [0.0; 3];
                let mut vec02 = [0.0; 3];
                for j in 0..3 {
                    vec01[j] = dvert[3 * vertid[1] + j] - dvert[3 * vertid[0] + j];
                    vec02[j] = dvert[3 * vertid[2] + j] - dvert[3 * vertid[0] + j];
                }

                // compute face normal
                let mut nrm = [0.0; 3];
                crossvec(&mut nrm, &vec01, &vec02);
                let area = normvec(&mut nrm, 3);

                // compare to vertex normal, subtract contribution if dot product too small
                for j in 0..3 {
                    // normalized vertex normal
                    let mut vnrm = [self.normal[3 * vertid[j]] as f64,
                        self.normal[3 * vertid[j] + 1] as f64,
                        self.normal[3 * vertid[j] + 2] as f64];
                    normvec(&mut vnrm, 3);

                    // dot too small: remove
                    if dot3(&nrm, &vnrm) < 0.8 {
                        for k in 0..3 { nremove[3 * vertid[j] + k] = (nremove[3 * vertid[j] + k] as f64 + nrm[k] * area) as f32; }
                    }
                }
            }

            // apply removal
            for i in 0..3 * self.nnormal() { self.normal[i] -= nremove[i]; }
        }

        // normalize normals
        for i in 0..self.nnormal() {
            // compute length
            let len: f32 = (self.normal[3*i] * self.normal[3*i] +
                self.normal[3*i+1] * self.normal[3*i+1] +
                self.normal[3*i+2] * self.normal[3*i+2]).sqrt();

            // divide by length
            if len as f64 > MINVAL {
                for j in 0..3 { self.normal[3*i+j] /= len; }
            } else {
                self.normal[3*i] = 0.0;
                self.normal[3*i+1] = 0.0;
                self.normal[3*i+2] = 1.0;
            }
        }
    }

    // mjCMesh::MakeCenter: compute face circumradii.
    fn make_center(&mut self, dvert: &[f64]) {
        // allocate and clear
        self.center = vec![0.0; 3 * self.nface()];

        for i in 0..self.nface() {
            // get vertex ids
            let vertid = [self.face[3*i] as usize, self.face[3*i+1] as usize, self.face[3*i+2] as usize];

            // get triangle edges
            let mut a = [0.0; 3];
            let mut b = [0.0; 3];
            for j in 0..3 {
                a[j] = dvert[3 * vertid[0] + j] - dvert[3 * vertid[2] + j];
                b[j] = dvert[3 * vertid[1] + j] - dvert[3 * vertid[2] + j];
            }

            // compute face normal
            let mut nrm = [0.0; 3];
            crossvec(&mut nrm, &a, &b);

            // compute circumradius
            let norm_a_2 = dot3(&a, &a);
            let norm_b_2 = dot3(&b, &b);
            let area = dot3(&nrm, &nrm).sqrt();

            // compute circumcenter
            let mut res = [0.0; 3];
            let vec = [norm_a_2 * b[0] - norm_b_2 * a[0],
                norm_a_2 * b[1] - norm_b_2 * a[1],
                norm_a_2 * b[2] - norm_b_2 * a[2]];
            crossvec(&mut res, &vec, &nrm);
            self.center[3*i] = res[0] / (2.0 * area * area) + dvert[3 * vertid[2]];
            self.center[3*i+1] = res[1] / (2.0 * area * area) + dvert[3 * vertid[2] + 1];
            self.center[3*i+2] = res[2] / (2.0 * area * area) + dvert[3 * vertid[2] + 2];
        }
    }

    // mjCMesh::MakePolygonNormals: compute the normals of the polygons.
    fn make_polygon_normals(&mut self, dvert: &[f64]) {
        for i in 0..self.polygons.len() {
            let mut n = [0.0; 3];
            let nvert = self.polygons[i].len();

            // compute normal using Newell's method
            for j in 0..nvert {
                let v0 = &dvert[3 * self.polygons[i][j] as usize..];
                let v1 = &dvert[3 * self.polygons[i][(j + 1) % nvert] as usize..];
                n[0] += (v0[1] - v1[1]) * (v0[2] + v1[2]);
                n[1] += (v0[2] - v1[2]) * (v0[0] + v1[0]);
                n[2] += (v0[0] - v1[0]) * (v0[1] + v1[1]);
            }
            let nrm = dot3(&n, &n).sqrt();
            if nrm < EPS {
                self.polygon_normals[3*i] = 1.0;
                self.polygon_normals[3*i+1] = 0.0;
                self.polygon_normals[3*i+2] = 0.0;
            } else {
                self.polygon_normals[3*i] = n[0] / nrm;
                self.polygon_normals[3*i+1] = n[1] / nrm;
                self.polygon_normals[3*i+2] = n[2] / nrm;
            }
        }
    }

    // mjCMesh::MakePolygons: merge coplanar mesh triangular faces into polygonal sides to represent
    // the geometry of the mesh. The C keeps the polygons in an unordered_map, whose iteration order is
    // unspecified; the polygons are listed here in the order their keys first occur.
    fn make_polygons(&mut self, dvert: &[f64]) {
        let k_angle_tol = 0.01;
        let mut mesh_polygons: Vec<MeshPolygon> = Vec::new();
        let mut index: std::collections::HashMap<(u64, u64), usize> = std::collections::HashMap::new();
        self.polygons.clear();
        self.polygon_normals.clear();
        self.polygon_map.clear();
        self.polygon_map.resize(self.nvert(), Vec::new());

        // we need a convex mesh, so we use graph faces
        let faces = self.graph_faces();
        let nfaces = self.graph[1] as usize;

        // process each face
        for i in 0..nfaces {
            let vi1 = self.graph[faces + 3*i];
            let vi2 = self.graph[faces + 3*i + 1];
            let vi3 = self.graph[faces + 3*i + 2];
            let v1 = &dvert[3 * vi1 as usize..];
            let v2 = &dvert[3 * vi2 as usize..];
            let v3 = &dvert[3 * vi3 as usize..];

            let key = match mesh_polygon_key(v1, v2, v3, k_angle_tol) { Some(key) => key, None => continue };
            // the keys are rounded values; +0.0 makes -0.0 and 0.0 one key, as C's == does
            let bits = ((key.0 + 0.0).to_bits(), (key.1 + 0.0).to_bits());
            let found = index.get(&bits).copied();
            match found {
                None => {
                    let theta = k_angle_tol * key.0;
                    let phi = k_angle_tol * key.1;
                    index.insert(bits, mesh_polygons.len());
                    mesh_polygons.push(MeshPolygon::new(vi1, vi2, vi3, theta, phi));
                }
                Some(it) => mesh_polygons[it].insert_face(vi1, vi2, vi3),
            }
        }

        for polygon in &mesh_polygons {
            let paths = polygon.paths();

            // separate the polygons if they were grouped together
            for path in paths {
                if path.len() < 3 { continue; }
                self.polygons.push(path);
                self.polygon_normals.push(polygon.normal[0]);
                self.polygon_normals.push(polygon.normal[1]);
                self.polygon_normals.push(polygon.normal[2]);
            }
        }

        // populate the polygon map
        for i in 0..self.polygons.len() {
            for j in 0..self.polygons[i].len() { self.polygon_map[self.polygons[i][j] as usize].push(i as i32); }
        }
    }
}

// mjCGeom::ComputeAABB.
fn compute_aabb(aabb: &mut [f64; 6], g: &GeomSpec, mesh: Option<&Mesh>) {
    let mut aamm = [0.0; 6];
    match g.r#type {
        GEOM_SPHERE => { aamm[3] = g.size[0]; aamm[4] = g.size[0]; aamm[5] = g.size[0]; }
        GEOM_CAPSULE => { aamm[3] = g.size[0]; aamm[4] = g.size[0]; aamm[5] = g.size[0] + g.size[1]; }
        GEOM_CYLINDER => { aamm[3] = g.size[0]; aamm[4] = g.size[0]; aamm[5] = g.size[1]; }
        GEOM_MESH => { aamm = mesh.unwrap().aamm; }
        GEOM_PLANE => { aamm[3] = MAXVAL; aamm[4] = MAXVAL; aamm[5] = 0.0; }
        _ => { aamm[3..].copy_from_slice(&g.size); }
    }
    if g.r#type != GEOM_MESH {
        aamm[0] = -aamm[3]; aamm[1] = -aamm[4]; aamm[2] = -aamm[5];
        if g.r#type == GEOM_PLANE { aamm[2] = -MAXVAL; }
    }
    aabb[0] = (aamm[3] + aamm[0]) / 2.0;
    aabb[1] = (aamm[4] + aamm[1]) / 2.0;
    aabb[2] = (aamm[5] + aamm[2]) / 2.0;
    aabb[3] = (aamm[3] - aamm[0]) / 2.0;
    aabb[4] = (aamm[4] - aamm[1]) / 2.0;
    aabb[5] = (aamm[5] - aamm[2]) / 2.0;
}

// The identical fromto blocks in mjCGeom::Compile and mjCSite::Compile.
fn compile_fromto(pos: &mut [f64; 3], quat: &mut [f64; 4], size: &mut [f64; 3], fromto: &[f64; 6], r#type: i32) -> Result<(), String> {
    if fromto[0].is_nan() { return Ok(()); }
    if !matches!(r#type, GEOM_CAPSULE | GEOM_CYLINDER | GEOM_ELLIPSOID | GEOM_BOX) {
        return Err("fromto requires capsule, cylinder, box or ellipsoid".into());
    }
    if pos[0] != 0.0 || pos[1] != 0.0 || pos[2] != 0.0 { return Err("both pos and fromto defined".into()); }
    let mut vec = [fromto[0] - fromto[3], fromto[1] - fromto[4], fromto[2] - fromto[5]];
    size[1] = normvec(&mut vec, 3) / 2.0;
    if size[1] < EPS { return Err("fromto points too close".into()); }
    if r#type == GEOM_ELLIPSOID || r#type == GEOM_BOX { size[2] = size[1]; size[1] = size[0]; }
    pos[0] = (fromto[0] + fromto[3]) / 2.0;
    pos[1] = (fromto[1] + fromto[4]) / 2.0;
    pos[2] = (fromto[2] + fromto[5]) / 2.0;
    z2quat(quat, &vec);
    Ok(())
}

// mjCGeom::Compile; hfield/SDF assets are outside the specification. `mesh` is the mesh the
// geom names (IndexAssets), `id` the geom's id.
fn compile_geom(g: &mut GeomSpec, aabb: &mut [f64; 6], static_body: bool, mesh: Option<&Mesh>, id: usize) -> Result<(), String> {
    if !matches!(g.r#type, GEOM_PLANE | GEOM_SPHERE | GEOM_CAPSULE | GEOM_ELLIPSOID | GEOM_CYLINDER | GEOM_BOX | GEOM_MESH) {
        return Err("geom type outside the model specification".into());
    }
    if !matches!(g.condim, 1 | 3 | 4 | 6) { return Err("invalid condim in geom".into()); }

    // check mesh
    if g.r#type == GEOM_MESH && mesh.is_none() {
        return Err(format!("mesh geom '{}' (id = {}) must have valid meshid", g.name.as_deref().unwrap_or(""), id));
    }
    if g.r#type == GEOM_PLANE && !static_body { return Err("plane only allowed in static bodies".into()); }
    normvec(&mut g.quat, 4);
    compile_fromto(&mut g.pos, &mut g.quat, &mut g.size, &g.fromto, g.r#type)?;

    // mesh: accumulate frame, fit geom if needed
    if let Some(mesh) = mesh {
        // check for inapplicable fromto
        if !g.fromto[0].is_nan() { return Err("fromto cannot be used with mesh geom".into()); }

        // fitting a primitive to a mesh (FitGeom) is outside the subset
        if g.r#type != GEOM_MESH { return Err("a geom fitted to a mesh is outside the builder subset".into()); }
        if g.typeinertia == 1 {
            return Err("for mesh geoms, inertia should be specified in the mesh asset".into());
        }
        let center = [0.0; 3];

        // rotate center to geom frame and add it to mesh frame
        let mut meshpos = [0.0; 3];
        rot_vec_quat(&mut meshpos, &center, &mesh.quat);
        meshpos[0] += mesh.pos[0];
        meshpos[1] += mesh.pos[1];
        meshpos[2] += mesh.pos[2];

        // accumulate mesh frame into geom frame
        frameaccum(&mut g.pos, &mut g.quat, &meshpos, &mesh.quat);

        // re-express surfacevel in the compiled geom frame, which absorbed the mesh frame
        if g.surfacevel.iter().any(|&v| v != 0.0) {
            // angular origin moves to meshpos: linear part gains omega x meshpos
            let mut wxp = [0.0; 3];
            crossvec(&mut wxp, &[g.surfacevel[3], g.surfacevel[4], g.surfacevel[5]], &meshpos);
            g.surfacevel[0] += wxp[0];
            g.surfacevel[1] += wxp[1];
            g.surfacevel[2] += wxp[2];

            // rotate both parts by the inverse mesh orientation
            let mq = &mesh.quat;
            let invq = [mq[0], -mq[1], -mq[2], -mq[3]];
            let mut tmp = [0.0; 3];
            rot_vec_quat(&mut tmp, &[g.surfacevel[0], g.surfacevel[1], g.surfacevel[2]], &invq);
            g.surfacevel[..3].copy_from_slice(&tmp);
            rot_vec_quat(&mut tmp, &[g.surfacevel[3], g.surfacevel[4], g.surfacevel[5]], &invq);
            g.surfacevel[3..].copy_from_slice(&tmp);
        }
    }
    checksize(&g.size, g.r#type)?;

    // set mesh sizes in geom.size
    if g.r#type == GEOM_MESH {
        let aamm = &mesh.unwrap().aamm;
        g.size[0] = max(aamm[0].abs(), aamm[3].abs());
        g.size[1] = max(aamm[1].abs(), aamm[4].abs());
        g.size[2] = max(aamm[2].abs(), aamm[5].abs());
    }
    for s in g.size { if s.is_nan() { return Err("nan size in geom".into()); } }
    compute_aabb(aabb, g, mesh);
    // inertiafromgeom: outside the explicit-inertial subset (GetVolume/SetInertia omitted).
    // The compiled fluid coefficients are copied directly from the unchanged spec in CopyTree.
    // plugins: outside the subset.
    // frames: outside the subset.
    Ok(())
}

// mjCSite::Compile.
fn compile_site(s: &mut SiteSpec) -> Result<(), String> {
    if !matches!(s.r#type, GEOM_SPHERE | GEOM_CAPSULE | GEOM_ELLIPSOID | GEOM_CYLINDER | GEOM_BOX) {
        return Err("site type outside the primitive model specification".into());
    }
    compile_fromto(&mut s.pos, &mut s.quat, &mut s.size, &s.fromto, s.r#type)?;
    normvec(&mut s.quat, 4);
    checksize(&s.size, s.r#type)
}

struct Pair {
    spec: PairSpec,
    geom1: usize,
    geom2: usize,
    signature: i32,
}
struct Exclude {
    spec: ExcludeSpec,
    signature: i32,
}
struct Equality {
    spec: EqualitySpec,
    obj1id: i32,
    obj2id: i32,
}
struct Tree {
    bodies: Vec<BodySpec>,
    children: Vec<Vec<usize>>,
    weldid: Vec<usize>,
    dofnum: Vec<usize>,
    body_names: HashMap<String, usize>,
    joint_names: HashMap<String, usize>,
    geom_names: HashMap<String, usize>,
    site_names: HashMap<String, usize>,
    joint_types: Vec<i32>,
    mesh_names: HashMap<String, usize>,
    geoms: Vec<(usize, usize)>,
    // The mesh each geom names, by id, or -1 (IndexAssets).
    geom_mesh: Vec<Vec<i32>>,
    geom_aabb: Vec<Vec<[f64; 6]>>,
    body_bvh: Vec<BoundingVolumeHierarchy>,
}

// mjCModel::ProcessLists / MakeLists: depth-first body order, then each body's
// authored object order. Rust ownership replaces the compiler's pointer lists.
fn object_lists(spec: &ModelSpec) -> Result<Tree, String> {
    if spec.bodies.is_empty() { return Err("body zero must be the world".into()); }
    if spec.bodies[0].parent != 0 { return Err("world must parent itself".into()); }
    if !spec.bodies[0].joints.is_empty() { return Err("joint found in world body".into()); }
    if spec.bodies.len() >= 65534 { return Err("number of bodies plus flexes must be less than 65534".into()); }
    let mut children = vec![Vec::new(); spec.bodies.len()];
    for (id, b) in spec.bodies.iter().enumerate().skip(1) {
        if b.parent >= id { return Err("body parent must precede child".into()); }
        children[b.parent].push(id);
    }
    let mut stack = vec![0];
    let mut bodies = Vec::new();
    let mut ids = vec![0; spec.bodies.len()];
    while let Some(id) = stack.pop() {
        ids[id] = bodies.len();
        let mut b = spec.bodies[id].clone();
        b.parent = ids[b.parent];
        if id == 0 {
            b = BodySpec { name: Some("world".into()), geoms: b.geoms, sites: b.sites, ..BodySpec::default() };
        }
        bodies.push(b);
        for &child in children[id].iter().rev() { stack.push(child); }
    }
    let nbody = bodies.len();
    let geom_aabb = bodies.iter().map(|b| vec![[0.0; 6]; b.geoms.len()]).collect();
    let geom_mesh = bodies.iter().map(|b| vec![-1; b.geoms.len()]).collect();
    let mut tree = Tree {
        bodies, children: vec![Vec::new(); nbody], weldid: vec![0; nbody], dofnum: vec![0; nbody],
        body_names: HashMap::new(), joint_names: HashMap::new(), geom_names: HashMap::new(), site_names: HashMap::new(),
        mesh_names: HashMap::new(), joint_types: Vec::new(), geoms: Vec::new(), geom_mesh, geom_aabb,
        body_bvh: (0..nbody).map(|_| BoundingVolumeHierarchy::default()).collect(),
    };
    let mut njnt = 0;
    let mut ngeom = 0;
    let mut nsite = 0;
    for (i, mesh) in spec.meshes.iter().enumerate() { insert_name(&mut tree.mesh_names, &mesh.name, i)?; }
    for (i, b) in tree.bodies.iter().enumerate() {
        if i > 0 { tree.children[b.parent].push(i); }
        insert_name(&mut tree.body_names, &b.name, i)?;
        for j in &b.joints {
            insert_name(&mut tree.joint_names, &j.name, njnt)?;
            tree.joint_types.push(j.r#type);
            njnt += 1;
        }
        for (k, g) in b.geoms.iter().enumerate() {
            insert_name(&mut tree.geom_names, &g.name, ngeom)?;
            tree.geoms.push((i, k));
            ngeom += 1;
        }
        for s in &b.sites { insert_name(&mut tree.site_names, &s.name, nsite)?; nsite += 1; }
    }
    for names in [spec.pairs.iter().map(|p| &p.name).collect::<Vec<_>>(), spec.excludes.iter().map(|e| &e.name).collect(), spec.equalities.iter().map(|e| &e.name).collect()] {
        let mut map = HashMap::new();
        for (i, name) in names.into_iter().enumerate() { insert_name(&mut map, name, i)?; }
    }
    Ok(tree)
}

// mjCModel::ProcessLists name uniqueness check.
fn insert_name(map: &mut HashMap<String, usize>, name: &Option<String>, id: usize) -> Result<(), String> {
    if let Some(name) = name.as_ref().filter(|s| !s.is_empty()) {
        if name.as_bytes().contains(&0) { return Err("name contains a zero byte".into()); }
        if map.insert(name.clone(), id).is_some() { return Err(format!("repeated name '{name}'")); }
    }
    Ok(())
}

// mjCModel::FindObject, on the compiled object list.
fn find_object(map: &HashMap<String, usize>, name: &Option<String>) -> Result<usize, String> {
    let name = name.as_deref().unwrap_or("");
    map.get(name).copied().ok_or_else(|| format!("unknown element '{name}'"))
}

// mjCModel::IndexAssets: map each geom's mesh name to the mesh.
fn index_assets(tree: &mut Tree) -> Result<(), String> {
    for (gid, &(i, k)) in tree.geoms.iter().enumerate() {
        let name = tree.bodies[i].geoms[k].meshname.as_deref().unwrap_or("");
        if name.is_empty() { continue; }
        match tree.mesh_names.get(name) {
            Some(&id) => tree.geom_mesh[i][k] = id as i32,
            None => return Err(format!("mesh '{name}' not found in geom {gid}")),
        }
    }
    Ok(())
}

// mjCBody::Compile, continuing compile_inertial in the compiler's order.
fn compile_body(tree: &mut Tree, i: usize, c: &CompilerSpec, geomadr: usize, meshes: &[Mesh]) -> Result<(), String> {
    compile_inertial(&mut tree.bodies[i], i, c)?;
    for &child in &tree.children[i] {
        tree.weldid[child] = if !tree.bodies[child].joints.is_empty() || tree.bodies[child].mocap { child } else { tree.weldid[i] };
    }
    let static_body = tree.weldid[i] == 0 || tree.bodies[tree.weldid[i]].mocap;
    let b = &mut tree.bodies[i];
    for (j, g) in b.geoms.iter_mut().enumerate() {
        let mesh = if tree.geom_mesh[i][j] >= 0 { Some(&meshes[tree.geom_mesh[i][j] as usize]) } else { None };
        compile_geom(g, &mut tree.geom_aabb[i][j], static_body, mesh, geomadr + j)?;
    }
    // inertiafromgeom: outside the subset; InertiaFromGeom and its helpers are omitted.
    if b.ipos[0].is_nan() { b.ipos = b.pos; b.iquat = b.quat; }
    if i > 0 {
        b.mass = max(b.mass, c.boundmass);
        b.inertia[0] = max(b.inertia[0], c.boundinertia);
        b.inertia[1] = max(b.inertia[1], c.boundinertia);
        b.inertia[2] = max(b.inertia[2], c.boundinertia);
        if b.mass < 0.0 || b.inertia[0] < 0.0 || b.inertia[1] < 0.0 || b.inertia[2] < 0.0 {
            return Err("mass and inertia cannot be negative".into());
        }
        if b.inertia[0] + b.inertia[1] < b.inertia[2] || b.inertia[0] + b.inertia[2] < b.inertia[1] || b.inertia[1] + b.inertia[2] < b.inertia[0] {
            if c.balanceinertia { b.inertia = [(b.inertia[0] + b.inertia[1] + b.inertia[2]) / 3.0; 3]; }
            else { return Err("inertia must satisfy A + B >= C; use 'balanceinertia' to fix".into()); }
        }
    }
    // alignfree: outside the subset; frame inversion/accumulation helpers omitted.
    if b.joints.iter().any(|j| j.align == 1) { return Err("free-joint alignment is outside the builder subset".into()); }
    compute_bvh(&mut tree.body_bvh[i], b, &tree.geom_aabb[i], geomadr);
    let mut dofnum = 0;
    for j in &mut b.joints { dofnum += compile_joint(j)?; }
    tree.dofnum[i] = dofnum;
    if dofnum > 6 { return Err("more than 6 dofs in body".into()); }
    let mut hasball = false;
    for j in &b.joints {
        if (j.r#type == JNT_BALL || j.r#type == JNT_HINGE) && hasball { return Err("ball followed by rotation in body".into()); }
        if j.r#type == JNT_BALL { hasball = true; }
    }
    if b.mocap && (dofnum != 0 || b.parent != 0) { return Err("mocap body is not a fixed child of world".into()); }
    // xpos0/xquat0 are compiler-only state consumed by excluded frame/gauge operations.
    for s in &mut b.sites { compile_site(s)?; }
    // cameras: outside the subset.
    // lights: outside the subset.
    Ok(())
}

// mjCPair::Compile and ResolveReferences.
fn compile_pair(spec: &PairSpec, tree: &Tree) -> Result<Pair, String> {
    let mut p = spec.clone();
    if !matches!(p.condim, 1 | 3 | 4 | 6) { return Err("invalid condim in contact pair".into()); }
    let mut geom1 = find_object(&tree.geom_names, &p.geomname1)?;
    let mut geom2 = find_object(&tree.geom_names, &p.geomname2)?;
    if tree.geoms[geom1].0 > tree.geoms[geom2].0 { std::mem::swap(&mut geom1, &mut geom2); }
    let (b1, g1) = tree.geoms[geom1];
    let (b2, g2) = tree.geoms[geom2];
    let a = &tree.bodies[b1].geoms[g1];
    let b = &tree.bodies[b2].geoms[g2];
    if p.margin.is_nan() { p.margin = max(a.margin, b.margin); }
    if p.gap.is_nan() { p.gap = max(a.gap, b.gap); }
    if a.priority != b.priority {
        let high = if a.priority > b.priority { a } else { b };
        if p.condim < 0 { p.condim = high.condim; }
        if p.friction[0].is_nan() { p.friction = [high.friction[0], high.friction[0], high.friction[1], high.friction[2], high.friction[2]]; }
        if p.solref[0].is_nan() { p.solref = high.solref; }
        if p.solimp[0].is_nan() { p.solimp = high.solimp; }
    } else {
        if p.condim < 0 { p.condim = a.condim.max(b.condim); }
        if p.friction[0].is_nan() {
            p.friction[0] = max(a.friction[0], b.friction[0]); p.friction[1] = p.friction[0];
            p.friction[2] = max(a.friction[1], b.friction[1]);
            p.friction[3] = max(a.friction[2], b.friction[2]); p.friction[4] = p.friction[3];
        }
        let mix = if a.solmix >= EPS && b.solmix >= EPS { a.solmix / (a.solmix + b.solmix) }
            else if a.solmix < EPS && b.solmix < EPS { 0.5 } else if a.solmix < EPS { 0.0 } else { 1.0 };
        if p.solref[0].is_nan() {
            if a.solref[0] > 0.0 && b.solref[0] > 0.0 {
                for i in 0..NREF { p.solref[i] = mix * a.solref[i] + (1.0 - mix) * b.solref[i]; }
            } else { for i in 0..NREF { p.solref[i] = min(a.solref[i], b.solref[i]); } }
        }
        if p.solimp[0].is_nan() { for i in 0..NIMP { p.solimp[i] = mix * a.solimp[i] + (1.0 - mix) * b.solimp[i]; } }
    }
    Ok(Pair { spec: p, geom1, geom2, signature: (((b1 as u32) << 16) + b2 as u32) as i32 })
}

// mjCBodyPair::Compile and ResolveReferences.
fn compile_exclude(spec: &ExcludeSpec, tree: &Tree) -> Result<Exclude, String> {
    let mut body1 = find_object(&tree.body_names, &spec.bodyname1)?;
    let mut body2 = find_object(&tree.body_names, &spec.bodyname2)?;
    if body1 > body2 { std::mem::swap(&mut body1, &mut body2); }
    Ok(Exclude { spec: spec.clone(), signature: (((body1 as u32) << 16) + body2 as u32) as i32 })
}

// mjCEquality::Compile and ResolveReferences. OBJ_UNKNOWN on a body constraint
// takes the same default as the MJCF reader.
fn compile_equality(spec: &EqualitySpec, tree: &Tree) -> Result<Equality, String> {
    let mut e = spec.clone();
    let objects = match e.r#type {
        EQ_CONNECT | EQ_WELD => {
            if e.objtype == OBJ_UNKNOWN { e.objtype = OBJ_BODY; }
            match e.objtype { OBJ_BODY => &tree.body_names, OBJ_SITE => &tree.site_names, _ => return Err("constraint supports only sites and bodies".into()) }
        }
        EQ_JOINT => &tree.joint_names,
        _ => return Err("equality type outside the rigid-body specification".into()),
    };
    let obj1id = find_object(objects, &e.name1)? as i32;
    let mut obj2id = if e.name2.as_deref().unwrap_or("").is_empty() { -1 } else { find_object(objects, &e.name2)? as i32 };
    if matches!(e.r#type, EQ_CONNECT | EQ_WELD) && e.objtype == OBJ_BODY && obj2id == -1 { obj2id = 0; }
    if obj1id == obj2id { return Err("element is repeated in equality constraint".into()); }
    if e.r#type == EQ_JOINT {
        let jt1 = tree.joint_types[obj1id as usize];
        let jt2 = if obj2id >= 0 { tree.joint_types[obj2id as usize] } else { JNT_HINGE };
        if !matches!(jt1, JNT_HINGE | JNT_SLIDE) || !matches!(jt2, JNT_HINGE | JNT_SLIDE) { return Err("only HINGE and SLIDE joint allowed in constraint".into()); }
    }
    if e.r#type == EQ_CONNECT && e.objtype == OBJ_SITE { e.data.fill(0.0); }
    if e.r#type == EQ_WELD {
        let body = e.objtype == OBJ_BODY;
        e.data[11] = (body && e.data[6] == 0.0 && e.data[7] == 0.0 && e.data[8] == 0.0 && e.data[9] == 0.0) as u8 as f64;
        if body && e.data[11] == 0.0 && normvec(&mut e.data[6..10], 4) == 0.0 { e.data[6..10].copy_from_slice(&[1.0, 0.0, 0.0, 0.0]); }
    }
    Ok(Equality { spec: e, obj1id, obj2id })
}

// IsNullPose.
fn is_null_pose(pos: &[f64; 3], quat: Option<&[f64; 4]>) -> bool {
    if pos[0].abs() >= 1e-6 || pos[1].abs() >= 1e-6 || pos[2].abs() >= 1e-6 { return false; }
    if let Some(q) = quat {
        let plus = (q[0]-1.0).abs() < 1e-6 && q[1].abs() < 1e-6 && q[2].abs() < 1e-6 && q[3].abs() < 1e-6;
        let minus = (q[0]+1.0).abs() < 1e-6 && q[1].abs() < 1e-6 && q[2].abs() < 1e-6 && q[3].abs() < 1e-6;
        return plus || minus;
    }
    true
}

// mjCJoint::nq / nv (the scalar-joint fallback is reached only after validation).
fn joint_nq(j: &JointSpec) -> usize { match j.r#type { JNT_FREE => 7, JNT_BALL => 4, _ => 1 } }
fn joint_nv(j: &JointSpec) -> usize { match j.r#type { JNT_FREE => 6, JNT_BALL => 3, _ => 1 } }

// mjCModel::ComputeSparseSizes, rigid bodies only.
fn compute_sparse_sizes(tree: &Tree, nv: usize, sizes: &mut [i64]) {
    if nv == 0 { return; }
    let nbody = tree.bodies.len();
    let mut dof_parentid_pre = vec![-1; nv];
    let mut dof_bodyid_pre = vec![0; nv];
    let mut body_simple_pre = vec![0; nbody];
    let mut body_rootid_pre = vec![0; nbody];
    let mut dof_simplenum_pre = vec![0; nv];
    let mut body_lastdof_map = vec![0; nbody];
    body_lastdof_map[0] = -1;
    let mut dofadr = 0;
    for (i, pb) in tree.bodies.iter().enumerate() {
        let mut current_parent_dof = if i > 0 { body_lastdof_map[pb.parent] } else { -1 };
        for jnt in &pb.joints {
            for _ in 0..joint_nv(jnt) {
                dof_bodyid_pre[dofadr] = i;
                dof_parentid_pre[dofadr] = current_parent_dof;
                current_parent_dof = dofadr as i32;
                dofadr += 1;
            }
        }
        body_lastdof_map[i] = current_parent_dof;
    }
    let mut n_m = 0;
    for i in 0..nv {
        let mut j = i as i32;
        while j != -1 { n_m += 1; j = dof_parentid_pre[j as usize]; }
    }
    set_size(sizes, "n_m", n_m);
    set_size(sizes, "n_d", 2*n_m-nv);
    let mut subtreedofs = vec![0; nbody];
    for i in (0..nbody).rev() {
        subtreedofs[i] = tree.dofnum[i];
        for &child in &tree.children[i] { subtreedofs[i] += subtreedofs[child]; }
    }
    let mut n_b = 0;
    for i in 0..nbody {
        n_b += subtreedofs[i];
        let mut parent = tree.bodies[i].parent;
        while parent > 0 { n_b += tree.dofnum[parent]; parent = tree.bodies[parent].parent; }
    }
    set_size(sizes, "n_b", n_b);
    for (i, pb) in tree.bodies.iter().enumerate() {
        let parentid = pb.parent;
        body_rootid_pre[i] = if i == 0 || parentid == 0 { i } else { body_rootid_pre[parentid] };
        body_simple_pre[i] = (is_null_pose(&pb.ipos, Some(&pb.iquat)) &&
            (body_rootid_pre[i] == i || (parentid > 0 && tree.bodies[parentid].parent == 0 && tree.dofnum[parentid] == 0))) as i32;
        if pb.simple == 0 { body_simple_pre[i] = 0; }
    }
    for i in 1..nbody { body_simple_pre[tree.bodies[i].parent] = 0; }
    for i in 1..nbody {
        if body_simple_pre[i] == 0 { continue; }
        let pb = &tree.bodies[i];
        let mut rotfound = false;
        for pj in &pb.joints {
            let axis_aligned = (pj.axis[0].abs() > EPS) as u8 + (pj.axis[1].abs() > EPS) as u8 + (pj.axis[2].abs() > EPS) as u8 == 1;
            if rotfound || !is_null_pose(&pj.pos, None) || (matches!(pj.r#type, JNT_HINGE | JNT_SLIDE) && !axis_aligned) {
                body_simple_pre[i] = 0; break;
            }
            if matches!(pj.r#type, JNT_BALL | JNT_HINGE) { rotfound = true; }
        }
        if body_simple_pre[i] == 0 { continue; }
        if tree.dofnum[i] > 0 {
            body_simple_pre[i] = 2;
            for pj in &pb.joints { if pj.r#type != JNT_SLIDE { body_simple_pre[i] = 1; break; } }
        }
    }
    // tendons and actuator armature: outside the subset; GetBodyIdFromWrap omitted.
    let mut count = 0;
    for i in (0..nv).rev() {
        if body_simple_pre[dof_bodyid_pre[i]] != 0 { count += 1; } else { count = 0; }
        dof_simplenum_pre[i] = count;
    }
    let mut n_od = 0;
    for i in 0..nv {
        if dof_simplenum_pre[i] == 0 {
            let mut j = i as i32;
            while j != -1 { if j != i as i32 { n_od += 1; } j = dof_parentid_pre[j as usize]; }
        }
    }
    set_size(sizes, "n_c", n_od+nv);
}

// mjCModel::SetSizes: named construction arguments map to MJMODEL_SIZES.
fn set_size(sizes: &mut [i64], name: &str, value: usize) {
    sizes[MODEL_SIZES.iter().position(|&s| s == name).unwrap()] = value as i64;
}

// namelist and addtolist.
fn namelist(list: &[&Option<String>], adr: &mut usize, name_adr: &mut [i32], names: &mut [u8], map: &mut [i32]) {
    let map_size = 2 * list.len();
    for (i, name) in list.iter().enumerate() {
        let name = name.as_deref().unwrap_or("");
        if name.is_empty() { continue; }
        let mut j = crate::name::hash_string(name, map_size as u64) as usize;
        while map[j] != -1 { j = (j + 1) % map_size; }
        map[j] = i as i32;
    }
    for (i, name) in list.iter().enumerate() {
        let name = name.as_deref().unwrap_or("");
        name_adr[i] = *adr as i32;
        names[*adr..*adr+name.len()].copy_from_slice(name.as_bytes());
        *adr += name.len();
        names[*adr] = 0;
        *adr += 1;
    }
}

// mjCModel::CopyNames.
fn copy_names(m: &mut Model, modelname: &str, tree: &Tree, meshes: &[Mesh], pairs: &[Pair], excludes: &[Exclude], equalities: &[Equality]) {
    let mut adr = modelname.len()+1;
    m.names[..modelname.len()].copy_from_slice(modelname.as_bytes());
    m.names_map.fill(-1);
    let mut mapadr = 0;
    macro_rules! list {
        ($field:ident, $names:expr) => {{
            let names: Vec<&Option<String>> = $names;
            namelist(&names, &mut adr, &mut m.$field, &mut m.names, &mut m.names_map[mapadr..]);
            mapadr += 2 * names.len();
        }};
    }
    list!(name_bodyadr, tree.bodies.iter().map(|b| &b.name).collect());
    list!(name_jntadr, tree.bodies.iter().flat_map(|b| b.joints.iter().map(|j| &j.name)).collect());
    list!(name_geomadr, tree.bodies.iter().flat_map(|b| b.geoms.iter().map(|g| &g.name)).collect());
    list!(name_siteadr, tree.bodies.iter().flat_map(|b| b.sites.iter().map(|s| &s.name)).collect());
    list!(name_meshadr, meshes.iter().map(|mesh| &mesh.name).collect());
    list!(name_pairadr, pairs.iter().map(|p| &p.spec.name).collect());
    list!(name_excludeadr, excludes.iter().map(|e| &e.spec.name).collect());
    list!(name_eqadr, equalities.iter().map(|e| &e.spec.name).collect());
    assert_eq!(mapadr, m.nnames_map);
    assert_eq!(adr, m.nnames);
}

// mjCModel::CopyTree.
fn copy_tree(m: &mut Model, tree: &Tree, meshes: &[Mesh]) -> Result<(), String> {
    let mut jntadr = 0;
    let mut dofadr = 0;
    let mut qposadr = 0;
    let mut plasticadr = 0;
    let mut geomadr = 0;
    let mut siteadr = 0;
    let mut mocapid = 0;
    let mut bvh_adr = 0;
    let mut lastdof = vec![-1; m.nbody];
    m.body_sameframe[0] = SAMEFRAME_BODY as u8;
    macro_rules! copy {
        ($field:ident, $id:expr, $values:expr) => {{
            let values = &$values;
            let adr = $id * values.len();
            m.$field[adr..adr+values.len()].copy_from_slice(values);
        }};
    }
    for (i, pb) in tree.bodies.iter().enumerate() {
        m.body_parentid[i] = pb.parent as i32;
        m.body_weldid[i] = tree.weldid[i] as i32;
        m.body_mocapid[i] = if pb.mocap { let id = mocapid; mocapid += 1; id } else { -1 };
        m.body_jntnum[i] = pb.joints.len() as i32;
        m.body_jntadr[i] = if pb.joints.is_empty() { -1 } else { jntadr as i32 };
        m.body_dofnum[i] = tree.dofnum[i] as i32;
        m.body_dofadr[i] = if tree.dofnum[i] == 0 { -1 } else { dofadr as i32 };
        m.body_geomnum[i] = pb.geoms.len() as i32;
        m.body_geomadr[i] = if pb.geoms.is_empty() { -1 } else { geomadr as i32 };
        copy!(body_pos, i, pb.pos);
        copy!(body_quat, i, pb.quat);
        copy!(body_ipos, i, pb.ipos);
        copy!(body_iquat, i, pb.iquat);
        m.body_mass[i] = pb.mass;
        copy!(body_inertia, i, pb.inertia);
        m.body_gravcomp[i] = pb.gravcomp;
        // user data: outside the subset.
        let bvh = &tree.body_bvh[i];
        let nbvh = bvh.nodeid.len();
        m.body_bvhadr[i] = if nbvh != 0 { bvh_adr as i32 } else { -1 };
        m.body_bvhnum[i] = nbvh as i32;
        if nbvh != 0 {
            m.bvh_child[2*bvh_adr..2*(bvh_adr+nbvh)].copy_from_slice(&bvh.child);
            m.bvh_depth[bvh_adr..bvh_adr+nbvh].copy_from_slice(&bvh.level);
            m.bvh_nodeid[bvh_adr..bvh_adr+nbvh].copy_from_slice(&bvh.nodeid);
        }
        // Body AABBs are recomputed by set_compiled_const, as in the reference.
        bvh_adr += nbvh;
        let mut cntfree = 0;
        for j in &pb.joints { cntfree += (j.r#type == JNT_FREE) as usize; }
        if cntfree > 1 || (cntfree == 1 && pb.joints.len() > 1) { return Err("free joint can only appear by itself".into()); }
        if cntfree != 0 && pb.parent != 0 { return Err("free joint can only be used on top level".into()); }
        m.body_rootid[i] = if i == 0 || pb.parent == 0 { i as i32 } else { m.body_rootid[pb.parent] };
        lastdof[i] = if i > 0 { lastdof[pb.parent] } else { -1 };
        let parentid = pb.parent;
        m.body_simple[i] = (is_null_pose(&pb.ipos, Some(&pb.iquat)) &&
            (m.body_rootid[i] == i as i32 || (m.body_parentid[parentid] == 0 && m.body_dofnum[parentid] == 0))) as u8;
        if pb.simple == 0 { m.body_simple[i] = 0; }
        if parentid > 0 { m.body_simple[parentid] = 0; }
        let mut rotfound = false;
        for pj in &pb.joints {
            let jid = jntadr;
            m.jnt_type[jid] = pj.r#type;
            m.jnt_group[jid] = pj.group;
            m.jnt_plasticadr[jid] = if pj.springyield > 0.0 { plasticadr as i32 } else { -1 };
            if pj.springyield > 0.0 { plasticadr += joint_nq(pj); }
            m.jnt_limited[jid] = islimited(pj.limited, &pj.range) as u8;
            m.jnt_actfrclimited[jid] = islimited(pj.actfrclimited, &pj.actfrcrange) as u8;
            m.jnt_actgravcomp[jid] = pj.actgravcomp as u8;
            m.jnt_qposadr[jid] = qposadr as i32;
            m.jnt_dofadr[jid] = dofadr as i32;
            m.jnt_bodyid[jid] = i as i32;
            copy!(jnt_pos, jid, pj.pos);
            copy!(jnt_axis, jid, pj.axis);
            m.jnt_stiffness[jid] = pj.stiffness[0];
            copy!(jnt_stiffnesspoly, jid, pj.stiffness[1..]);
            m.jnt_springyield[jid] = pj.springyield;
            copy!(jnt_range, jid, pj.range);
            copy!(jnt_actfrcrange, jid, pj.actfrcrange);
            copy!(jnt_solref, jid, pj.solref_limit);
            copy!(jnt_solimp, jid, pj.solimp_limit);
            m.jnt_margin[jid] = pj.margin;
            let axis_aligned = (pj.axis[0].abs() > EPS) as u8 + (pj.axis[1].abs() > EPS) as u8 + (pj.axis[2].abs() > EPS) as u8 == 1;
            if rotfound || !is_null_pose(&pj.pos, None) || (matches!(pj.r#type, JNT_HINGE | JNT_SLIDE) && !axis_aligned) { m.body_simple[i] = 0; }
            if matches!(pj.r#type, JNT_BALL | JNT_HINGE) { rotfound = true; }
            match pj.r#type {
                JNT_FREE => {
                    m.qpos0[qposadr..qposadr+3].copy_from_slice(&pb.pos);
                    m.qpos0[qposadr+3..qposadr+7].copy_from_slice(&pb.quat);
                    m.qpos_spring[qposadr..qposadr+7].copy_from_slice(&m.qpos0[qposadr..qposadr+7]);
                }
                JNT_BALL => {
                    m.qpos0[qposadr..qposadr+4].copy_from_slice(&[1.0, 0.0, 0.0, 0.0]);
                    m.qpos_spring[qposadr..qposadr+4].copy_from_slice(&m.qpos0[qposadr..qposadr+4]);
                }
                JNT_SLIDE | JNT_HINGE => { m.qpos0[qposadr] = pj.r#ref; m.qpos_spring[qposadr] = pj.springref; }
                _ => return Err("unknown joint type".into()),
            }
            for _ in 0..joint_nv(pj) {
                m.dof_bodyid[dofadr] = i as i32;
                m.dof_jntid[dofadr] = jid as i32;
                copy!(dof_solref, dofadr, pj.solref_friction);
                copy!(dof_solimp, dofadr, pj.solimp_friction);
                m.dof_frictionloss[dofadr] = pj.frictionloss;
                m.dof_armature[dofadr] = pj.armature;
                m.dof_damping[dofadr] = pj.damping[0];
                copy!(dof_dampingpoly, dofadr, pj.damping[1..]);
                m.dof_parentid[dofadr] = lastdof[i];
                lastdof[i] = dofadr as i32;
                dofadr += 1;
            }
            jntadr += 1;
            qposadr += joint_nq(pj);
        }
        if m.body_simple[i] != 0 && m.body_dofnum[i] != 0 {
            m.body_simple[i] = 2;
            for j in &pb.joints { if j.r#type != JNT_SLIDE { m.body_simple[i] = 1; break; } }
        }
        for (j, pg) in pb.geoms.iter().enumerate() {
            let gid = geomadr;
            m.geom_type[gid] = pg.r#type;
            m.geom_contype[gid] = pg.contype;
            m.geom_conaffinity[gid] = pg.conaffinity;
            m.geom_condim[gid] = pg.condim;
            m.geom_bodyid[gid] = i as i32;
            m.geom_dataid[gid] = if tree.geom_mesh[i][j] >= 0 && pg.r#type == GEOM_MESH { tree.geom_mesh[i][j] } else { -1 };
            m.geom_matid[gid] = -1;
            m.geom_group[gid] = pg.group;
            m.geom_priority[gid] = pg.priority;
            m.geom_castshadow[gid] = pg.castshadow as u8;
            copy!(geom_size, gid, pg.size);
            copy!(geom_aabb, gid, tree.geom_aabb[i][j]);
            copy!(geom_pos, gid, pg.pos);
            copy!(geom_quat, gid, pg.quat);
            copy!(geom_friction, gid, pg.friction);
            m.geom_solmix[gid] = pg.solmix;
            copy!(geom_solref, gid, pg.solref);
            copy!(geom_solimp, gid, pg.solimp);
            m.geom_margin[gid] = pg.margin;
            m.geom_gap[gid] = pg.gap;
            copy!(geom_surfacevel, gid, pg.surfacevel);
            m.geom_adhesion[gid] = pg.adhesion;
            if pg.fluid_ellipsoid > 0.0 {
                m.geom_fluid[NFLUID*gid] = pg.fluid_ellipsoid;
                m.geom_fluid[NFLUID*gid+1..NFLUID*gid+6].copy_from_slice(&pg.fluid_coefs);
            }
            copy!(geom_rgba, gid, pg.rgba);
            // Bounding sphere of a mesh geom (GetRBound); zero for primitives, whose radius
            // set_compiled_const derives.
            m.geom_rbound[gid] = if pg.r#type == GEOM_MESH {
                let aamm = &meshes[tree.geom_mesh[i][j] as usize].aamm;
                let haabb = [max(aamm[0].abs(), aamm[3].abs()), max(aamm[1].abs(), aamm[4].abs()), max(aamm[2].abs(), aamm[5].abs())];
                (haabb[0] * haabb[0] + haabb[1] * haabb[1] + haabb[2] * haabb[2]).sqrt()
            } else { 0.0 };
            geomadr += 1;
        }
        for ps in &pb.sites {
            let sid = siteadr;
            m.site_type[sid] = ps.r#type;
            m.site_bodyid[sid] = i as i32;
            m.site_dataid[sid] = -1;
            m.site_matid[sid] = -1;
            m.site_group[sid] = ps.group;
            copy!(site_size, sid, ps.size);
            copy!(site_pos, sid, ps.pos);
            copy!(site_quat, sid, ps.quat);
            copy!(site_rgba, sid, ps.rgba);
            siteadr += 1;
        }
    }
    if m.nv != dofadr { return Err("unexpected number of DOFs".into()); }
    let mut ntree = 0;
    for i in 0..m.nv {
        if m.dof_parentid[i] == -1 { ntree += 1; }
        m.dof_treeid[i] = ntree-1;
    }
    if ntree as usize != m.ntree { return Err("unexpected number of TREEs".into()); }
    for i in 0..m.nbody {
        let weldid = m.body_weldid[i] as usize;
        m.body_treeid[i] = if m.body_dofnum[weldid] != 0 { m.dof_treeid[m.body_dofadr[weldid] as usize] } else { -1 };
    }
    m.tree_sleep_policy.fill(SLEEP_AUTO);
    for (i, pb) in tree.bodies.iter().enumerate().skip(1) {
        if pb.sleep != SLEEP_AUTO {
            let treeid = m.body_treeid[i];
            if treeid == -1 || treeid == m.body_treeid[i-1] { return Err("sleep policy only allowed for movable root bodies".into()); }
            m.tree_sleep_policy[treeid as usize] = pb.sleep;
        }
    }
    let mut n_m_post = 0;
    for i in 0..m.nv {
        m.dof_madr[i] = n_m_post as i32;
        let mut j = i as i32;
        while j >= 0 { n_m_post += 1; j = m.dof_parentid[j as usize]; }
    }
    if n_m_post != m.n_m { return Err("nM mismatch".into()); }
    if 2*m.n_m-m.nv != m.n_d { return Err("nD mismatch".into()); }
    // CopyPlugins: plugins are outside the subset; absent references are -1.
    m.body_plugin.fill(-1);
    m.geom_plugin.fill(-1);
    Ok(())
}

// mjCModel::CopyObjects, meshes and rigid constraints only.
fn copy_objects(m: &mut Model, meshes: &[Mesh], pairs: &[Pair], excludes: &[Exclude], equalities: &[Equality]) {
    // find bvh_adr after bodies
    let mut bvh_adr = 0;
    for i in 0..m.nbody { bvh_adr = bvh_adr.max(m.body_bvhadr[i] + m.body_bvhnum[i]); }

    // meshes
    let mut vert_adr = 0;
    let mut normal_adr = 0;
    let mut face_adr = 0;
    let mut graph_adr = 0;
    let mut poly_adr = 0;
    let mut polyvert_adr = 0;
    let mut polymap_adr = 0;
    for (i, pme) in meshes.iter().enumerate() {
        // set fields
        m.mesh_polyadr[i] = poly_adr as i32;
        m.mesh_polynum[i] = pme.npolygon() as i32;
        m.mesh_vertadr[i] = vert_adr as i32;
        m.mesh_vertnum[i] = pme.nvert() as i32;
        m.mesh_normaladr[i] = normal_adr as i32;
        m.mesh_normalnum[i] = pme.nnormal() as i32;
        // texcoord: no mesh has texture coordinates
        m.mesh_texcoordadr[i] = -1;
        m.mesh_texcoordnum[i] = 0;
        m.mesh_faceadr[i] = face_adr as i32;
        m.mesh_facenum[i] = pme.nface() as i32;
        m.mesh_graphadr[i] = if !pme.graph.is_empty() { graph_adr as i32 } else { -1 };
        m.mesh_bvhnum[i] = pme.tree.nodeid.len() as i32;
        m.mesh_bvhadr[i] = if !pme.tree.nodeid.is_empty() { bvh_adr } else { -1 };
        // octree: only for signed distance fields
        m.mesh_octnum[i] = 0;
        m.mesh_octadr[i] = -1;
        m.mesh_scale[3*i..3*i+3].copy_from_slice(&pme.scale);
        m.mesh_pos[3*i..3*i+3].copy_from_slice(&pme.pos);
        m.mesh_quat[4*i..4*i+4].copy_from_slice(&pme.quat);
        // mesh files: none, so the path address stays -1
        m.mesh_pathadr[i] = -1;

        // copy vertices, normals, faces, texcoords, aux data
        m.mesh_vert[3*vert_adr..3*vert_adr+pme.vert.len()].copy_from_slice(&pme.vert);
        m.mesh_normal[3*normal_adr..3*normal_adr+pme.normal.len()].copy_from_slice(&pme.normal);
        m.mesh_face[3*face_adr..3*face_adr+pme.face.len()].copy_from_slice(&pme.face);
        m.mesh_facenormal[3*face_adr..3*face_adr+pme.facenormal.len()].copy_from_slice(&pme.facenormal);
        m.mesh_facetexcoord[3*face_adr..3*face_adr+3*pme.nface()].fill(0);
        m.mesh_extrema[27*i..27*i+27].fill(0);
        if !pme.graph.is_empty() {
            m.mesh_graph[graph_adr..graph_adr+pme.graph.len()].copy_from_slice(&pme.graph);

            // compute grid extrema (local indices in graph)
            let mut max_val = [f32::MIN; 27];

            let graph = &pme.graph;
            let numgraphvert = graph[0] as usize;
            let vert_globalid = 2 + numgraphvert;
            let verts = &pme.vert;

            // map the 27 features (8 vertices, 6 faces, 12 edges) of a unit cube to the farthest
            // vertex in the mesh
            for local_id in 0..numgraphvert {
                let global_id = graph[vert_globalid + local_id] as usize;

                let x = verts[3 * global_id];
                let y = verts[3 * global_id + 1];
                let z = verts[3 * global_id + 2];

                let mut k = 0;
                for cx in -1..=1 {
                    for cy in -1..=1 {
                        for cz in -1..=1 {
                            let dot: f32 = x * cx as f32 + y * cy as f32 + z * cz as f32;
                            if dot > max_val[k] {
                                max_val[k] = dot;
                                m.mesh_extrema[27 * i + k] = local_id as i32;
                            }
                            k += 1;
                        }
                    }
                }
            }
        }

        // CopyPolygonNormals
        m.mesh_polynormal[3*poly_adr..3*poly_adr+pme.polygon_normals.len()].copy_from_slice(&pme.polygon_normals);
        // CopyPolygons
        let mut count = 0;
        for (p, polygon) in pme.polygons.iter().enumerate() {
            m.mesh_polyvertnum[poly_adr + p] = polygon.len() as i32;
            m.mesh_polyvertadr[poly_adr + p] = (polyvert_adr + count) as i32;
            for (j, &v) in polygon.iter().enumerate() { m.mesh_polyvert[polyvert_adr + count + j] = v; }
            count += polygon.len();
        }
        // CopyPolygonMap
        let mut count = 0;
        for (p, faces) in pme.polygon_map.iter().enumerate() {
            m.mesh_polymapnum[vert_adr + p] = faces.len() as i32;
            m.mesh_polymapadr[vert_adr + p] = (polymap_adr + count) as i32;
            for (j, &f) in faces.iter().enumerate() { m.mesh_polymap[polymap_adr + count + j] = f; }
            count += faces.len();
        }

        // copy bvh data
        let nbvh = pme.tree.nodeid.len();
        if nbvh != 0 {
            let adr = bvh_adr as usize;
            m.bvh_aabb[6*adr..6*(adr+nbvh)].copy_from_slice(&pme.tree.bvh);
            m.bvh_child[2*adr..2*(adr+nbvh)].copy_from_slice(&pme.tree.child);
            m.bvh_depth[adr..adr+nbvh].copy_from_slice(&pme.tree.level);
            for j in 0..nbvh { m.bvh_nodeid[j + adr] = if pme.tree.nodeid[j] > -1 { pme.tree.nodeid[j] } else { -1 }; }
        }

        // advance counters
        poly_adr += pme.npolygon();
        polyvert_adr += pme.npolygonvert();
        polymap_adr += pme.npolygonmap();
        vert_adr += pme.nvert();
        normal_adr += pme.nnormal();
        face_adr += pme.nface();
        graph_adr += pme.graph.len();
        bvh_adr += nbvh as i32;
    }

    m.nemax = 0;
    for e in equalities { m.nemax += match e.spec.r#type { EQ_CONNECT => 3, EQ_WELD => 7, _ => 1 }; }
    m.njmax = usize::MAX;
    m.nconmax = usize::MAX;
    for (i, pair) in pairs.iter().enumerate() {
        let p = &pair.spec;
        m.pair_dim[i] = p.condim;
        m.pair_geom1[i] = pair.geom1 as i32;
        m.pair_geom2[i] = pair.geom2 as i32;
        m.pair_signature[i] = pair.signature;
        m.pair_solref[NREF*i..NREF*(i+1)].copy_from_slice(&p.solref);
        m.pair_solreffriction[NREF*i..NREF*(i+1)].copy_from_slice(&p.solreffriction);
        m.pair_solimp[NIMP*i..NIMP*(i+1)].copy_from_slice(&p.solimp);
        m.pair_margin[i] = p.margin;
        m.pair_gap[i] = p.gap;
        m.pair_adhesion[i] = p.adhesion;
        m.pair_friction[5*i..5*(i+1)].copy_from_slice(&p.friction);
    }
    for (i, exclude) in excludes.iter().enumerate() { m.exclude_signature[i] = exclude.signature; }
    for (i, eq) in equalities.iter().enumerate() {
        let e = &eq.spec;
        m.eq_type[i] = e.r#type;
        m.eq_obj1id[i] = eq.obj1id;
        m.eq_obj2id[i] = eq.obj2id;
        m.eq_objtype[i] = e.objtype;
        m.eq_active0[i] = e.active as u8;
        m.eq_solref[NREF*i..NREF*(i+1)].copy_from_slice(&e.solref);
        m.eq_solimp[NIMP*i..NIMP*(i+1)].copy_from_slice(&e.solimp);
        m.eq_data[NEQDATA*i..NEQDATA*(i+1)].copy_from_slice(&e.data);
    }
}

// mjCModel::FinalizeSimple; tendon demotion is outside the subset.
fn finalize_simple(m: &mut Model) -> Result<(), String> {
    let mut count = 0;
    for i in (0..m.nv).rev() {
        if m.body_simple[m.dof_bodyid[i] as usize] != 0 { count += 1; } else { count = 0; }
        m.dof_simplenum[i] = count;
    }
    let mut n_od = 0;
    for i in 0..m.nv {
        if m.dof_simplenum[i] == 0 {
            let mut j = i as i32;
            while j >= 0 { if j != i as i32 { n_od += 1; } j = m.dof_parentid[j as usize]; }
        }
    }
    if n_od+m.nv != m.n_c { return Err("nC mismatch".into()); }
    Ok(())
}

// mjCModel::AutoSpringDamper.
fn auto_spring_damper(m: &mut Model, tree: &Tree) {
    let mut n = 0;
    for b in &tree.bodies {
        for j in &b.joints {
            let adr = m.jnt_dofadr[n] as usize;
            let ndim = joint_nv(j);
            let timeconst = j.springdamper[0];
            let dampratio = j.springdamper[1];
            if !(timeconst <= 0.0 || dampratio <= 0.0) {
                let mut inertia = 0.0;
                for i in 0..ndim { inertia += m.dof_invweight0[adr+i]; }
                inertia = ndim as f64 / max(MINVAL, inertia);
                let stiffness = inertia / max(MINVAL, timeconst*timeconst*dampratio*dampratio);
                let damping = 2.0 * inertia / max(MINVAL, timeconst);
                m.jnt_stiffness[n] = stiffness;
                for i in 0..ndim { m.dof_damping[adr+i] = damping; }
            }
            n += 1;
        }
    }
}

// mjCModel::CheckBodyMassInertia. Explicit traversal preserves child order.
fn check_body_mass_inertia(tree: &Tree, root: usize) -> bool {
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let b = &tree.bodies[id];
        if b.mass >= MINVAL && b.inertia[0] >= MINVAL && b.inertia[1] >= MINVAL && b.inertia[2] >= MINVAL { return true; }
        for &child in tree.children[id].iter().rev() {
            if tree.bodies[child].joints.is_empty() { stack.push(child); }
        }
    }
    false
}

// PrintIndent, PrintTree and mjCModel::Signature. The explicit stack avoids
// recursion while retaining the reference's depth check and indentation cap.
fn signature(tree: &Tree, spec: &ModelSpec) -> Result<u64, String> {
    let mut text = String::from("\n");
    let mut stack = vec![(0, 0, false)];
    while let Some((id, depth, close)) = stack.pop() {
        if depth == 1024 { return Err("depth limit exceeded in signature computation".into()); }
        text.push_str(&" ".repeat((depth*2).min(1024)));
        if close { text.push_str("</body>\n"); continue; }
        text.push_str("<body>\n");
        let b = &tree.bodies[id];
        let indent = " ".repeat(((depth+1)*2).min(1024));
        for j in &b.joints {
            text.push_str(&indent);
            text.push_str(&format!("<joint>{}</joint>\n", joint_nq(j)));
        }
        for _ in &b.geoms { text.push_str(&indent); text.push_str("<geom/>\n"); }
        for _ in &b.sites { text.push_str(&indent); text.push_str("<site/>\n"); }
        stack.push((id, depth, true));
        for &child in tree.children[id].iter().rev() { stack.push((child, depth+1, false)); }
    }
    for _ in &spec.meshes { text.push_str("<mesh/>\n"); }
    for _ in &spec.pairs { text.push_str("<pair/>\n"); }
    for _ in &spec.excludes { text.push_str("<exclude/>\n"); }
    for _ in &spec.equalities { text.push_str("<equality/>\n"); }
    Ok(crate::name::hash_string(&text, u64::MAX))
}

/// mjCModel::Compile / TryCompile, for the explicit rigid-body subset.
/// Compilation owns its working copies; the input specification is unchanged.
// mjCModel::LengthRange. Its threaded branch gives each thread a run of actuators and its own
// mjData; mj_setLengthRange resets the data for every computation, so the ranges are the ones this
// loop computes, and the actuator count that chooses between the branches is left out with it.
fn length_range(m: &mut Model, d: &mut crate::data::Data, lropt: &crate::model::LROpt) -> Result<(), String> {
    let saveopt = m.opt.clone();
    m.opt.disableflags = DSBL_FRICTIONLOSS | DSBL_CONTACT | DSBL_SPRING | DSBL_DAMPER | DSBL_GRAVITY | DSBL_ACTUATION;
    if lropt.timestep > 0.0 { m.opt.timestep = lropt.timestep; }
    let mut err = [0u8; 200];
    for i in 0..m.nactuator {
        if crate::setconst::set_length_range(m, d, i as i32, lropt, &mut err, 200) == 0 {
            let len = err.iter().position(|&c| c == 0).unwrap_or(err.len());
            return Err(String::from_utf8_lossy(&err[..len]).into_owned());
        }
    }
    m.opt = saveopt;
    Ok(())
}

pub fn compile(spec: &ModelSpec) -> Result<Model, String> {
    // fusestatic: outside the subset; FuseStatic and its private helpers omitted.
    // discardvisual: outside the subset; DiscardVisual and its helpers omitted.
    if spec.compiler.fusestatic || spec.compiler.discardvisual || spec.compiler.alignfree {
        return Err("fusestatic, discardvisual and alignfree must be false".into());
    }
    // default classes: outside the subset.
    // user data: outside the subset; SetNuser resolves empty auto fields to zero.
    if spec.nuserdata != 0 || [spec.nuser_body, spec.nuser_jnt, spec.nuser_geom, spec.nuser_site].iter().any(|&n| n > 0 || n < -1)
        || spec.bodies.iter().any(|b| !b.userdata.is_empty() || b.joints.iter().any(|j| !j.userdata.is_empty()) || b.geoms.iter().any(|g| !g.userdata.is_empty()) || b.sites.iter().any(|s| !s.userdata.is_empty())) {
        return Err("user data is outside the builder subset".into());
    }
    if spec.memory < -1 { return Err("memory must be -1 or nonnegative".into()); }
    // CheckEmptyNames: a mesh needs a name to be referenced (only the XML parser names one after its file).
    for mesh in &spec.meshes {
        if mesh.name.as_deref().unwrap_or("").is_empty() { return Err("empty name in mesh".into()); }
    }
    let mut tree = object_lists(spec)?;
    index_assets(&mut tree)?;
    // hfields: outside the subset.
    // textures: outside the subset.
    // CompileTree compiles pairs once before the tree for mesh convex hull checks.
    for p in &spec.pairs { compile_pair(p, &tree)?; }
    // SetNeedHull: a mesh geom with a collision type needs the hull; a vertex-only mesh always
    // has it made, whether or not a geom uses it.
    // CompileMeshesAndTextures, serial branch: the meshes are compiled in order.
    let mut meshes = Vec::new();
    for mesh in &spec.meshes { meshes.push(Mesh::compile(mesh)?); }
    let mut geomadr = 0;
    for i in 0..tree.bodies.len() {
        compile_body(&mut tree, i, &spec.compiler, geomadr, &meshes)?;
        geomadr += tree.bodies[i].geoms.len();
    }
    // flexes: outside the subset.
    // skins: outside the subset.
    // materials: outside the subset.
    let mut pairs = Vec::new();
    for p in &spec.pairs { pairs.push(compile_pair(p, &tree)?); }
    let mut excludes = Vec::new();
    for e in &spec.excludes { excludes.push(compile_exclude(e, &tree)?); }
    // tendons: outside the subset.
    let mut equalities = Vec::new();
    for e in &spec.equalities { equalities.push(compile_equality(e, &tree)?); }
    // actuators: outside the subset; CountNJmom and LengthRange workers omitted.
    // sensors: outside the subset.
    // numeric/text/tuple: outside the subset.
    // plugins: outside the subset.
    pairs.sort_by_key(|p| p.signature as u32);
    excludes.sort_by_key(|e| e.signature as u32);

    // SetSizes and SaveDofOffsets: joint addresses are copied in the same order below.
    let mut sizes = vec![0; MODEL_SIZES.len()];
    let nbody = tree.bodies.len();
    let mut nq = 0;
    let mut nv = 0;
    let mut njnt = 0;
    let mut ngeom = 0;
    let mut nsite = 0;
    let mut ntree = 0;
    let mut nmocap = 0;
    let mut nplastic = 0;
    let modelname = spec.modelname.as_deref().unwrap_or("model");
    let mut nnames = modelname.len()+1;
    for (i, b) in tree.bodies.iter().enumerate() {
        nnames += b.name.as_deref().unwrap_or("").len()+1;
        for (k, j) in b.joints.iter().enumerate() {
            nq += joint_nq(j);
            nv += joint_nv(j);
            if k == 0 {
                let mut static_ancestry = true;
                let mut ancestor = i;
                while ancestor != 0 {
                    ancestor = tree.bodies[ancestor].parent;
                    if !tree.bodies[ancestor].joints.is_empty() { static_ancestry = false; break; }
                }
                if static_ancestry { ntree += 1; }
            }
            if j.springyield > 0.0 { nplastic += joint_nq(j); }
            nnames += j.name.as_deref().unwrap_or("").len()+1;
        }
        njnt += b.joints.len();
        ngeom += b.geoms.len();
        nsite += b.sites.len();
        for g in &b.geoms { nnames += g.name.as_deref().unwrap_or("").len()+1; }
        for s in &b.sites { nnames += s.name.as_deref().unwrap_or("").len()+1; }
        if b.mocap { nmocap += 1; }
    }
    for mesh in &meshes { nnames += mesh.name().len()+1; }
    for p in &pairs { nnames += p.spec.name.as_deref().unwrap_or("").len()+1; }
    for e in &excludes { nnames += e.spec.name.as_deref().unwrap_or("").len()+1; }
    for e in &equalities { nnames += e.spec.name.as_deref().unwrap_or("").len()+1; }
    for (name, value) in [("nbody", nbody), ("nq", nq), ("nv", nv), ("njnt", njnt), ("ngeom", ngeom), ("nsite", nsite), ("ntree", ntree), ("nmocap", nmocap), ("nplastic", nplastic), ("npair", pairs.len()), ("nexclude", excludes.len()), ("neq", equalities.len()), ("nnames", nnames), ("npaths", 1),
        ("nmesh", meshes.len()),
        ("nmeshvert", meshes.iter().map(|mesh| mesh.nvert()).sum::<usize>()),
        ("nmeshnormal", meshes.iter().map(|mesh| mesh.nnormal()).sum::<usize>()),
        ("nmeshface", meshes.iter().map(|mesh| mesh.nface()).sum::<usize>()),
        ("nmeshgraph", meshes.iter().map(|mesh| mesh.graph.len()).sum::<usize>()),
        ("nmeshpoly", meshes.iter().map(|mesh| mesh.npolygon()).sum::<usize>()),
        ("nmeshpolyvert", meshes.iter().map(|mesh| mesh.npolygonvert()).sum::<usize>()),
        ("nmeshpolymap", meshes.iter().map(|mesh| mesh.npolygonmap()).sum::<usize>())] {
        set_size(&mut sizes, name, value);
    }
    compute_sparse_sizes(&tree, nv, &mut sizes);
    let mut nbvhstatic = 0;
    for bvh in &tree.body_bvh { nbvhstatic += bvh.nodeid.len(); }
    for mesh in &meshes { nbvhstatic += mesh.tree.nodeid.len(); }
    set_size(&mut sizes, "nbvhstatic", nbvhstatic);
    set_size(&mut sizes, "nbvh", nbvhstatic);
    for i in 1..nbody {
        if !tree.bodies[i].joints.is_empty() && !check_body_mass_inertia(&tree, i) { return Err("mass and inertia of moving bodies must be larger than mjMINVAL".into()); }
    }
    let mut m = make_model(&sizes)?;
    m.opt = spec.option.clone();
    copy_names(&mut m, modelname, &tree, &meshes, &pairs, &excludes, &equalities);
    // CopyPaths: no assets; the one-byte empty paths buffer is already zero.
    copy_tree(&mut m, &tree, &meshes)?;
    // keyframes: outside the subset; ResolveKeyframes and quaternion normalization omitted.
    copy_objects(&mut m, &meshes, &pairs, &excludes, &equalities);
    finalize_simple(&mut m)?;
    if spec.compiler.settotalmass > 0.0 { crate::support::set_totalmass(&mut m, spec.compiler.settotalmass); }
    if spec.memory != -1 { m.narena = spec.memory as usize; }
    else {
        let nconmax = 100;
        let njmax = 500;
        let ndense = njmax + m.neq + m.nv;
        let nlinear = m.nq + m.nv + m.nu + m.na + m.nbody + m.njnt + m.ngeom + m.nsite + m.neq + m.ntendon + m.nwrap;
        m.narena = 8 * 1000_usize.max(5*ndense*ndense + 20*nlinear);
        // sizeof(mjContact): 69 doubles, 13 ints, rounded to double alignment.
        // Do not depend on Rust's unspecified Contact field layout.
        let contact_bytes = (69*8 + 13*4 + 7) & !7;
        let arena_bytes = nconmax*contact_bytes + njmax*(8*4 + 14*8) + m.nv*(3*4)
            + njmax*m.nv*(2*4 + 2*8) + njmax*njmax*(4 + 8);
        m.narena += arena_bytes;
        let megabyte = 1 << 20;
        let nstack_mb = m.narena / megabyte;
        let residual_mb = (m.narena % megabyte != 0) as usize;
        m.narena = megabyte * (nstack_mb + residual_mb);
    }
    {
        let mut scratch = vec![0; m.nv];
        let mut count = vec![0; m.nbody];
        let mut m_mat = vec![0; m.n_m]; // C's M scratch, distinct from the model m.
        crate::io::make_dof_dof_sparse(m.nv, m.n_c, m.n_d, m.n_m, &m.dof_parentid, &m.dof_simplenum,
            &mut m.d_rownnz, &mut m.d_rowadr, Some(&mut m.d_diag), &mut m.d_colind, false, true, &mut scratch);
        crate::io::make_b_sparse(m.nv, m.nbody, m.n_b, &m.body_dofnum, &m.body_parentid, &m.body_dofadr,
            &mut m.b_rownnz, &mut m.b_rowadr, &mut m.b_colind, &mut count);
        crate::io::make_dof_dof_sparse(m.nv, m.n_c, m.n_d, m.n_m, &m.dof_parentid, &m.dof_simplenum,
            &mut m.m_rownnz, &mut m.m_rowadr, None, &mut m.m_colind, true, false, &mut scratch);
        crate::io::make_dof_dof_maps(m.nv, m.n_m, m.n_c, m.n_d, &m.dof_madr, &m.dof_simplenum, &m.dof_parentid,
            &m.d_rownnz, &m.d_rowadr, &m.d_colind, &m.m_rownnz, &m.m_rowadr, &m.m_colind,
            &mut m.map_m2d, &mut m.map_d2m, &mut m.map_m2m, &mut m_mat, &mut scratch);
    }
    check_supported(&m)?;
    let disableflags = m.opt.disableflags;
    m.opt.disableflags |= DSBL_CONTACT;
    let enableflags = m.opt.enableflags;
    m.opt.enableflags &= !ENBL_SLEEP;
    // With no plugins, make_data is exactly makeRawData followed by resetData.
    let mut d = crate::io::make_data(&m);
    crate::setconst::set_compiled_const(&mut m, &mut d);
    crate::io::reset_plastic(&m, &mut d);
    auto_spring_damper(&mut m, &tree);
    // compiler.LRopt: the spec has no length-range options, so mj_defaultLROpt
    let mut lropt = crate::model::LROpt::default();
    crate::model::default_lropt(&mut lropt);
    length_range(&mut m, &mut d, &lropt)?;
    if !spec.stat.extent.is_nan() { m.stat.extent = spec.stat.extent; }
    if !spec.stat.meaninertia.is_nan() { m.stat.meaninertia = spec.stat.meaninertia; }
    if !spec.stat.meanmass.is_nan() { m.stat.meanmass = spec.stat.meanmass; }
    if !spec.stat.meansize.is_nan() { m.stat.meansize = spec.stat.meansize; }
    if !spec.stat.center[0].is_nan() { m.stat.center = spec.stat.center; }
    crate::io::validate_references(&m)?;
    drop(d);
    let mut asleep_init = false;
    if enableflags & ENBL_SLEEP != 0 {
        for i in 0..m.ntree { if m.tree_sleep_policy[i] == SLEEP_INIT { asleep_init = true; break; } }
    }
    if asleep_init { m.opt.disableflags = disableflags; m.opt.enableflags = enableflags; }
    let mut d = crate::io::make_data(&m);
    // test forward simulation unless asleep_init is true
    if !asleep_init { crate::forward::step(&m, &mut d); }
    drop(d);
    m.opt.disableflags = disableflags;
    m.opt.enableflags = enableflags;
    m.signature = signature(&tree, spec)?;
    Ok(m)
}
