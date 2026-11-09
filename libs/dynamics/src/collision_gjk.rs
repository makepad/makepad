//! Native convex distance, penetration, polytopes and contact manifolds.
//!
//! Translated from `src/engine/engine_collision_gjk.c` and its header of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2024 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::inline::{at3, at3_mut};
use crate::types::*;
use crate::util_blas::{mul_mat_vec3, normalize3};
use std::cell::Cell;

pub const MAX_LIMIT: f64 = f64::MAX;
pub const FACE_TOL: f64 = 0.996;
pub const EDGE_TOL: f64 = 0.0888;
pub const EPA_NOCONTACT: i32 = -1;
pub const EPA_SUCCESS: i32 = 0;
pub const EPA_P2_INVALID_FACES: i32 = 1;
pub const EPA_P2_NONCONVEX: i32 = 2;
pub const EPA_P2_ORIGIN_ON_FACE: i32 = 3;
pub const EPA_P3_BAD_NORMAL: i32 = 4;
pub const EPA_P3_INVALID_V4: i32 = 5;
pub const EPA_P3_INVALID_V5: i32 = 6;
pub const EPA_P3_MISSING_ORIGIN: i32 = 7;
pub const EPA_P3_ORIGIN_ON_FACE: i32 = 8;
pub const EPA_P4_MISSING_ORIGIN: i32 = 9;
const MINVAL2: f64 = MINVAL * MINVAL;
const MINDIST2: f64 = MINVAL2;
const MINDIST3: f64 = MINVAL2;
const MINDIST4: f64 = MINVAL2;
const MINEPATOL: f64 = MINVAL;
const MINEPATOL2: f64 = MINEPATOL * MINEPATOL;

#[derive(Default)]
pub struct CcdMesh<'a> {
    pub nvert: usize,
    pub mesh_polynum: usize,
    pub vert: &'a [f32],
    pub mpolymapadr: &'a [i32],
    pub mpolymapnum: &'a [i32],
    pub polymap: &'a [i32],
    pub polyvertadr: &'a [i32],
    pub polyvertnum: &'a [i32],
    pub polyvert: &'a [i32],
    pub polynormal: &'a [f64],
    pub graph: &'a [i32],
    pub extrema: &'a [i32],
}
#[derive(Default)]
pub struct CcdHfield<'a> {
    pub prism: [[f64; 3]; 6],
    pub hfield_data: &'a [f32],
    pub hfield_nrow: usize,
    pub hfield_ncol: usize,
}
#[derive(Default)]
pub struct CcdFlex<'a> {
    pub elem: &'a [i32],
    pub dim: &'a [i32],
    pub aabb: &'a [f64],
    pub elemadr: &'a [i32],
    pub elemdataadr: &'a [i32],
    pub vert_xpos: &'a [f64],
    pub vertadr: &'a [i32],
    pub xradius: &'a [f64],
}
// The C union is represented by separate borrowed views; it owns no model data.
#[derive(Default)]
pub struct CcdObjData<'a> {
    pub mesh: CcdMesh<'a>,
    pub hfield: CcdHfield<'a>,
    pub flex: CcdFlex<'a>,
}
/// A CCD object's support function: one variant per support function of
/// engine_collision_convex.c, `Null` for its NULL. C stores a function pointer and compares it
/// (discreteGeom, mjc_penetration); Rust gives a function no single address (codegen units can
/// hold copies), so the object holds this tag and `CcdSupport::call` dispatches.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CcdSupport { Null, Point, Sphere, Line, Capsule, Ellipsoid, Cylinder, Box, Mesh, Hillclimb, Prism, Flex }
pub struct CcdObj<'a> {
    pub geom: i32,
    pub geom_type: i32,
    pub size: [f64; 4],
    pub pos: [f64; 3],
    pub mat: [f64; 9],
    // Supports cache these indices through the shared callback argument.
    pub vertindex: Cell<i32>,
    pub meshindex: Cell<i32>,
    pub flex: i32,
    pub elem: i32,
    pub vert: i32,
    pub margin: f64,
    pub rotate: [f64; 4],
    pub suppvert: [f64; 12],
    pub suppind: [i32; 4],
    pub nsupp: usize,
    pub data: CcdObjData<'a>,
    pub center: fn(&mut [f64; 3], &CcdObj<'_>),
    pub support: CcdSupport,
}
impl Default for CcdObj<'_> {
    fn default() -> Self {
        Self {
            geom: -1, geom_type: GEOM_NONE, size: [0.0; 4], pos: [0.0; 3],
            mat: [0.0; 9], vertindex: Cell::new(-1), meshindex: Cell::new(-1),
            flex: -1, elem: -1, vert: -1, margin: 0.0, rotate: [0.0; 4],
            suppvert: [0.0; 12], suppind: [0; 4], nsupp: 0,
            data: CcdObjData::default(), center: crate::collision_convex::center,
            support: CcdSupport::Null,
        }
    }
}
pub struct CcdConfig<'a> {
    pub max_iterations: usize,
    pub tolerance: f64,
    pub max_contacts: usize,
    pub dist_cutoff: f64,
    pub buffer: Option<&'a mut CcdBuffer>,
    pub npolygonmax: usize,
    pub nmeshdegmax: usize,
}
pub struct CcdStatus {
    pub separated: bool,
    pub dist: [f64; MAXCONPAIR],
    pub x1: [f64; 3 * MAXCONPAIR],
    pub x2: [f64; 3 * MAXCONPAIR],
    pub nx: usize,
    pub max_iterations: usize,
    pub tolerance: f64,
    pub max_contacts: usize,
    pub dist_cutoff: f64,
    pub gjk_iterations: usize,
    pub epa_iterations: usize,
    pub epa_status: i32,
}
impl Default for CcdStatus {
    fn default() -> Self {
        Self {
            separated: false,
            dist: [0.0; MAXCONPAIR],
            x1: [0.0; 3 * MAXCONPAIR],
            x2: [0.0; 3 * MAXCONPAIR],
            nx: 0,
            max_iterations: 0,
            tolerance: 0.0,
            max_contacts: 0,
            dist_cutoff: 0.0,
            gjk_iterations: 0,
            epa_iterations: 0,
            epa_status: 0,
        }
    }
}
// Typed, reusable storage replaces the caller's untyped C byte buffer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CcdBuffer {
    polytope: Polytope,
    idx1: Vec<i32>,
    idx2: Vec<i32>,
    n1: Vec<f64>,
    n2: Vec<f64>,
    edge: Vec<f64>,
    endverts: Vec<f64>,
    face1: Vec<f64>,
    face2: Vec<f64>,
    polygon: Vec<f64>,
}
// align memory size on 8-byte boundary; needed for single precision
fn align8(size: usize) -> usize {
    ((size + 7) / 8) * 8
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
struct Vertex {
    vert: [f64; 3],
    vert1: [f64; 3],
    vert2: [f64; 3],
    index1: i32,
    index2: i32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
struct Face {
    verts: i32,
    adj: [i32; 3],
    v: [f64; 3],
    dist2: f64,
    index: i32,
}
fn epa_vert_expand(n: i32) -> [usize; 3] {
    [
        (n & 0x3ff) as usize,
        ((n >> 10) & 0x3ff) as usize,
        ((n >> 20) & 0x3ff) as usize,
    ]
}
#[derive(Clone, Debug, Default, PartialEq)]
struct Polytope {
    verts: Vec<Vertex>,
    nverts: usize,
    faces: Vec<Face>,
    nfaces: usize,
    maxfaces: usize,
    center: [f64; 3],
    map: Vec<usize>,
    nmap: usize,
    horizon: Horizon,
}
#[derive(Clone, Debug, Default, PartialEq)]
struct Horizon {
    indices: Vec<usize>,
    edges: Vec<usize>,
    nedges: usize,
    maxedges: usize,
    w: [f64; 3],
}

// res = v1 + v2
fn add3(res: &mut [f64; 3], v1: &[f64; 3], v2: &[f64; 3]) {
    res[0] = v1[0] + v2[0];
    res[1] = v1[1] + v2[1];
    res[2] = v1[2] + v2[2];
}
// res = v1 - v2
fn sub3(res: &mut [f64; 3], v1: &[f64; 3], v2: &[f64; 3]) {
    res[0] = v1[0] - v2[0];
    res[1] = v1[1] - v2[1];
    res[2] = v1[2] - v2[2];
}
// dot product
fn dot3(v1: &[f64; 3], v2: &[f64; 3]) -> f64 {
    v1[0] * v2[0] + v1[1] * v2[1] + v1[2] * v2[2]
}
// norm of v
fn norm3(v: &[f64; 3]) -> f64 {
    dot3(v, v).sqrt()
}
// res = v
fn copy3(res: &mut [f64; 3], v: &[f64; 3]) {
    res[0] = v[0];
    res[1] = v[1];
    res[2] = v[2];
}
// scalar product: res = s*v
fn scl3(res: &mut [f64; 3], v: &[f64; 3], s: f64) {
    res[0] = s * v[0];
    res[1] = s * v[1];
    res[2] = s * v[2];
}
// res = v1 + s*v2
fn add_scl3(res: &mut [f64; 3], v1: &[f64; 3], v2: &[f64; 3], s: f64) {
    res[0] = v1[0] + s * v2[0];
    res[1] = v1[1] + s * v2[1];
    res[2] = v1[2] + s * v2[2];
}
// cross product: res = v1 x v2
fn cross3(res: &mut [f64; 3], v1: &[f64; 3], v2: &[f64; 3]) {
    res[0] = v1[1] * v2[2] - v1[2] * v2[1];
    res[1] = v1[2] * v2[0] - v1[0] * v2[2];
    res[2] = v1[0] * v2[1] - v1[1] * v2[0];
}
// return determinant of the 3x3 matrix with columns v1, v2, v3
fn det3(v1: &[f64; 3], v2: &[f64; 3], v3: &[f64; 3]) -> f64 {
    v1[0] * (v2[1] * v3[2] - v2[2] * v3[1])
        + v1[1] * (v2[2] * v3[0] - v2[0] * v3[2])
        + v1[2] * (v2[0] * v3[1] - v2[1] * v3[0])
}
// return true if object is a discrete shape or shrunk to a point/line with no margin
fn discrete_geom(obj: &CcdObj<'_>) -> bool {
    if obj.margin != 0.0 {
        return false;
    }
    if obj.support == CcdSupport::Point || obj.support == CcdSupport::Line {
        return true;
    }
    let g = obj.geom_type;
    g == GEOM_MESH || g == GEOM_BOX || g == GEOM_HFIELD
}
// C's file-local gjk and exported mjc_gjk collide after prefix removal.
// GJK algorithm
fn gjk_internal(status: &mut CcdStatus, obj1: &mut CcdObj<'_>, obj2: &mut CcdObj<'_>) {
    let get_dist = status.dist_cutoff > 0.0;
    let mut simplex = [0.0; 12];
    let mut n = 0;
    let mut k = 0;
    let kmax = status.max_iterations;
    let mut x_k = [0.0; 3];
    let mut lambda = [0.0; 4];
    let tol2 = status.tolerance * status.tolerance;
    status.separated = false;
    obj1.nsupp = 0;
    obj2.nsupp = 0;
    let discrete = discrete_geom(obj1) && discrete_geom(obj2);
    let epsilon = if discrete { 0.0 } else { 0.5 * tol2 };
    let min_norm = if discrete || get_dist {
        MINVAL
    } else {
        status.tolerance
    };
    sub3(&mut x_k, at3(&status.x1, 0), at3(&status.x2, 0));
    let mut x_norm = norm3(&x_k);
    if x_norm < MINVAL {
        x_k = [1.0, 0.0, 0.0];
        x_norm = 1.0;
    }
    while k < kmax {
        let mut v = Vertex::default();
        gjk_support(&mut v, obj1, obj2, &x_k, x_norm);
        copy3(at3_mut(&mut simplex, 3 * n), &v.vert);
        copy3(at3_mut(&mut obj1.suppvert, 3 * n), &v.vert1);
        copy3(at3_mut(&mut obj2.suppvert, 3 * n), &v.vert2);
        obj1.suppind[n] = v.index1;
        obj2.suppind[n] = v.index2;
        let lower = dot3(&x_k, at3(&simplex, 3 * n));
        if !get_dist {
            if lower > 0.0 {
                status.separated = true;
                status.gjk_iterations = k;
                status.nx = 0;
                status.dist[0] = MAX_LIMIT;
                return;
            }
        } else if lower > 0.0 {
            status.separated = true;
            if status.dist_cutoff < MAX_LIMIT && lower >= status.dist_cutoff * x_norm {
                status.gjk_iterations = k;
                status.nx = 0;
                status.dist[0] = MAX_LIMIT;
                return;
            }
        }
        if k > 0 {
            let s_k = at3(&simplex, 3 * n);
            let diff = [x_k[0] - s_k[0], x_k[1] - s_k[1], x_k[2] - s_k[2]];
            if dot3(&x_k, &diff) < epsilon {
                break;
            }
        }
        subdistance(&mut lambda, n + 1, &simplex);
        n = 0;
        for i in 0..4 {
            if lambda[i] == 0.0 {
                continue;
            }
            if n != i {
                let a = *at3(&simplex, 3 * i);
                copy3(at3_mut(&mut simplex, 3 * n), &a);
                let a = *at3(&obj1.suppvert, 3 * i);
                copy3(at3_mut(&mut obj1.suppvert, 3 * n), &a);
                let a = *at3(&obj2.suppvert, 3 * i);
                copy3(at3_mut(&mut obj2.suppvert, 3 * n), &a);
                obj1.suppind[n] = obj1.suppind[i];
                obj2.suppind[n] = obj2.suppind[i];
            }
            lambda[n] = lambda[i];
            n += 1;
        }
        if n < 1 {
            status.gjk_iterations = k;
            status.nx = 0;
            status.dist[0] = MAX_LIMIT;
            status.separated = true;
            return;
        }
        if n == 4 {
            if !status.separated {
                x_norm = 0.0;
            }
            break;
        }
        let mut x_next = [0.0; 3];
        let mut fallback = 1;
        if n == 3 {
            fallback = project_origin_plane(
                &mut x_next,
                at3(&simplex, 0),
                at3(&simplex, 3),
                at3(&simplex, 6),
            );
        } else if n == 2 {
            fallback = project_origin_line(&mut x_next, at3(&simplex, 0), at3(&simplex, 3));
        }
        if fallback != 0 {
            lincomb(
                &mut x_next,
                &lambda,
                n,
                at3(&simplex, 0),
                at3(&simplex, 3),
                at3(&simplex, 6),
                at3(&simplex, 9),
            );
        }
        let x_next_norm = norm3(&x_next);
        let tol = if n > 1 { min_norm } else { MINVAL };
        if x_next_norm < tol || (k > 0 && (x_next_norm - x_norm).abs() < MINVAL) {
            copy3(&mut x_k, &x_next);
            x_norm = x_next_norm;
            break;
        }
        copy3(&mut x_k, &x_next);
        x_norm = x_next_norm;
        k += 1;
    }
    if n > 0 {
        let s = &obj1.suppvert;
        lincomb(
            at3_mut(&mut status.x1, 0),
            &lambda,
            n,
            at3(s, 0),
            at3(s, 3),
            at3(s, 6),
            at3(s, 9),
        );
        let s = &obj2.suppvert;
        lincomb(
            at3_mut(&mut status.x2, 0),
            &lambda,
            n,
            at3(s, 0),
            at3(s, 3),
            at3(s, 6),
            at3(s, 9),
        );
    }
    if !status.separated && n < 4 && x_norm >= MINVAL {
        let mut tmp = Vertex::default();
        gjk_support(&mut tmp, obj1, obj2, &x_k, x_norm);
        if dot3(&x_k, &tmp.vert) > 0.0 {
            status.separated = true;
        }
    }
    status.nx = 1;
    status.gjk_iterations = k;
    obj1.nsupp = n;
    obj2.nsupp = n;
    status.dist[0] = x_norm;
}
// compute the support point in obj1 and obj2 for Minkowski difference
fn support(
    v: &mut Vertex,
    obj1: &CcdObj<'_>,
    obj2: &CcdObj<'_>,
    dir: &[f64; 3],
    dir_neg: &[f64; 3],
) {
    obj1.support.call(&mut v.vert1, obj1, dir);
    if obj1.margin > 0.0 && obj1.geom >= 0 {
        let margin = 0.5 * obj1.margin;
        v.vert1[0] += dir[0] * margin;
        v.vert1[1] += dir[1] * margin;
        v.vert1[2] += dir[2] * margin;
    }
    obj2.support.call(&mut v.vert2, obj2, dir_neg);
    if obj2.margin > 0.0 && obj2.geom >= 0 {
        let margin = 0.5 * obj2.margin;
        v.vert2[0] += dir_neg[0] * margin;
        v.vert2[1] += dir_neg[1] * margin;
        v.vert2[2] += dir_neg[2] * margin;
    }
    sub3(&mut v.vert, &v.vert1, &v.vert2);
    v.index1 = obj1.vertindex.get();
    v.index2 = obj2.vertindex.get();
}
// compute the support points in obj1 and obj2 for the kth approximation point
fn gjk_support(v: &mut Vertex, obj1: &CcdObj<'_>, obj2: &CcdObj<'_>, x_k: &[f64; 3], x_norm: f64) {
    let mut dir = [0.0; 3];
    let mut dir_neg = [0.0; 3];
    scl3(&mut dir_neg, x_k, 1.0 / x_norm);
    scl3(&mut dir, &dir_neg, -1.0);
    support(v, obj1, obj2, &dir, &dir_neg);
}
// compute support points in Minkowski difference, return index of new vertex in polytope
fn epa_support(
    pt: &mut Polytope,
    obj1: &CcdObj<'_>,
    obj2: &CcdObj<'_>,
    d: &[f64; 3],
    dnorm: f64,
) -> usize {
    let mut dir = [1.0, 0.0, 0.0];
    let mut dir_neg = [-1.0, 0.0, 0.0];
    if dnorm > MINVAL {
        dir[0] = d[0] / dnorm;
        dir[1] = d[1] / dnorm;
        dir[2] = d[2] / dnorm;
        scl3(&mut dir_neg, &dir, -1.0);
    }
    let n = pt.nverts;
    pt.nverts += 1;
    if pt.verts.len() < pt.nverts {
        pt.verts.resize(pt.nverts, Vertex::default());
    }
    support(&mut pt.verts[n], obj1, obj2, &dir, &dir_neg);
    n
}
// compute the linear combination of up to 4 3D vectors
fn lincomb(
    res: &mut [f64; 3],
    coef: &[f64],
    n: usize,
    v1: &[f64; 3],
    v2: &[f64; 3],
    v3: &[f64; 3],
    v4: &[f64; 3],
) {
    match n {
        0 => {
            res[0] = 0.0;
            res[1] = 0.0;
            res[2] = 0.0;
        }
        1 => {
            res[0] = coef[0] * v1[0];
            res[1] = coef[0] * v1[1];
            res[2] = coef[0] * v1[2];
        }
        2 => {
            res[0] = coef[0] * v1[0] + coef[1] * v2[0];
            res[1] = coef[0] * v1[1] + coef[1] * v2[1];
            res[2] = coef[0] * v1[2] + coef[1] * v2[2];
        }
        3 => {
            res[0] = coef[0] * v1[0] + coef[1] * v2[0] + coef[2] * v3[0];
            res[1] = coef[0] * v1[1] + coef[1] * v2[1] + coef[2] * v3[1];
            res[2] = coef[0] * v1[2] + coef[1] * v2[2] + coef[2] * v3[2];
        }
        4 => {
            res[0] = coef[0] * v1[0] + coef[1] * v2[0] + coef[2] * v3[0] + coef[3] * v4[0];
            res[1] = coef[0] * v1[1] + coef[1] * v2[1] + coef[2] * v3[1] + coef[3] * v4[1];
            res[2] = coef[0] * v1[2] + coef[1] * v2[2] + coef[2] * v3[2] + coef[3] * v4[2];
        }
        _ => {}
    }
}
// res = origin projected onto plane defined by v1, v2, v3
fn project_origin_plane(res: &mut [f64; 3], v1: &[f64; 3], v2: &[f64; 3], v3: &[f64; 3]) -> i32 {
    let mut diff21 = [0.0; 3];
    let mut diff31 = [0.0; 3];
    let mut diff32 = [0.0; 3];
    let mut n = [0.0; 3];
    sub3(&mut diff21, v2, v1);
    sub3(&mut diff31, v3, v1);
    sub3(&mut diff32, v3, v2);
    cross3(&mut n, &diff32, &diff21);
    let mut nv = dot3(&n, v2);
    let mut nn = dot3(&n, &n);
    if nn == 0.0 {
        return 1;
    }
    if nv != 0.0 && nn > MINVAL {
        scl3(res, &n, nv / nn);
        return 0;
    }
    cross3(&mut n, &diff21, &diff31);
    nv = dot3(&n, v1);
    nn = dot3(&n, &n);
    if nn == 0.0 {
        return 1;
    }
    if nv != 0.0 && nn > MINVAL {
        scl3(res, &n, nv / nn);
        return 0;
    }
    cross3(&mut n, &diff31, &diff32);
    nv = dot3(&n, v3);
    nn = dot3(&n, &n);
    if nn == 0.0 {
        return 1;
    }
    scl3(res, &n, nv / nn);
    0
}
// res = origin projected onto line defined by v1, v2
fn project_origin_line(res: &mut [f64; 3], v1: &[f64; 3], v2: &[f64; 3]) -> i32 {
    let mut diff = [0.0; 3];
    sub3(&mut diff, v2, v1);
    let dd = dot3(&diff, &diff);
    if dd < MINVAL2 {
        return 1;
    }
    let scl = -(dot3(v2, &diff) / dd);
    res[0] = v2[0] + scl * diff[0];
    res[1] = v2[1] + scl * diff[1];
    res[2] = v2[2] + scl * diff[2];
    0
}
// return 1 if both numbers are positive, -1 if both negative and 0 otherwise
fn same_sign2(a: f64, b: f64) -> i32 {
    if a > 0.0 && b > 0.0 {
        return 1;
    }
    if a < 0.0 && b < 0.0 {
        return -1;
    }
    0
}
// subdistance algorithm for GJK that computes the barycentric coordinates of the point in a
// simplex closest to the origin
// implementation adapted from Montanari et al, ToG 2017
fn subdistance(lambda: &mut [f64; 4], n: usize, simplex: &[f64; 12]) {
    lambda.fill(0.0);
    let s1 = at3(simplex, 0);
    let s2 = at3(simplex, 3);
    let s3 = at3(simplex, 6);
    let s4 = at3(simplex, 9);
    match n {
        4 => s3d(lambda, s1, s2, s3, s4),
        3 => s2d(at3_mut(lambda, 0), s1, s2, s3),
        2 => s1d((&mut lambda[..2]).try_into().unwrap(), s1, s2),
        _ => lambda[0] = 1.0,
    }
}
fn s3d(lambda: &mut [f64; 4], s1: &[f64; 3], s2: &[f64; 3], s3: &[f64; 3], s4: &[f64; 3]) {
    let c41 = -det3(s2, s3, s4);
    let c42 = det3(s1, s3, s4);
    let c43 = -det3(s1, s2, s4);
    let c44 = det3(s1, s2, s3);
    let m_det = c41 + c42 + c43 + c44;
    let comp1 = same_sign2(m_det, c41);
    let comp2 = same_sign2(m_det, c42);
    let comp3 = same_sign2(m_det, c43);
    let comp4 = same_sign2(m_det, c44);
    if comp1 != 0 && comp2 != 0 && comp3 != 0 && comp4 != 0 {
        lambda[0] = c41 / m_det;
        lambda[1] = c42 / m_det;
        lambda[2] = c43 / m_det;
        lambda[3] = c44 / m_det;
        return;
    }
    let mut dmin = MAX_LIMIT;
    if comp1 == 0 {
        let mut lambda_2d = [0.0; 3];
        let mut x = [0.0; 3];
        s2d(&mut lambda_2d, s2, s3, s4);
        lincomb(&mut x, &lambda_2d, 3, s2, s3, s4, &[0.0; 3]);
        let d = dot3(&x, &x);
        lambda[0] = 0.0;
        lambda[1] = lambda_2d[0];
        lambda[2] = lambda_2d[1];
        lambda[3] = lambda_2d[2];
        dmin = d;
    }
    if comp2 == 0 {
        let mut lambda_2d = [0.0; 3];
        let mut x = [0.0; 3];
        s2d(&mut lambda_2d, s1, s3, s4);
        lincomb(&mut x, &lambda_2d, 3, s1, s3, s4, &[0.0; 3]);
        let d = dot3(&x, &x);
        if d < dmin {
            lambda[0] = lambda_2d[0];
            lambda[1] = 0.0;
            lambda[2] = lambda_2d[1];
            lambda[3] = lambda_2d[2];
            dmin = d;
        }
    }
    if comp3 == 0 {
        let mut lambda_2d = [0.0; 3];
        let mut x = [0.0; 3];
        s2d(&mut lambda_2d, s1, s2, s4);
        lincomb(&mut x, &lambda_2d, 3, s1, s2, s4, &[0.0; 3]);
        let d = dot3(&x, &x);
        if d < dmin {
            lambda[0] = lambda_2d[0];
            lambda[1] = lambda_2d[1];
            lambda[2] = 0.0;
            lambda[3] = lambda_2d[2];
            dmin = d;
        }
    }
    if comp4 == 0 {
        let mut lambda_2d = [0.0; 3];
        let mut x = [0.0; 3];
        s2d(&mut lambda_2d, s1, s2, s3);
        lincomb(&mut x, &lambda_2d, 3, s1, s2, s3, &[0.0; 3]);
        let d = dot3(&x, &x);
        if d < dmin {
            lambda[0] = lambda_2d[0];
            lambda[1] = lambda_2d[1];
            lambda[2] = lambda_2d[2];
            lambda[3] = 0.0;
        }
    }
}
fn s2d(lambda: &mut [f64; 3], s1: &[f64; 3], s2: &[f64; 3], s3: &[f64; 3]) {
    let mut comp1 = 0;
    let mut comp2 = 0;
    let mut comp3 = 0;
    let mut p_o = [0.0; 3];
    if project_origin_plane(&mut p_o, s1, s2, s3) == 0 {
        let d21 = [s2[0] - s1[0], s2[1] - s1[1], s2[2] - s1[2]];
        let d31 = [s3[0] - s1[0], s3[1] - s1[1], s3[2] - s1[2]];
        let m_14 = d21[1] * d31[2] - d21[2] * d31[1];
        let m_24 = d21[0] * d31[2] - d21[2] * d31[0];
        let m_34 = d21[0] * d31[1] - d21[1] * d31[0];
        let m_max;
        let s1_2d;
        let s2_2d;
        let s3_2d;
        let p_o_2d;
        let mu1 = m_14.abs();
        let mu2 = m_24.abs();
        let mu3 = m_34.abs();
        if mu1 >= mu2 && mu1 >= mu3 {
            m_max = m_14;
            s1_2d = [s1[1], s1[2]];
            s2_2d = [s2[1], s2[2]];
            s3_2d = [s3[1], s3[2]];
            p_o_2d = [p_o[1], p_o[2]];
        } else if mu2 >= mu3 {
            m_max = m_24;
            s1_2d = [s1[0], s1[2]];
            s2_2d = [s2[0], s2[2]];
            s3_2d = [s3[0], s3[2]];
            p_o_2d = [p_o[0], p_o[2]];
        } else {
            m_max = m_34;
            s1_2d = [s1[0], s1[1]];
            s2_2d = [s2[0], s2[1]];
            s3_2d = [s3[0], s3[1]];
            p_o_2d = [p_o[0], p_o[1]];
        }
        let d1 = [s1_2d[0] - p_o_2d[0], s1_2d[1] - p_o_2d[1]];
        let d2 = [s2_2d[0] - p_o_2d[0], s2_2d[1] - p_o_2d[1]];
        let d3 = [s3_2d[0] - p_o_2d[0], s3_2d[1] - p_o_2d[1]];
        let c31 = d2[0] * d3[1] - d3[0] * d2[1];
        let c32 = d3[0] * d1[1] - d1[0] * d3[1];
        let c33 = d1[0] * d2[1] - d2[0] * d1[1];
        comp1 = same_sign2(m_max, c31);
        comp2 = same_sign2(m_max, c32);
        comp3 = same_sign2(m_max, c33);
        if comp1 != 0 && comp2 != 0 && comp3 != 0 {
            let m_det = c31 + c32 + c33;
            lambda[0] = c31 / m_det;
            lambda[1] = c32 / m_det;
            lambda[2] = c33 / m_det;
            return;
        }
    }
    let mut dmin = MAX_LIMIT;
    if comp1 == 0 {
        let mut lambda_1d = [0.0; 2];
        let mut x = [0.0; 3];
        s1d(&mut lambda_1d, s2, s3);
        lincomb(&mut x, &lambda_1d, 2, s2, s3, &[0.0; 3], &[0.0; 3]);
        let d = dot3(&x, &x);
        lambda[0] = 0.0;
        lambda[1] = lambda_1d[0];
        lambda[2] = lambda_1d[1];
        dmin = d;
    }
    if comp2 == 0 {
        let mut lambda_1d = [0.0; 2];
        let mut x = [0.0; 3];
        s1d(&mut lambda_1d, s1, s3);
        lincomb(&mut x, &lambda_1d, 2, s1, s3, &[0.0; 3], &[0.0; 3]);
        let d = dot3(&x, &x);
        if d < dmin {
            lambda[0] = lambda_1d[0];
            lambda[1] = 0.0;
            lambda[2] = lambda_1d[1];
            dmin = d;
        }
    }
    if comp3 == 0 {
        let mut lambda_1d = [0.0; 2];
        let mut x = [0.0; 3];
        s1d(&mut lambda_1d, s1, s2);
        lincomb(&mut x, &lambda_1d, 2, s1, s2, &[0.0; 3], &[0.0; 3]);
        let d = dot3(&x, &x);
        if d < dmin {
            lambda[0] = lambda_1d[0];
            lambda[1] = lambda_1d[1];
            lambda[2] = 0.0;
        }
    }
}
fn s1d(lambda: &mut [f64; 2], s1: &[f64; 3], s2: &[f64; 3]) {
    let mut p_o = [0.0; 3];
    if project_origin_line(&mut p_o, s1, s2) != 0 {
        lambda[0] = 0.0;
        lambda[1] = 1.0;
        return;
    }
    let mut mu = s1[0] - s2[0];
    let mut mu_max = mu;
    let mut index = 0;
    mu = s1[1] - s2[1];
    if mu.abs() >= mu_max.abs() {
        mu_max = mu;
        index = 1;
    }
    mu = s1[2] - s2[2];
    if mu.abs() >= mu_max.abs() {
        mu_max = mu;
        index = 2;
    }
    let c1 = p_o[index] - s2[index];
    let c2 = s1[index] - p_o[index];
    let comp1 = same_sign2(mu_max, c1);
    let comp2 = same_sign2(mu_max, c2);
    if comp1 != 0 && comp2 != 0 {
        lambda[0] = c1 / mu_max;
        lambda[1] = c2 / mu_max;
    } else if comp1 != 0 {
        lambda[0] = 1.0;
        lambda[1] = 0.0;
    } else {
        lambda[0] = 0.0;
        lambda[1] = 1.0;
    }
}

// replace a simplex with one of its edges
fn replace_simplex2(
    pt: &mut Polytope,
    obj1: &mut CcdObj<'_>,
    obj2: &mut CcdObj<'_>,
    v1: usize,
    v2: usize,
) {
    obj1.nsupp = 2;
    obj2.nsupp = 2;
    copy3(at3_mut(&mut obj1.suppvert, 0), &pt.verts[v1].vert1);
    copy3(at3_mut(&mut obj2.suppvert, 0), &pt.verts[v1].vert2);
    obj1.suppind[0] = pt.verts[v1].index1;
    obj2.suppind[0] = pt.verts[v1].index2;
    copy3(at3_mut(&mut obj1.suppvert, 3), &pt.verts[v2].vert1);
    copy3(at3_mut(&mut obj2.suppvert, 3), &pt.verts[v2].vert2);
    obj1.suppind[1] = pt.verts[v2].index1;
    obj2.suppind[1] = pt.verts[v2].index2;
    pt.nfaces = 0;
    pt.nverts = 0;
    pt.nmap = 0;
}
// return 1 if the origin lies strictly between v1 and v2 within mjMINEPATOL
fn origin_on_edge(v1: &[f64; 3], v2: &[f64; 3]) -> bool {
    let mut p = [0.0; 3];
    if dot3(v1, v2) < 0.0 && project_origin_line(&mut p, v1, v2) == 0 {
        return dot3(&p, &p) < MINEPATOL2;
    }
    false
}
// replace a 3-simplex with one of its faces
fn replace_simplex3(
    pt: &mut Polytope,
    obj1: &mut CcdObj<'_>,
    obj2: &mut CcdObj<'_>,
    v1: usize,
    v2: usize,
    v3: usize,
) {
    let v = [v1, v2, v3];
    obj1.nsupp = 3;
    obj2.nsupp = 3;
    for i in 0..3 {
        copy3(at3_mut(&mut obj1.suppvert, 3 * i), &pt.verts[v[i]].vert1);
        copy3(at3_mut(&mut obj2.suppvert, 3 * i), &pt.verts[v[i]].vert2);
        obj1.suppind[i] = pt.verts[v[i]].index1;
        obj2.suppind[i] = pt.verts[v[i]].index2;
    }
    pt.nfaces = 0;
    pt.nverts = 0;
    pt.nmap = 0;
}
// replace a 2-simplex with the better of (v1, va, vb) or (v2, vb, va) for polytope3
fn replace_best_simplex3(
    pt: &mut Polytope,
    obj1: &mut CcdObj<'_>,
    obj2: &mut CcdObj<'_>,
    v1i: usize,
    v2i: usize,
    vai: usize,
    vbi: usize,
) {
    let v1 = pt.verts[v1i].vert;
    let v2 = pt.verts[v2i].vert;
    let va = pt.verts[vai].vert;
    let vb = pt.verts[vbi].vert;
    let mut e1a = [0.0; 3];
    let mut e1b = [0.0; 3];
    let mut n1 = [0.0; 3];
    let mut e2a = [0.0; 3];
    let mut e2b = [0.0; 3];
    let mut n2 = [0.0; 3];
    sub3(&mut e1a, &va, &v1);
    sub3(&mut e1b, &vb, &v1);
    cross3(&mut n1, &e1a, &e1b);
    let nn1 = dot3(&n1, &n1);
    sub3(&mut e2a, &vb, &v2);
    sub3(&mut e2b, &va, &v2);
    cross3(&mut n2, &e2a, &e2b);
    let nn2 = dot3(&n2, &n2);
    if nn1 < MINVAL && nn2 >= MINVAL {
        replace_simplex3(pt, obj1, obj2, v2i, vbi, vai);
        return;
    }
    if nn2 < MINVAL && nn1 >= MINVAL {
        replace_simplex3(pt, obj1, obj2, v1i, vai, vbi);
        return;
    }
    let mut l1 = [0.0; 3];
    let mut l2 = [0.0; 3];
    let mut p1 = [0.0; 3];
    let mut p2 = [0.0; 3];
    s2d(&mut l1, &v1, &va, &vb);
    lincomb(&mut p1, &l1, 3, &v1, &va, &vb, &[0.0; 3]);
    let d1 = dot3(&p1, &p1);
    s2d(&mut l2, &v2, &vb, &va);
    lincomb(&mut p2, &l2, 3, &v2, &vb, &va, &[0.0; 3]);
    let d2 = dot3(&p2, &p2);
    if d2 < d1 {
        replace_simplex3(pt, obj1, obj2, v2i, vbi, vai);
    } else {
        replace_simplex3(pt, obj1, obj2, v1i, vai, vbi);
    }
}
// return 1 if the origin and p3 are on the same side of the plane defined by p0, p1, p2
fn same_side(p0: &[f64; 3], p1: &[f64; 3], p2: &[f64; 3], p3: &[f64; 3]) -> bool {
    let mut diff1 = [0.0; 3];
    let mut diff2 = [0.0; 3];
    let mut diff3 = [0.0; 3];
    let mut diff4 = [0.0; 3];
    let mut n = [0.0; 3];
    sub3(&mut diff1, p1, p0);
    sub3(&mut diff2, p2, p0);
    cross3(&mut n, &diff1, &diff2);
    sub3(&mut diff3, p3, p0);
    let dot1 = dot3(&n, &diff3);
    scl3(&mut diff4, p0, -1.0);
    let dot2 = dot3(&n, &diff4);
    if dot1 > 0.0 && dot2 > 0.0 {
        return true;
    }
    if dot1 < 0.0 && dot2 < 0.0 {
        return true;
    }
    false
}
// return 1 if the origin is contained in the tetrahedron, 0 otherwise
fn test_tetra(p0: &[f64; 3], p1: &[f64; 3], p2: &[f64; 3], p3: &[f64; 3]) -> bool {
    same_side(p0, p1, p2, p3)
        && same_side(p1, p2, p3, p0)
        && same_side(p2, p3, p0, p1)
        && same_side(p3, p0, p1, p2)
}
// matrix for 120 degrees rotation around given axis
fn rotmat(r: &mut [f64; 9], axis: &[f64; 3]) {
    let n = norm3(axis);
    let u1 = axis[0] / n;
    let u2 = axis[1] / n;
    let u3 = axis[2] / n;
    let sin = 0.86602540378;
    let cos = -0.5;
    r[0] = cos + u1 * u1 * (1.0 - cos);
    r[1] = u1 * u2 * (1.0 - cos) - u3 * sin;
    r[2] = u1 * u3 * (1.0 - cos) + u2 * sin;
    r[3] = u2 * u1 * (1.0 - cos) + u3 * sin;
    r[4] = cos + u2 * u2 * (1.0 - cos);
    r[5] = u2 * u3 * (1.0 - cos) - u1 * sin;
    r[6] = u1 * u3 * (1.0 - cos) - u2 * sin;
    r[7] = u2 * u3 * (1.0 - cos) + u1 * sin;
    r[8] = cos + u3 * u3 * (1.0 - cos);
}
// return nonzero if the ray v1v2 intersects the triangle v3v4v5 within tolerance
fn ray_triangle(
    det: Option<&mut [f64; 3]>,
    v1: &[f64; 3],
    v2: &[f64; 3],
    v3: &[f64; 3],
    v4: &[f64; 3],
    v5: &[f64; 3],
    tol: f64,
) -> i32 {
    let mut tmp = [0.0; 3];
    let mut diff12 = [0.0; 3];
    let mut diff13 = [0.0; 3];
    let mut diff14 = [0.0; 3];
    let mut diff15 = [0.0; 3];
    sub3(&mut diff12, v2, v1);
    sub3(&mut diff13, v3, v1);
    sub3(&mut diff14, v4, v1);
    sub3(&mut diff15, v5, v1);
    let det = det.unwrap_or(&mut tmp);
    det[0] = det3(&diff13, &diff14, &diff12);
    det[1] = det3(&diff14, &diff15, &diff12);
    det[2] = det3(&diff15, &diff13, &diff12);
    if det[0] >= tol && det[1] >= tol && det[2] >= tol {
        return 1;
    }
    if det[0] <= -tol && det[1] <= -tol && det[2] <= -tol {
        return -1;
    }
    0
}
// create a polytope from a 1-simplex (returns 0 on success)
fn polytope2(
    pt: &mut Polytope,
    status: &mut CcdStatus,
    obj1: &mut CcdObj<'_>,
    obj2: &mut CcdObj<'_>,
) -> i32 {
    let v1i = insert_vertex(pt, obj1, obj2, 0);
    let v2i = insert_vertex(pt, obj1, obj2, 1);
    let v1 = pt.verts[v1i].vert;
    let v2 = pt.verts[v2i].vert;
    let mut diff = [0.0; 3];
    sub3(&mut diff, &v2, &v1);
    let mut value = MAX_LIMIT;
    let mut index = 0;
    for i in 0..3 {
        if diff[i].abs() < value {
            value = diff[i].abs();
            index = i;
        }
    }
    let mut e = [0.0; 3];
    e[index] = 1.0;
    let mut d1 = [0.0; 3];
    let mut d2 = [0.0; 3];
    let mut d3 = [0.0; 3];
    cross3(&mut d1, &e, &diff);
    let mut r = [0.0; 9];
    rotmat(&mut r, &diff);
    mul_mat_vec3(&mut d2, &r, &d1);
    mul_mat_vec3(&mut d3, &r, &d2);
    let v3i = epa_support(pt, obj1, obj2, &d1, norm3(&d1));
    let v4i = epa_support(pt, obj1, obj2, &d2, norm3(&d2));
    let v5i = epa_support(pt, obj1, obj2, &d3, norm3(&d3));
    let mut v3 = pt.verts[v3i].vert;
    let mut v4 = pt.verts[v4i].vert;
    let mut v5 = pt.verts[v5i].vert;
    let mut det = [0.0; 3];
    let diff2 = dot3(&diff, &diff);
    let tol = MINVAL * diff2 * diff2.sqrt();
    if ray_triangle(Some(&mut det), &v1, &v2, &v3, &v4, &v5, tol) == 0 {
        let mut vai = v5i;
        let mut vbi = v3i;
        let mut vci = v4i;
        if det[0] <= det[1] && det[0] <= det[2] {
            vai = v3i;
            vbi = v4i;
            vci = v5i;
        } else if det[1] <= det[0] && det[1] <= det[2] {
            vai = v4i;
            vbi = v5i;
            vci = v3i;
        }
        let va = pt.verts[vai].vert;
        let vb = pt.verts[vbi].vert;
        let vc = pt.verts[vci].vert;
        let mut edge = [0.0; 3];
        let mut d4 = [0.0; 3];
        let mut d1c = [0.0; 3];
        sub3(&mut edge, &vb, &va);
        sub3(&mut d1c, &vc, &v1);
        cross3(&mut d4, &edge, &diff);
        let sign = if dot3(&d1c, &diff) >= 0.5 * diff2 {
            1e-3
        } else {
            -1e-3
        };
        let a = d4;
        add_scl3(&mut d4, &a, &diff, sign * norm3(&edge));
        let v6i = epa_support(pt, obj1, obj2, &d4, norm3(&d4));
        pt.nverts -= 1;
        let v6 = pt.verts[v6i];
        if ray_triangle(None, &v1, &v2, &v6.vert, &vb, &vc, tol) != 0 {
            pt.verts[vai] = v6;
        } else if ray_triangle(None, &v1, &v2, &va, &v6.vert, &vc, tol) != 0 {
            pt.verts[vbi] = v6;
        } else if ray_triangle(None, &v1, &v2, &va, &vb, &v6.vert, tol) != 0 {
            pt.verts[vci] = v6;
        }
        // The C v3/v4/v5 pointers see the in-place replacement above.
        v3 = pt.verts[v3i].vert;
        v4 = pt.verts[v4i].vert;
        v5 = pt.verts[v5i].vert;
    }
    for i in 0..3 {
        pt.center[i] = 0.2 * (v1[i] + v2[i] + v3[i] + v4[i] + v5[i]);
    }
    if attach_face(pt, v1i, v3i, v4i, 1, 3, 2) < MINDIST2 {
        replace_best_simplex3(pt, obj1, obj2, v1i, v2i, v3i, v4i);
        return polytope3(pt, status, obj1, obj2, false);
    }
    if attach_face(pt, v1i, v5i, v3i, 2, 4, 0) < MINDIST2 {
        replace_best_simplex3(pt, obj1, obj2, v1i, v2i, v5i, v3i);
        return polytope3(pt, status, obj1, obj2, false);
    }
    if attach_face(pt, v1i, v4i, v5i, 0, 5, 1) < MINDIST2 {
        replace_best_simplex3(pt, obj1, obj2, v1i, v2i, v4i, v5i);
        return polytope3(pt, status, obj1, obj2, false);
    }
    if attach_face(pt, v2i, v4i, v3i, 5, 0, 4) < MINDIST2 {
        replace_best_simplex3(pt, obj1, obj2, v1i, v2i, v3i, v4i);
        return polytope3(pt, status, obj1, obj2, false);
    }
    if attach_face(pt, v2i, v3i, v5i, 3, 1, 5) < MINDIST2 {
        replace_best_simplex3(pt, obj1, obj2, v1i, v2i, v5i, v3i);
        return polytope3(pt, status, obj1, obj2, false);
    }
    if attach_face(pt, v2i, v5i, v4i, 4, 2, 3) < MINDIST2 {
        replace_best_simplex3(pt, obj1, obj2, v1i, v2i, v4i, v5i);
        return polytope3(pt, status, obj1, obj2, false);
    }
    if ray_triangle(None, &v1, &v2, &v3, &v4, &v5, 0.0) == 0 {
        for i in 0..6 {
            if pt.faces[i].dist2 < MINEPATOL2 {
                let vai = if i % 3 == 0 {
                    v3i
                } else if i % 3 == 1 {
                    v5i
                } else {
                    v4i
                };
                let vbi = if i % 3 == 0 {
                    v4i
                } else if i % 3 == 1 {
                    v3i
                } else {
                    v5i
                };
                replace_best_simplex3(pt, obj1, obj2, v1i, v2i, vai, vbi);
                return polytope3(pt, status, obj1, obj2, false);
            }
        }
        return EPA_P2_NONCONVEX;
    }
    for i in 0..6 {
        pt.map[i] = i;
        pt.faces[i].index = i as i32;
    }
    pt.nmap = 6;
    0
}
// compute the affine coordinates of p on the triangle v1v2v3
fn tri_affine_coord(
    lambda: &mut [f64; 3],
    v1: &[f64; 3],
    v2: &[f64; 3],
    v3: &[f64; 3],
    p: &[f64; 3],
) {
    let d21 = [v2[0] - v1[0], v2[1] - v1[1], v2[2] - v1[2]];
    let d31 = [v3[0] - v1[0], v3[1] - v1[1], v3[2] - v1[2]];
    let m_14 = d21[1] * d31[2] - d21[2] * d31[1];
    let m_24 = d21[0] * d31[2] - d21[2] * d31[0];
    let m_34 = d21[0] * d31[1] - d21[1] * d31[0];
    let x;
    let y;
    let mu1 = m_14.abs();
    let mu2 = m_24.abs();
    let mu3 = m_34.abs();
    if mu1 >= mu2 && mu1 >= mu3 {
        x = 1;
        y = 2;
    } else if mu2 >= mu3 {
        x = 0;
        y = 2;
    } else {
        x = 0;
        y = 1;
    }
    let d1 = [v1[x] - p[x], v1[y] - p[y]];
    let d2 = [v2[x] - p[x], v2[y] - p[y]];
    let d3 = [v3[x] - p[x], v3[y] - p[y]];
    let c31 = d2[0] * d3[1] - d3[0] * d2[1];
    let c32 = d3[0] * d1[1] - d1[0] * d3[1];
    let c33 = d1[0] * d2[1] - d2[0] * d1[1];
    let m_det = c31 + c32 + c33;
    lambda[0] = c31 / m_det;
    lambda[1] = c32 / m_det;
    lambda[2] = c33 / m_det;
}
// return true if point p and triangle v1v2v3 intersect
fn tri_point_intersect(v1: &[f64; 3], v2: &[f64; 3], v3: &[f64; 3], p: &[f64; 3]) -> bool {
    let mut lambda = [0.0; 3];
    tri_affine_coord(&mut lambda, v1, v2, v3, p);
    if lambda[0] < 0.0 || lambda[1] < 0.0 || lambda[2] < 0.0 {
        return false;
    }
    let mut pr = [0.0; 3];
    let mut diff = [0.0; 3];
    pr[0] = v1[0] * lambda[0] + v2[0] * lambda[1] + v3[0] * lambda[2];
    pr[1] = v1[1] * lambda[0] + v2[1] * lambda[1] + v3[1] * lambda[2];
    pr[2] = v1[2] * lambda[0] + v2[2] * lambda[1] + v3[2] * lambda[2];
    sub3(&mut diff, &pr, p);
    norm3(&diff) < MINVAL
}
// recover hexahedron or tetrahedron when ray v4v5 misses interior of triangle v1v2v3;
// return 1 if hexahedron, 0 if tetrahedron
fn recover_hexa(idx: &mut [usize; 5], det: &[f64; 3]) -> bool {
    let i1 = idx[0];
    let i2 = idx[1];
    let i3 = idx[2];
    let i4 = idx[3];
    let i5 = idx[4];
    if det[0] > 0.0 && det[1] < -MINVAL && det[2] < -MINVAL {
        *idx = [i3, i5, i4, i1, i2];
        return true;
    }
    if det[1] > 0.0 && det[2] < -MINVAL && det[0] < -MINVAL {
        *idx = [i1, i5, i4, i2, i3];
        return true;
    }
    if det[2] > 0.0 && det[0] < -MINVAL && det[1] < -MINVAL {
        *idx = [i2, i5, i4, i3, i1];
        return true;
    }
    if det[0] <= det[1] && det[0] <= det[2] {
        idx[0] = i1;
        idx[1] = i2;
    } else if det[1] <= det[0] && det[1] <= det[2] {
        idx[0] = i2;
        idx[1] = i3;
    } else {
        idx[0] = i3;
        idx[1] = i1;
    }
    false
}
// create a polytope from a 2-simplex (returns 0 on success)
fn polytope3(
    pt: &mut Polytope,
    status: &mut CcdStatus,
    obj1: &mut CcdObj<'_>,
    obj2: &mut CcdObj<'_>,
    allow_p2: bool,
) -> i32 {
    let v1i = insert_vertex(pt, obj1, obj2, 0);
    let v2i = insert_vertex(pt, obj1, obj2, 1);
    let v3i = insert_vertex(pt, obj1, obj2, 2);
    let mut v1 = pt.verts[v1i].vert;
    let mut v2 = pt.verts[v2i].vert;
    let mut v3 = pt.verts[v3i].vert;
    let mut diff1 = [0.0; 3];
    let mut diff2 = [0.0; 3];
    let mut n = [0.0; 3];
    let mut n_neg = [0.0; 3];
    sub3(&mut diff1, &v2, &v1);
    sub3(&mut diff2, &v3, &v1);
    cross3(&mut n, &diff1, &diff2);
    let n_norm = norm3(&n);
    if n_norm < MINVAL {
        return EPA_P3_BAD_NORMAL;
    }
    scl3(&mut n_neg, &n, -1.0);
    let v5i = epa_support(pt, obj1, obj2, &n_neg, n_norm);
    let v4i = epa_support(pt, obj1, obj2, &n, n_norm);
    let mut v4 = pt.verts[v4i].vert;
    let mut v5 = pt.verts[v5i].vert;
    if tri_point_intersect(&v1, &v2, &v3, &v4) {
        return EPA_P3_INVALID_V4;
    }
    if tri_point_intersect(&v1, &v2, &v3, &v5) {
        return EPA_P3_INVALID_V5;
    }
    let mut d4 = [0.0; 3];
    let mut d5 = [0.0; 3];
    sub3(&mut d4, &v4, &v1);
    sub3(&mut d5, &v5, &v1);
    if dot3(&n, &d4) < MINEPATOL * n_norm || dot3(&n_neg, &d5) < MINEPATOL * n_norm {
        return EPA_P3_ORIGIN_ON_FACE;
    }
    let mut det = [0.0; 3];
    let mut idx = [v1i, v2i, v3i, v4i, v5i];
    let mut is_hexa = true;
    if ray_triangle(Some(&mut det), &v4, &v5, &v1, &v2, &v3, 0.0) == 0 {
        is_hexa = recover_hexa(&mut idx, &det);
        v1 = pt.verts[idx[0]].vert;
        v2 = pt.verts[idx[1]].vert;
        v3 = pt.verts[idx[2]].vert;
        v4 = pt.verts[idx[3]].vert;
        v5 = pt.verts[idx[4]].vert;
    }
    let dist0;
    let dist1;
    let dist2;
    let dist3;
    let mut dist4 = MAX_LIMIT;
    let mut dist5 = MAX_LIMIT;
    if is_hexa {
        for i in 0..3 {
            pt.center[i] = 0.2 * (v1[i] + v2[i] + v3[i] + v4[i] + v5[i]);
        }
        dist0 = attach_face(pt, idx[3], idx[0], idx[1], 1, 3, 2);
        dist1 = attach_face(pt, idx[3], idx[2], idx[0], 2, 4, 0);
        dist2 = attach_face(pt, idx[3], idx[1], idx[2], 0, 5, 1);
        dist3 = attach_face(pt, idx[4], idx[1], idx[0], 5, 0, 4);
        dist4 = attach_face(pt, idx[4], idx[0], idx[2], 3, 1, 5);
        dist5 = attach_face(pt, idx[4], idx[2], idx[1], 4, 2, 3);
    } else {
        add3(&mut pt.center, &v1, &v2);
        let a = pt.center;
        add3(&mut pt.center, &a, &v4);
        let a = pt.center;
        add3(&mut pt.center, &a, &v5);
        let a = pt.center;
        scl3(&mut pt.center, &a, 0.25);
        dist0 = attach_face(pt, idx[0], idx[1], idx[3], 1, 3, 2);
        dist1 = attach_face(pt, idx[0], idx[4], idx[1], 2, 3, 0);
        dist2 = attach_face(pt, idx[0], idx[3], idx[4], 0, 3, 1);
        dist3 = attach_face(pt, idx[4], idx[3], idx[1], 2, 0, 1);
    }
    if dist0 < MINDIST3
        || dist1 < MINDIST3
        || dist2 < MINDIST3
        || dist3 < MINDIST3
        || dist4 < MINDIST3
        || dist5 < MINDIST3
    {
        if allow_p2 {
            if origin_on_edge(&pt.verts[v1i].vert, &pt.verts[v2i].vert) {
                replace_simplex2(pt, obj1, obj2, v1i, v2i);
                return polytope2(pt, status, obj1, obj2);
            }
            if origin_on_edge(&pt.verts[v2i].vert, &pt.verts[v3i].vert) {
                replace_simplex2(pt, obj1, obj2, v2i, v3i);
                return polytope2(pt, status, obj1, obj2);
            }
            if origin_on_edge(&pt.verts[v3i].vert, &pt.verts[v1i].vert) {
                replace_simplex2(pt, obj1, obj2, v3i, v1i);
                return polytope2(pt, status, obj1, obj2);
            }
        }
        return EPA_P3_ORIGIN_ON_FACE;
    }
    let tol2 = status.tolerance * status.tolerance;
    if is_hexa {
        if (!same_side(&v4, &v1, &v2, &v3) && dist0 >= tol2)
            || (!same_side(&v4, &v3, &v1, &v2) && dist1 >= tol2)
            || (!same_side(&v4, &v2, &v3, &v1) && dist2 >= tol2)
            || (!same_side(&v5, &v2, &v1, &v3) && dist3 >= tol2)
            || (!same_side(&v5, &v1, &v3, &v2) && dist4 >= tol2)
            || (!same_side(&v5, &v3, &v2, &v1) && dist5 >= tol2)
        {
            return EPA_P3_MISSING_ORIGIN;
        }
    } else if (!same_side(&v1, &v2, &v4, &v5) && dist0 >= tol2)
        || (!same_side(&v1, &v5, &v2, &v4) && dist1 >= tol2)
        || (!same_side(&v1, &v4, &v5, &v2) && dist2 >= tol2)
        || (!same_side(&v5, &v4, &v2, &v1) && dist3 >= tol2)
    {
        return EPA_P3_MISSING_ORIGIN;
    }
    for i in 0..pt.nfaces {
        pt.map[i] = i;
        pt.faces[i].index = i as i32;
    }
    pt.nmap = pt.nfaces;
    0
}
// create a polytope from a 3-simplex (returns 0 on success)
fn polytope4(
    pt: &mut Polytope,
    status: &mut CcdStatus,
    obj1: &mut CcdObj<'_>,
    obj2: &mut CcdObj<'_>,
) -> i32 {
    let v1 = insert_vertex(pt, obj1, obj2, 0);
    let v2 = insert_vertex(pt, obj1, obj2, 1);
    let v3 = insert_vertex(pt, obj1, obj2, 2);
    let v4 = insert_vertex(pt, obj1, obj2, 3);
    add3(&mut pt.center, &pt.verts[v1].vert, &pt.verts[v2].vert);
    let a = pt.center;
    add3(&mut pt.center, &a, &pt.verts[v3].vert);
    let a = pt.center;
    add3(&mut pt.center, &a, &pt.verts[v4].vert);
    let a = pt.center;
    scl3(&mut pt.center, &a, 0.25);
    let dist0 = attach_face(pt, v1, v2, v3, 1, 3, 2);
    if dist0 < MINDIST4 {
        replace_simplex3(pt, obj1, obj2, v1, v2, v3);
        return polytope3(pt, status, obj1, obj2, true);
    }
    let dist1 = attach_face(pt, v1, v4, v2, 2, 3, 0);
    if dist1 < MINDIST4 {
        replace_simplex3(pt, obj1, obj2, v1, v4, v2);
        return polytope3(pt, status, obj1, obj2, true);
    }
    let dist2 = attach_face(pt, v1, v3, v4, 0, 3, 1);
    if dist2 < MINDIST4 {
        replace_simplex3(pt, obj1, obj2, v1, v3, v4);
        return polytope3(pt, status, obj1, obj2, true);
    }
    let dist3 = attach_face(pt, v4, v3, v2, 2, 0, 1);
    if dist3 < MINDIST4 {
        replace_simplex3(pt, obj1, obj2, v4, v3, v2);
        return polytope3(pt, status, obj1, obj2, true);
    }
    if !test_tetra(
        &pt.verts[v1].vert,
        &pt.verts[v2].vert,
        &pt.verts[v3].vert,
        &pt.verts[v4].vert,
    ) {
        let mut dmin = dist0;
        let mut t1 = v1;
        let mut t2 = v2;
        let mut t3 = v3;
        if dist1 < dmin {
            dmin = dist1;
            t1 = v1;
            t2 = v4;
            t3 = v2;
        }
        if dist2 < dmin {
            dmin = dist2;
            t1 = v1;
            t2 = v3;
            t3 = v4;
        }
        if dist3 < dmin {
            dmin = dist3;
            t1 = v4;
            t2 = v3;
            t3 = v2;
        }
        if dmin < MINEPATOL2 {
            replace_simplex3(pt, obj1, obj2, t1, t2, t3);
            return polytope3(pt, status, obj1, obj2, true);
        }
        return EPA_P4_MISSING_ORIGIN;
    }
    for i in 0..4 {
        pt.map[i] = i;
        pt.faces[i].index = i as i32;
    }
    pt.nmap = 4;
    0
}
// copy support vertex i from obj1 and obj2 into polytope and return its index
fn insert_vertex(pt: &mut Polytope, obj1: &CcdObj<'_>, obj2: &CcdObj<'_>, i: usize) -> usize {
    let n = pt.nverts;
    pt.nverts += 1;
    if pt.verts.len() < pt.nverts {
        pt.verts.resize(pt.nverts, Vertex::default());
    }
    let v = &mut pt.verts[n];
    copy3(&mut v.vert1, at3(&obj1.suppvert, 3 * i));
    copy3(&mut v.vert2, at3(&obj2.suppvert, 3 * i));
    sub3(&mut v.vert, &v.vert1, &v.vert2);
    v.index1 = obj1.suppind[i];
    v.index2 = obj2.suppind[i];
    n
}
// delete face from map (return non-zero on error)
fn delete_face(pt: &mut Polytope, face: usize) {
    let index = pt.faces[face].index;
    if index >= 0 {
        pt.nmap -= 1;
        pt.map[index as usize] = pt.map[pt.nmap];
        pt.faces[pt.map[index as usize]].index = index;
    }
    pt.faces[face].index = -2;
}
// return max number of faces that can be stored in polytope
fn max_faces(pt: &Polytope) -> usize {
    pt.maxfaces - pt.nfaces
}
// attach a face to the polytope with the given vertex indices; return squared distance to origin
fn attach_face(
    pt: &mut Polytope,
    v1: usize,
    v2: usize,
    v3: usize,
    adj1: usize,
    adj2: usize,
    adj3: usize,
) -> f64 {
    let idx = pt.nfaces;
    pt.nfaces += 1;
    if pt.faces.len() < pt.nfaces {
        pt.faces.resize(pt.nfaces, Face::default());
        pt.map.resize(pt.nfaces, 0);
        pt.maxfaces = pt.nfaces;
    }
    let face = &mut pt.faces[idx];
    face.verts = (v1 + (v2 << 10) + (v3 << 20)) as i32;
    face.adj[0] = adj1 as i32;
    face.adj[1] = adj2 as i32;
    face.adj[2] = adj3 as i32;
    let ret = project_origin_plane(
        &mut face.v,
        &pt.verts[v3].vert,
        &pt.verts[v2].vert,
        &pt.verts[v1].vert,
    );
    if ret != 0 {
        return 0.0;
    }
    let mut outward = [0.0; 3];
    sub3(&mut outward, &pt.verts[v1].vert, &pt.center);
    if dot3(&face.v, &outward) < 0.0 {
        let a = face.v;
        scl3(&mut face.v, &a, -1.0);
    }
    face.dist2 = dot3(&face.v, &face.v);
    face.index = -1;
    face.dist2
}
// add an edge to the horizon; return number of edges added
fn add_edge(pt: &mut Polytope, index: usize, edge: usize) -> i32 {
    let i = pt.horizon.nedges;
    pt.horizon.nedges += 1;
    // Grow at the C buffer-full boundary.
    if i >= pt.horizon.maxedges {
        eprintln!("EPA: growing storage for horizon edges on expanding polytope");
        pt.horizon.maxedges = pt.horizon.nedges;
        if pt.horizon.edges.len() < pt.horizon.maxedges {
            pt.horizon.edges.resize(pt.horizon.maxedges, 0);
        }
        if pt.horizon.indices.len() < pt.horizon.maxedges {
            pt.horizon.indices.resize(pt.horizon.maxedges, 0);
        }
    }
    pt.horizon.edges[i] = edge;
    pt.horizon.indices[i] = index;
    1
}
// get edge index where vertex lies
fn get_edge(face: &Face, vertex: usize) -> usize {
    let verts = epa_vert_expand(face.verts);
    if verts[0] == vertex {
        return 0;
    }
    if verts[1] == vertex {
        return 1;
    }
    2
}
// recursive call to build horizon; return 1 if face is visible from w otherwise 0
fn horizon_rec(pt: &mut Polytope, face: usize, e: usize) -> i32 {
    let tol = MINEPATOL * pt.faces[face].dist2.sqrt();
    let vis_tol = if tol > MINVAL { tol } else { MINVAL };
    if dot3(&pt.faces[face].v, &pt.horizon.w) - pt.faces[face].dist2 > vis_tol {
        let verts = epa_vert_expand(pt.faces[face].verts);
        delete_face(pt, face);
        let mut ret = 1;
        for k in 1..3 {
            let i = (e + k) % 3;
            let adj_face = pt.faces[face].adj[i] as usize;
            if pt.faces[adj_face].index > -2 {
                let adj_edge = get_edge(&pt.faces[adj_face], verts[(i + 1) % 3]);
                let r = horizon_rec(pt, adj_face, adj_edge);
                if r < 0 || (r == 0 && add_edge(pt, pt.faces[face].adj[i] as usize, adj_edge) == 0)
                {
                    ret = -1;
                }
            }
        }
        return ret;
    }
    0
}
// create horizon given the face as starting point; return 0 on success
fn horizon(pt: &mut Polytope, face: usize) -> i32 {
    delete_face(pt, face);
    let verts = epa_vert_expand(pt.faces[face].verts);
    let mut ret = 0;
    for i in 0..3 {
        let adj_face = pt.faces[face].adj[i] as usize;
        if pt.faces[adj_face].index > -2 {
            let adj_edge = get_edge(&pt.faces[adj_face], verts[(i + 1) % 3]);
            let r = horizon_rec(pt, adj_face, adj_edge);
            if r < 0 || (r == 0 && add_edge(pt, pt.faces[face].adj[i] as usize, adj_edge) == 0) {
                ret = -1;
            }
        }
    }
    ret
}
// recover witness points from EPA polytope, return signed distance between witness points
fn epa_witness(pt: &Polytope, face: &Face, x1: &mut [f64; 3], x2: &mut [f64; 3]) -> f64 {
    let mut lambda = [0.0; 3];
    let verts = epa_vert_expand(face.verts);
    let v1 = &pt.verts[verts[0]];
    let v2 = &pt.verts[verts[1]];
    let v3 = &pt.verts[verts[2]];
    tri_affine_coord(&mut lambda, &v1.vert, &v2.vert, &v3.vert, &face.v);
    lincomb(x1, &lambda, 3, &v1.vert1, &v2.vert1, &v3.vert1, &[0.0; 3]);
    lincomb(x2, &lambda, 3, &v1.vert2, &v2.vert2, &v3.vert2, &[0.0; 3]);
    -face.dist2.sqrt()
}
// return a face of the expanded polytope that best approximates the pentration depth
// witness points are in status->{x1, x2}
fn epa(
    status: &mut CcdStatus,
    pt: &mut Polytope,
    obj1: &mut CcdObj<'_>,
    obj2: &mut CcdObj<'_>,
) -> Option<usize> {
    let mut upper = MAX_LIMIT;
    let mut upper2 = MAX_LIMIT;
    let mut face = None;
    let discrete = discrete_geom(obj1) && discrete_geom(obj2);
    let tolerance = if discrete {
        MINEPATOL
    } else {
        status.tolerance
    };
    let mut k = 0;
    let kmax = if status.max_iterations < 1000 {
        status.max_iterations
    } else {
        1000
    };
    while k < kmax {
        let pface = face;
        let mut lower2 = MAX_LIMIT;
        for i in 0..pt.nmap {
            if pt.faces[pt.map[i]].dist2 < lower2 {
                face = Some(pt.map[i]);
                lower2 = pt.faces[pt.map[i]].dist2;
            }
        }
        if lower2 > upper2 || face.is_none() {
            face = pface;
            break;
        }
        let fi = face.unwrap();
        if lower2 <= 0.0 {
            eprintln!("EPA: origin lies on affine hull of face");
            break;
        }
        let lower = lower2.sqrt();
        let a = pt.faces[fi].v;
        let wi = epa_support(pt, obj1, obj2, &a, lower);
        let w = pt.verts[wi];
        let upper_k = dot3(&pt.faces[fi].v, &w.vert) / lower;
        if upper_k < upper {
            upper = upper_k;
            upper2 = upper * upper;
        }
        if upper_k - lower < tolerance {
            if k == 0 && upper < lower - tolerance {
                face = None;
                status.epa_status = EPA_NOCONTACT;
            }
            break;
        }
        if discrete {
            let mut i = 0;
            let nverts = pt.nverts - 1;
            while i < nverts {
                if w.index1 == pt.verts[i].index1 && w.index2 == pt.verts[i].index2 {
                    break;
                }
                i += 1;
            }
            if i != nverts {
                break;
            }
        }
        pt.horizon.w = w.vert;
        if horizon(pt, fi) < 0 {
            eprintln!("EPA: out of memory for horizon edges on expanding polytope, set ccd_iterations to at least {}",pt.horizon.nedges+11);
            face = None;
            break;
        }
        if pt.horizon.nedges < 3 {
            face = None;
            break;
        }
        let nfaces = pt.nfaces;
        let nedges = pt.horizon.nedges;
        if nedges > max_faces(pt) {
            eprintln!("EPA: growing storage for faces on expanding polytope");
            pt.maxfaces = pt.nfaces + nedges;
            if pt.faces.len() < pt.maxfaces {
                pt.faces.resize(pt.maxfaces, Face::default());
            }
            if pt.map.len() < pt.maxfaces {
                pt.map.resize(pt.maxfaces, 0);
            }
        }
        let mut hzn_index = pt.horizon.indices[0];
        let mut hzn_edge = pt.horizon.edges[0];
        let hzn_verts = epa_vert_expand(pt.faces[hzn_index].verts);
        let mut v1 = hzn_verts[hzn_edge];
        let mut v2 = hzn_verts[(hzn_edge + 1) % 3];
        pt.faces[hzn_index].adj[hzn_edge] = nfaces as i32;
        let mut dist2 = attach_face(pt, wi, v2, v1, nfaces + nedges - 1, hzn_index, nfaces + 1);
        if dist2 == 0.0 {
            break;
        }
        if dist2 >= lower2 && dist2 <= upper2 {
            let i = pt.nmap;
            pt.nmap += 1;
            pt.map[i] = pt.nfaces - 1;
            pt.faces[pt.map[i]].index = i as i32;
        }
        for i in 1..nedges {
            let cur = nfaces + i;
            let next = nfaces + (i + 1) % nedges;
            hzn_index = pt.horizon.indices[i];
            hzn_edge = pt.horizon.edges[i];
            let hzn_verts2 = epa_vert_expand(pt.faces[hzn_index].verts);
            v1 = hzn_verts2[hzn_edge];
            v2 = hzn_verts2[(hzn_edge + 1) % 3];
            pt.faces[hzn_index].adj[hzn_edge] = cur as i32;
            dist2 = attach_face(pt, wi, v2, v1, cur - 1, hzn_index, next);
            if dist2 == 0.0 {
                pt.nmap = 0;
                break;
            }
            if dist2 >= lower2 && dist2 <= upper2 {
                let idx = pt.nmap;
                pt.nmap += 1;
                pt.map[idx] = pt.nfaces - 1;
                pt.faces[pt.map[idx]].index = idx as i32;
            }
        }
        pt.horizon.nedges = 0;
        if pt.nmap == 0 || face.is_none() {
            break;
        }
        k += 1;
    }
    status.epa_iterations = k;
    if let Some(fi) = face {
        status.dist[0] = epa_witness(
            pt,
            &pt.faces[fi],
            at3_mut(&mut status.x1, 0),
            at3_mut(&mut status.x2, 0),
        );
        status.nx = 1;
        let verts = epa_vert_expand(pt.faces[fi].verts);
        obj1.nsupp = 3;
        obj2.nsupp = 3;
        for i in 0..3 {
            copy3(
                at3_mut(&mut obj1.suppvert, 3 * i),
                &pt.verts[verts[i]].vert1,
            );
            copy3(
                at3_mut(&mut obj2.suppvert, 3 * i),
                &pt.verts[verts[i]].vert2,
            );
            obj1.suppind[i] = pt.verts[verts[i]].index1;
            obj2.suppind[i] = pt.verts[verts[i]].index2;
        }
    } else {
        status.nx = 0;
        status.dist[0] = 0.0;
    }
    face
}

// compute area of a quadrilateral (helper for hull4)
fn area4(hull: &[f64], a: usize, b: usize, c: usize, d: usize) -> f64 {
    let mut ca = [0.0; 3];
    let mut db = [0.0; 3];
    let mut cross = [0.0; 3];
    sub3(&mut ca, at3(hull, 3 * a), at3(hull, 3 * c));
    sub3(&mut db, at3(hull, 3 * b), at3(hull, 3 * d));
    cross3(&mut cross, &ca, &db);
    0.5 * norm3(&cross)
}
// prune a convex hull to a maximum area convex quadrilateral
fn hull4(res: &mut [usize; 4], hull: &[f64], nhull: usize) {
    let mut a = 0;
    let mut b = 1;
    let mut c = 2;
    let mut d = 3;
    *res = [0, 1, 2, 3];
    let mut m = area4(hull, a, b, c, d);
    while a < nhull {
        loop {
            let d_next = (d + 1) % nhull;
            let mut m_next = area4(hull, a, b, c, d_next);
            if m_next <= m {
                break;
            }
            d = d_next;
            m = m_next;
            *res = [a, b, c, d];
            loop {
                let c_next = (c + 1) % nhull;
                m_next = area4(hull, a, b, c_next, d);
                if m_next <= m {
                    break;
                }
                c = c_next;
                m = m_next;
                *res = [a, b, c, d];
            }
            loop {
                let b_next = (b + 1) % nhull;
                m_next = area4(hull, a, b_next, c, d);
                if m_next <= m {
                    break;
                }
                b = b_next;
                m = m_next;
                *res = [a, b, c, d];
            }
        }
        if b == a {
            b = (b + 1) % nhull;
            if c == b {
                c = (c + 1) % nhull;
                if d == c {
                    d = (d + 1) % nhull;
                }
            }
        }
        a += 1;
    }
}
// find the normal of a plane perpendicular to the face (given by its normal n) and intersecting the
// face edge (v1, v2)
fn plane_normal(res: &mut [f64; 3], v1: &[f64; 3], v2: &[f64; 3], n: &[f64; 3]) -> f64 {
    let mut v3 = [0.0; 3];
    let mut diff1 = [0.0; 3];
    let mut diff2 = [0.0; 3];
    add3(&mut v3, v1, n);
    sub3(&mut diff1, v2, v1);
    sub3(&mut diff2, &v3, v1);
    cross3(res, &diff1, &diff2);
    normalize3(res);
    dot3(res, v1)
}
// find what side of a plane a point p lies
fn halfspace(a: &[f64; 3], n: &[f64; 3], p: &[f64; 3]) -> bool {
    let diff = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    dot3(&diff, n) > -MINVAL
}
// compute witness points on face (given by point p and normal n) from clipped vertex
fn witness_on_face(
    w1: &mut [f64; 3],
    w2: &mut [f64; 3],
    v: &[f64; 3],
    p: &[f64; 3],
    n: &[f64; 3],
    dir: &[f64; 3],
) -> f64 {
    let mut d = [v[0] - p[0], v[1] - p[1], v[2] - p[2]];
    add_scl3(w1, v, n, -dot3(&d, n));
    sub3(&mut d, w1, v);
    let dist = dot3(&d, dir);
    add_scl3(w2, w1, dir, -dist);
    dist
}
// clip a polygon against another polygon, return number of contacts written
fn polygon_clip(
    x1: &mut [f64],
    x2: &mut [f64],
    mut dist: Option<&mut [f64]>,
    nconmax: usize,
    face1: &[f64],
    nface1: usize,
    face2: &[f64],
    nface2: usize,
    n: &[f64; 3],
    dir: &[f64; 3],
    buffer: &mut [f64],
    npolygonmax: usize,
    margin: f64,
) -> usize {
    if nface1 != 0 && nface1 < 3 {
        return 0;
    }
    let (mut polygon, rest) = buffer.split_at_mut(6 * npolygonmax);
    let (mut clipped, rest) = rest.split_at_mut(6 * npolygonmax);
    let (pn, pd) = rest.split_at_mut(3 * npolygonmax);
    if nface1 > 0 {
        for i in 0..nface1 - 1 {
            pd[i] = plane_normal(
                at3_mut(pn, 3 * i),
                at3(face1, 3 * i),
                at3(face1, 3 * i + 3),
                n,
            );
        }
        pd[nface1 - 1] = plane_normal(
            at3_mut(pn, 3 * (nface1 - 1)),
            at3(face1, 3 * (nface1 - 1)),
            at3(face1, 0),
            n,
        );
    }
    let mut npolygon = nface2;
    let mut nclipped = 0;
    for i in 0..nface2 {
        copy3(at3_mut(polygon, 3 * i), at3(face2, 3 * i));
    }
    for e in (0..3 * nface1).step_by(3) {
        for i in 0..npolygon {
            let p = at3(polygon, 3 * i);
            let q = if i < npolygon - 1 {
                at3(polygon, 3 * (i + 1))
            } else {
                at3(polygon, 0)
            };
            let pq = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
            let inside1 = halfspace(at3(face1, e), at3(pn, e), p);
            let inside2 = halfspace(at3(face1, e), at3(pn, e), q);
            if !inside1 && !inside2 {
                continue;
            }
            if inside1 && inside2 {
                copy3(at3_mut(clipped, 3 * nclipped), q);
                nclipped += 1;
                continue;
            }
            let d_p = [p[0] - face1[e], p[1] - face1[e + 1], p[2] - face1[e + 2]];
            let d_q = [q[0] - face1[e], q[1] - face1[e + 1], q[2] - face1[e + 2]];
            let dist_p = dot3(&d_p, at3(pn, e));
            let dist_q = dot3(&d_q, at3(pn, e));
            let denom = dist_p - dist_q;
            if denom != 0.0 {
                let t = dist_p / denom;
                if t >= 0.0 && t <= 1.0 {
                    add_scl3(at3_mut(clipped, 3 * nclipped), p, &pq, t);
                    nclipped += 1;
                }
            }
            if inside2 {
                copy3(at3_mut(clipped, 3 * nclipped), q);
                nclipped += 1;
            }
        }
        std::mem::swap(&mut polygon, &mut clipped);
        npolygon = nclipped;
        nclipped = 0;
    }
    let m = npolygon;
    npolygon = 0;
    for i in 0..m {
        let mut diff = [0.0; 3];
        sub3(&mut diff, at3(polygon, 3 * i), at3(face1, 0));
        if dot3(&diff, n) <= margin {
            if npolygon != i {
                let a = *at3(polygon, 3 * i);
                copy3(at3_mut(polygon, 3 * npolygon), &a);
            }
            npolygon += 1;
        }
    }
    if npolygon < 1 {
        return 0;
    }
    if nconmax < 5 && npolygon > 4 {
        let mut idx = [0; 4];
        hull4(&mut idx, polygon, npolygon);
        for i in 0..4 {
            let d = witness_on_face(
                at3_mut(x1, 3 * i),
                at3_mut(x2, 3 * i),
                at3(polygon, 3 * idx[i]),
                at3(face1, 0),
                n,
                dir,
            );
            if let Some(dist) = dist.as_deref_mut() {
                dist[i] = d;
            }
        }
        return 4;
    }
    if nface2 == 2 && npolygon > 2 {
        let mut best1 = 0;
        let mut best2 = 1;
        let mut d = 0.0;
        for i in 0..npolygon {
            for j in i + 1..npolygon {
                let mut diff = [0.0; 3];
                sub3(&mut diff, at3(polygon, 3 * j), at3(polygon, 3 * i));
                let d2 = dot3(&diff, &diff);
                if d2 > d {
                    d = d2;
                    best1 = i;
                    best2 = j;
                }
            }
        }
        let d0 = witness_on_face(
            at3_mut(x1, 0),
            at3_mut(x2, 0),
            at3(polygon, 3 * best1),
            at3(face1, 0),
            n,
            dir,
        );
        let d1 = witness_on_face(
            at3_mut(x1, 3),
            at3_mut(x2, 3),
            at3(polygon, 3 * best2),
            at3(face1, 0),
            n,
            dir,
        );
        if let Some(dist) = dist {
            dist[0] = d0;
            dist[1] = d1;
        }
        return 2;
    }
    npolygon = if npolygon < nconmax {
        npolygon
    } else {
        nconmax
    };
    for i in 0..npolygon {
        let d = witness_on_face(
            at3_mut(x1, 3 * i),
            at3_mut(x2, 3 * i),
            at3(polygon, 3 * i),
            at3(face1, 0),
            n,
            dir,
        );
        if let Some(dist) = dist.as_deref_mut() {
            dist[i] = d;
        }
    }
    npolygon
}
// compute global coordinates of a local point (l1, l2, l3)
fn globalcoord(
    res: &mut [f64; 3],
    mat: &[f64; 9],
    pos: Option<&[f64; 3]>,
    l1: f64,
    l2: f64,
    l3: f64,
) {
    res[0] = mat[0] * l1 + mat[1] * l2 + mat[2] * l3;
    res[1] = mat[3] * l1 + mat[4] * l2 + mat[5] * l3;
    res[2] = mat[6] * l1 + mat[7] * l2 + mat[8] * l3;
    if let Some(pos) = pos {
        res[0] += pos[0];
        res[1] += pos[1];
        res[2] += pos[2];
    }
}
// find up to n <= 2 common integers of two arrays, return n
fn intersect(res: &mut [i32; 2], arr1: &[i32], arr2: &[i32], n: usize, m: usize) -> usize {
    let mut count = 0;
    for i in 0..n {
        for j in 0..m {
            if arr1[i] == arr2[j] {
                res[count] = arr1[i];
                count += 1;
                if count == 2 {
                    return 2;
                }
            }
        }
    }
    count
}
// compute possible polygon normals of a mesh given up to 3 vertices
fn mesh_normals(
    res: &mut [f64],
    resind: &mut [i32],
    dim: usize,
    obj: &CcdObj<'_>,
    vi: &[i32; 3],
) -> usize {
    let v1 = vi[0] as usize;
    let v2 = vi[1] as usize;
    let v3 = vi[2] as usize;
    let polymap = obj.data.mesh.polymap;
    let polynormal = obj.data.mesh.polynormal;
    if dim == 3 {
        let v1_adr = obj.data.mesh.mpolymapadr[v1] as usize;
        let v1_num = obj.data.mesh.mpolymapnum[v1] as usize;
        let v2_adr = obj.data.mesh.mpolymapadr[v2] as usize;
        let v2_num = obj.data.mesh.mpolymapnum[v2] as usize;
        let v3_adr = obj.data.mesh.mpolymapadr[v3] as usize;
        let v3_num = obj.data.mesh.mpolymapnum[v3] as usize;
        let mut edgeset = [0; 2];
        let mut faceset = [0; 2];
        let mut n = intersect(
            &mut edgeset,
            &polymap[v1_adr..],
            &polymap[v2_adr..],
            v1_num,
            v2_num,
        );
        if n == 0 {
            return 0;
        }
        n = intersect(&mut faceset, &edgeset, &polymap[v3_adr..], n, v3_num);
        if n == 0 {
            return 0;
        }
        let normal = at3(polynormal, 3 * faceset[0] as usize);
        globalcoord(
            at3_mut(res, 0),
            &obj.mat,
            None,
            normal[0],
            normal[1],
            normal[2],
        );
        resind[0] = faceset[0];
        return 1;
    }
    if dim == 2 {
        let v1_adr = obj.data.mesh.mpolymapadr[v1] as usize;
        let v1_num = obj.data.mesh.mpolymapnum[v1] as usize;
        let v2_adr = obj.data.mesh.mpolymapadr[v2] as usize;
        let v2_num = obj.data.mesh.mpolymapnum[v2] as usize;
        let mut edgeset = [0; 2];
        let n = intersect(
            &mut edgeset,
            &polymap[v1_adr..],
            &polymap[v2_adr..],
            v1_num,
            v2_num,
        );
        if n == 0 {
            return 0;
        }
        for i in 0..n {
            let normal = at3(polynormal, 3 * edgeset[i] as usize);
            globalcoord(
                at3_mut(res, 3 * i),
                &obj.mat,
                None,
                normal[0],
                normal[1],
                normal[2],
            );
            resind[i] = edgeset[i];
        }
        return n;
    }
    if dim == 1 {
        let v1_adr = obj.data.mesh.mpolymapadr[v1] as usize;
        let v1_num = obj.data.mesh.mpolymapnum[v1] as usize;
        for i in 0..v1_num {
            let index = polymap[v1_adr + i];
            let normal = at3(polynormal, 3 * index as usize);
            globalcoord(
                at3_mut(res, 3 * i),
                &obj.mat,
                None,
                normal[0],
                normal[1],
                normal[2],
            );
            resind[i] = index;
        }
        return v1_num;
    }
    0
}
// compute normal directional vectors along possible edges given by up to two vertices
fn mesh_edge_normals(
    res: &mut [f64],
    endverts: &mut [f64],
    dim: usize,
    obj: &CcdObj<'_>,
    v: &mut [f64; 9],
    vi: &[i32; 3],
) -> usize {
    let v1i = vi[0] as usize;
    let v2i = vi[1] as usize;
    let mat = &obj.mat;
    let pos = &obj.pos;
    let vert = obj.data.mesh.vert;
    let polyvert = obj.data.mesh.polyvert;
    globalcoord(
        at3_mut(v, 0),
        mat,
        Some(pos),
        vert[3 * v1i] as f64,
        vert[3 * v1i + 1] as f64,
        vert[3 * v1i + 2] as f64,
    );
    if dim == 2 {
        globalcoord(
            at3_mut(v, 3),
            mat,
            Some(pos),
            vert[3 * v2i] as f64,
            vert[3 * v2i + 1] as f64,
            vert[3 * v2i + 2] as f64,
        );
        copy3(at3_mut(endverts, 0), at3(v, 3));
        sub3(at3_mut(res, 0), at3(v, 3), at3(v, 0));
        normalize3(at3_mut(res, 0));
        return 1;
    }
    if dim == 1 {
        let v1_adr = obj.data.mesh.mpolymapadr[v1i] as usize;
        let v1_num = obj.data.mesh.mpolymapnum[v1i] as usize;
        for i in 0..v1_num {
            let idx = obj.data.mesh.polymap[v1_adr + i] as usize;
            let adr = obj.data.mesh.polyvertadr[idx] as usize;
            let nvert = obj.data.mesh.polyvertnum[idx] as usize;
            for j in 0..nvert {
                if polyvert[adr + j] == v1i as i32 {
                    let k = if j == 0 { nvert - 1 } else { j - 1 };
                    let vert_k = &vert[3 * polyvert[adr + k] as usize..];
                    globalcoord(
                        at3_mut(endverts, 3 * i),
                        mat,
                        Some(pos),
                        vert_k[0] as f64,
                        vert_k[1] as f64,
                        vert_k[2] as f64,
                    );
                    sub3(at3_mut(res, 3 * i), at3(endverts, 3 * i), at3(v, 0));
                    normalize3(at3_mut(res, 3 * i));
                    break;
                }
            }
        }
        return v1_num;
    }
    0
}
// try recovering the cylinder normal from vertex index
fn cylinder_normals(
    res: &mut [f64],
    resind: &mut [i32],
    dim: usize,
    obj: &CcdObj<'_>,
    vi: &[i32; 3],
    _dir: &[f64; 3],
) -> usize {
    if dim == 1 {
        globalcoord(
            at3_mut(res, 0),
            &obj.mat,
            None,
            0.0,
            0.0,
            if vi[0] != 0 { -1.0 } else { 1.0 },
        );
        resind[0] = vi[0];
        return 1;
    }
    0
}
// try recovering box normal from collision normal
fn box_normals2(res: &mut [f64], resind: &mut [i32], mat: &[f64; 9], n: &[f64; 3]) -> usize {
    let normals = [
        1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0,
    ];
    let mut local_n = [0.0; 3];
    local_n[0] = mat[0] * n[0] + mat[3] * n[1] + mat[6] * n[2];
    local_n[1] = mat[1] * n[0] + mat[4] * n[1] + mat[7] * n[2];
    local_n[2] = mat[2] * n[0] + mat[5] * n[1] + mat[8] * n[2];
    let a = local_n;
    scl3(&mut local_n, &a, 1.0 / dot3(&a, &a).sqrt());
    for i in 0..6 {
        if dot3(&local_n, at3(&normals, 3 * i)) > FACE_TOL {
            globalcoord(
                at3_mut(res, 0),
                mat,
                None,
                normals[3 * i],
                normals[3 * i + 1],
                normals[3 * i + 2],
            );
            resind[0] = i as i32;
            return 1;
        }
    }
    0
}
// compute possible face normals of a box given up to 3 vertices
fn box_normals(
    res: &mut [f64],
    resind: &mut [i32],
    dim: usize,
    obj: &CcdObj<'_>,
    vi: &[i32; 3],
    dir: &[f64; 3],
) -> usize {
    let v1 = vi[0];
    let v2 = vi[1];
    let v3 = vi[2];
    let mat = &obj.mat;
    if dim == 3 {
        let mut c = 0;
        let x = ((v1 & 1 != 0) && (v2 & 1 != 0) && (v3 & 1 != 0)) as i32
            - ((v1 & 1 == 0) && (v2 & 1 == 0) && (v3 & 1 == 0)) as i32;
        let y = ((v1 & 2 != 0) && (v2 & 2 != 0) && (v3 & 2 != 0)) as i32
            - ((v1 & 2 == 0) && (v2 & 2 == 0) && (v3 & 2 == 0)) as i32;
        let z = ((v1 & 4 != 0) && (v2 & 4 != 0) && (v3 & 4 != 0)) as i32
            - ((v1 & 4 == 0) && (v2 & 4 == 0) && (v3 & 4 == 0)) as i32;
        globalcoord(at3_mut(res, 0), mat, None, x as f64, y as f64, z as f64);
        let sgn = x + y + z;
        if x != 0 {
            resind[c] = 0;
            c += 1;
        }
        if y != 0 {
            resind[c] = 2;
            c += 1;
        }
        if z != 0 {
            resind[c] = 4;
            c += 1;
        }
        if sgn == -1 {
            resind[0] += 1;
        }
        return if c == 1 {
            1
        } else {
            box_normals2(res, resind, mat, dir)
        };
    }
    if dim == 2 {
        let mut c = 0;
        let x = ((v1 & 1 != 0) && (v2 & 1 != 0)) as i32 - ((v1 & 1 == 0) && (v2 & 1 == 0)) as i32;
        let y = ((v1 & 2 != 0) && (v2 & 2 != 0)) as i32 - ((v1 & 2 == 0) && (v2 & 2 == 0)) as i32;
        let z = ((v1 & 4 != 0) && (v2 & 4 != 0)) as i32 - ((v1 & 4 == 0) && (v2 & 4 == 0)) as i32;
        if x != 0 {
            globalcoord(at3_mut(res, 0), mat, None, x as f64, 0.0, 0.0);
            resind[c] = if x > 0 { 0 } else { 1 };
            c += 1;
        }
        if y != 0 {
            globalcoord(at3_mut(res, 3 * c), mat, None, 0.0, y as f64, 0.0);
            resind[c] = if y > 0 { 2 } else { 3 };
            c += 1;
        }
        if z != 0 {
            globalcoord(at3_mut(res, 3 * c), mat, None, 0.0, 0.0, z as f64);
            resind[c] = if z > 0 { 4 } else { 5 };
            c += 1;
        }
        return if c != 0 {
            c
        } else {
            box_normals2(res, resind, mat, dir)
        };
    }
    if dim == 1 {
        let x = if v1 & 1 != 0 { 1.0 } else { -1.0 };
        let y = if v1 & 2 != 0 { 1.0 } else { -1.0 };
        let z = if v1 & 4 != 0 { 1.0 } else { -1.0 };
        globalcoord(at3_mut(res, 0), mat, None, x, 0.0, 0.0);
        globalcoord(at3_mut(res, 3), mat, None, 0.0, y, 0.0);
        globalcoord(at3_mut(res, 6), mat, None, 0.0, 0.0, z);
        resind[0] = if x > 0.0 { 0 } else { 1 };
        resind[1] = if y > 0.0 { 2 } else { 3 };
        resind[2] = if z > 0.0 { 4 } else { 5 };
        return 3;
    }
    0
}
// compute possible edge normals for box for edge collisions
fn box_edge_normals(
    res: &mut [f64],
    endverts: &mut [f64],
    dim: usize,
    obj: &CcdObj<'_>,
    v: &mut [f64; 9],
    vi: &[i32; 3],
) -> usize {
    let v1i = vi[0];
    let v2i = vi[1];
    let mat = &obj.mat;
    let pos = Some(&obj.pos);
    let size = &obj.size;
    let x = if v1i & 1 != 0 { size[0] } else { -size[0] };
    let y = if v1i & 2 != 0 { size[1] } else { -size[1] };
    let z = if v1i & 4 != 0 { size[2] } else { -size[2] };
    globalcoord(at3_mut(v, 0), mat, pos, x, y, z);
    if dim == 2 {
        let x2 = if v2i & 1 != 0 { size[0] } else { -size[0] };
        let y2 = if v2i & 2 != 0 { size[1] } else { -size[1] };
        let z2 = if v2i & 4 != 0 { size[2] } else { -size[2] };
        globalcoord(at3_mut(v, 3), mat, pos, x2, y2, z2);
        copy3(at3_mut(endverts, 0), at3(v, 3));
        sub3(at3_mut(res, 0), at3(v, 3), at3(v, 0));
        normalize3(at3_mut(res, 0));
        return 1;
    }
    if dim == 1 {
        globalcoord(at3_mut(endverts, 0), mat, pos, -x, y, z);
        sub3(at3_mut(res, 0), at3(endverts, 0), at3(v, 0));
        normalize3(at3_mut(res, 0));
        globalcoord(at3_mut(endverts, 3), mat, pos, x, -y, z);
        sub3(at3_mut(res, 3), at3(endverts, 3), at3(v, 0));
        normalize3(at3_mut(res, 3));
        globalcoord(at3_mut(endverts, 6), mat, pos, x, y, -z);
        sub3(at3_mut(res, 6), at3(endverts, 6), at3(v, 0));
        normalize3(at3_mut(res, 6));
        return 3;
    }
    0
}
// recover edge of a cylinder from witness point
fn cylinder_edge_normals(
    res: &mut [f64],
    endverts: &mut [f64],
    dim: usize,
    obj: &CcdObj<'_>,
    v: &mut [f64; 9],
    v1i: i32,
) -> usize {
    if dim == 1 || dim == 2 {
        let sgn = if v1i != 0 { 1.0 } else { -1.0 };
        res[0] = sgn * obj.mat[2];
        res[1] = sgn * obj.mat[5];
        res[2] = sgn * obj.mat[8];
        if obj.margin > 0.0 && obj.geom >= 0 {
            let mut diff = [0.0; 3];
            let mut local = [0.0; 2];
            sub3(&mut diff, at3(v, 0), &obj.pos);
            local[0] = obj.mat[0] * diff[0] + obj.mat[3] * diff[1] + obj.mat[6] * diff[2];
            local[1] = obj.mat[1] * diff[0] + obj.mat[4] * diff[1] + obj.mat[7] * diff[2];
            let n2 = local[0] * local[0] + local[1] * local[1];
            let scl = if n2 >= MINVAL2 {
                obj.size[0] / n2.sqrt()
            } else {
                0.0
            };
            globalcoord(
                at3_mut(v, 0),
                &obj.mat,
                Some(&obj.pos),
                scl * local[0],
                scl * local[1],
                -sgn * obj.size[1],
            );
        }
        add_scl3(
            at3_mut(endverts, 0),
            at3(v, 0),
            at3(res, 0),
            2.0 * obj.size[1],
        );
        return 1;
    }
    0
}
// recover edge of a capsule from witness point
fn capsule_edge_normals(
    res: &mut [f64; 3],
    endverts: &mut [f64; 3],
    obj: &CcdObj<'_>,
    v: &mut [f64; 3],
    w: &[f64; 3],
) -> usize {
    res[0] = obj.mat[2];
    res[1] = obj.mat[5];
    res[2] = obj.mat[8];
    let mut tmp = [0.0; 3];
    sub3(&mut tmp, w, &obj.pos);
    let pr = dot3(&tmp, res);
    let length = obj.size[1];
    add_scl3(v, w, res, -pr - length);
    add_scl3(endverts, w, res, -pr + length);
    1
}
// recover face of a cylinder (approximated as a 16-gon) from its index
fn cylinder_face(res: &mut [f64], obj: &CcdObj<'_>, idx: i32) -> usize {
    const COS_16: [f64; 16] = [
        1.000000000000000,
        0.923879532511287,
        0.707106781186548,
        0.382683432365090,
        0.000000000000000,
        -0.382683432365090,
        -0.707106781186547,
        -0.923879532511287,
        -1.000000000000000,
        -0.923879532511287,
        -0.707106781186548,
        -0.382683432365090,
        0.000000000000000,
        0.382683432365090,
        0.707106781186547,
        0.923879532511287,
    ];
    const SIN_16: [f64; 16] = [
        0.000000000000000,
        0.382683432365090,
        0.707106781186547,
        0.923879532511287,
        1.000000000000000,
        0.923879532511287,
        0.707106781186548,
        0.382683432365090,
        0.000000000000000,
        -0.382683432365090,
        -0.707106781186547,
        -0.923879532511287,
        -1.000000000000000,
        -0.923879532511287,
        -0.707106781186548,
        -0.382683432365090,
    ];
    let sgn = if idx != 0 { -1.0 } else { 1.0 };
    for i in 0..16 {
        let x = COS_16[i] * obj.size[0];
        let y = -SIN_16[i] * obj.size[0] * sgn;
        globalcoord(
            at3_mut(res, 3 * i),
            &obj.mat,
            Some(&obj.pos),
            x,
            y,
            sgn * obj.size[1],
        );
    }
    16
}
// recover face of a box from its index
fn box_face(res: &mut [f64], obj: &CcdObj<'_>, idx: i32) -> usize {
    let mat = &obj.mat;
    let pos = Some(&obj.pos);
    let size = &obj.size;
    match idx {
        0 => {
            // right
            globalcoord(at3_mut(res, 0), mat, pos, size[0], size[1], size[2]);
            globalcoord(at3_mut(res, 3), mat, pos, size[0], size[1], -size[2]);
            globalcoord(at3_mut(res, 6), mat, pos, size[0], -size[1], -size[2]);
            globalcoord(at3_mut(res, 9), mat, pos, size[0], -size[1], size[2]);
            return 4;
        }
        1 => {
            // left
            globalcoord(at3_mut(res, 0), mat, pos, -size[0], size[1], -size[2]);
            globalcoord(at3_mut(res, 3), mat, pos, -size[0], size[1], size[2]);
            globalcoord(at3_mut(res, 6), mat, pos, -size[0], -size[1], size[2]);
            globalcoord(at3_mut(res, 9), mat, pos, -size[0], -size[1], -size[2]);
            return 4;
        }
        2 => {
            // top
            globalcoord(at3_mut(res, 0), mat, pos, -size[0], size[1], -size[2]);
            globalcoord(at3_mut(res, 3), mat, pos, size[0], size[1], -size[2]);
            globalcoord(at3_mut(res, 6), mat, pos, size[0], size[1], size[2]);
            globalcoord(at3_mut(res, 9), mat, pos, -size[0], size[1], size[2]);
            return 4;
        }
        3 => {
            // bottom
            globalcoord(at3_mut(res, 0), mat, pos, -size[0], -size[1], size[2]);
            globalcoord(at3_mut(res, 3), mat, pos, size[0], -size[1], size[2]);
            globalcoord(at3_mut(res, 6), mat, pos, size[0], -size[1], -size[2]);
            globalcoord(at3_mut(res, 9), mat, pos, -size[0], -size[1], -size[2]);
            return 4;
        }
        4 => {
            // front
            globalcoord(at3_mut(res, 0), mat, pos, -size[0], size[1], size[2]);
            globalcoord(at3_mut(res, 3), mat, pos, size[0], size[1], size[2]);
            globalcoord(at3_mut(res, 6), mat, pos, size[0], -size[1], size[2]);
            globalcoord(at3_mut(res, 9), mat, pos, -size[0], -size[1], size[2]);
            return 4;
        }
        5 => {
            // back
            globalcoord(at3_mut(res, 0), mat, pos, size[0], size[1], -size[2]);
            globalcoord(at3_mut(res, 3), mat, pos, -size[0], size[1], -size[2]);
            globalcoord(at3_mut(res, 6), mat, pos, -size[0], -size[1], -size[2]);
            globalcoord(at3_mut(res, 9), mat, pos, size[0], -size[1], -size[2]);
            return 4;
        }
        _ => {}
    }
    0
}

// recover mesh polygon from its index, return number of edges
fn mesh_face(res: &mut [f64], obj: &CcdObj<'_>, idx: i32) -> usize {
    let mat = &obj.mat;
    let pos = Some(&obj.pos);
    let adr = obj.data.mesh.polyvertadr[idx as usize] as usize;
    let mut j = 0;
    let nvert = obj.data.mesh.polyvertnum[idx as usize] as usize;
    let vert = obj.data.mesh.vert;
    let polyvert = &obj.data.mesh.polyvert[adr..];
    for i in (0..nvert).rev() {
        let v = &vert[3 * polyvert[i] as usize..];
        globalcoord(
            at3_mut(res, 3 * j),
            mat,
            pos,
            v[0] as f64,
            v[1] as f64,
            v[2] as f64,
        );
        j += 1;
    }
    nvert
}
// recover plane normal
fn plane_normals(res: &mut [f64], resind: &mut [i32], obj: &CcdObj<'_>) -> usize {
    res[0] = obj.mat[2];
    res[1] = obj.mat[5];
    res[2] = obj.mat[8];
    resind[0] = 0;
    1
}
// recover plane point (0 edges as infinite plane needs no edge clipping)
fn plane_face(res: &mut [f64], obj: &CcdObj<'_>) -> usize {
    copy3(at3_mut(res, 0), &obj.pos);
    0
}
// find two normals that are facing each other within a tolerance, return 1 if found
fn aligned_faces(
    res: &mut [usize; 2],
    v: &[f64],
    nv: usize,
    w: &[f64],
    nw: usize,
    dir: &[f64; 3],
    face_tol: f64,
) -> bool {
    let mut found = false;
    let mut best = -face_tol;
    let dir_tol = EDGE_TOL * norm3(dir);
    for i in 0..nv {
        if dot3(at3(v, 3 * i), dir) >= -dir_tol {
            continue;
        }
        for j in 0..nw {
            if dot3(at3(w, 3 * j), dir) <= dir_tol {
                continue;
            }
            let tmp = dot3(at3(v, 3 * i), at3(w, 3 * j));
            if tmp < best {
                best = tmp;
                res[0] = i;
                res[1] = j;
                found = true;
            }
        }
    }
    found
}
// find two normals that are perpendicular to each other within a tolerance, return 1 if found
fn aligned_face_edge(
    res: &mut [usize; 2],
    edge: &[f64],
    nedge: usize,
    face: &[f64],
    nface: usize,
    dir: &[f64; 3],
) -> bool {
    for i in 0..nface {
        if dot3(at3(face, 3 * i), dir) <= MINVAL {
            continue;
        }
        for j in 0..nedge {
            if dot3(at3(edge, 3 * j), at3(face, 3 * i)).abs() < EDGE_TOL {
                res[0] = j;
                res[1] = i;
                return true;
            }
        }
    }
    false
}
// return number (1, 2 or 3) of dimensions of a simplex; reorder vertices if necessary
fn simplex_dim(vi: &mut [i32; 3], v: &mut [f64; 9]) -> usize {
    if vi[0] != vi[1] {
        return if vi[2] == vi[0] || vi[2] == vi[1] {
            2
        } else {
            3
        };
    }
    if vi[0] != vi[2] {
        vi[1] = vi[2];
        let a = *at3(v, 6);
        copy3(at3_mut(v, 3), &a);
        return 2;
    }
    1
}
// inflate contacts by margin along normal (computed from witness points if normal is NULL)
fn inflate(
    nx: usize,
    x1: &mut [f64],
    x2: &mut [f64],
    mut dist: Option<&mut [f64]>,
    margin1: f64,
    margin2: f64,
    normal: Option<&[f64; 3]>,
) {
    if margin1 == 0.0 && margin2 == 0.0 {
        return;
    }
    let mut n = [0.0; 3];
    if let Some(normal) = normal {
        copy3(&mut n, normal);
    } else {
        sub3(&mut n, at3(x2, 0), at3(x1, 0));
        normalize3(&mut n);
    }
    for i in 0..nx {
        if margin1 != 0.0 {
            let a = *at3(x1, 3 * i);
            add_scl3(at3_mut(x1, 3 * i), &a, &n, margin1);
        }
        if margin2 != 0.0 {
            let a = *at3(x2, 3 * i);
            add_scl3(at3_mut(x2, 3 * i), &a, &n, -margin2);
        }
        if let Some(dist) = dist.as_deref_mut() {
            dist[i] -= margin1 + margin2;
        }
    }
}
// compute possible edge normals of a geom
fn geom_edge_normals(
    res: &mut [f64],
    endverts: &mut [f64],
    dim: usize,
    obj: &CcdObj<'_>,
    triface: &mut [f64; 9],
    trifacei: &[i32; 3],
    x: &[f64; 3],
) -> usize {
    if obj.geom_type == GEOM_BOX {
        return box_edge_normals(res, endverts, dim, obj, triface, trifacei);
    } else if obj.geom_type == GEOM_MESH {
        return mesh_edge_normals(res, endverts, dim, obj, triface, trifacei);
    } else if obj.geom_type == GEOM_CYLINDER {
        return cylinder_edge_normals(res, endverts, dim, obj, triface, trifacei[0]);
    } else if obj.geom_type == GEOM_CAPSULE {
        return capsule_edge_normals(
            at3_mut(res, 0),
            at3_mut(endverts, 0),
            obj,
            at3_mut(triface, 0),
            x,
        );
    }
    0
}

// compute multiple contacts from GJK/EPA support vertices, assuming 1 initial contact in x1, x2
// return number of contacts
pub fn multicontact(
    obj1: &CcdObj<'_>,
    obj2: &CcdObj<'_>,
    x1: &mut [f64],
    x2: &mut [f64],
    mut dist: Option<&mut [f64]>,
    nconmax: usize,
    mut nmeshdegmax: usize,
    mut npolygonmax: usize,
    buffer: &mut CcdBuffer,
) -> usize {
    if (obj1.geom_type != GEOM_PLANE && obj1.nsupp < 1)
        || (obj2.geom_type != GEOM_PLANE && obj2.nsupp < 1)
    {
        return 1;
    }
    if obj1.geom_type == GEOM_MESH && obj1.data.mesh.mesh_polynum == 0 {
        return 1;
    }
    if obj2.geom_type == GEOM_MESH && obj2.data.mesh.mesh_polynum == 0 {
        return 1;
    }
    if obj1.geom_type == GEOM_PLANE || obj2.geom_type == GEOM_PLANE {
        nmeshdegmax = if nmeshdegmax < 1 { 1 } else { nmeshdegmax };
        npolygonmax = if npolygonmax < 1 { 1 } else { npolygonmax };
    }
    if obj1.geom_type == GEOM_CYLINDER || obj2.geom_type == GEOM_CYLINDER {
        nmeshdegmax = if nmeshdegmax < 2 { 2 } else { nmeshdegmax };
        npolygonmax = if npolygonmax < 16 { 16 } else { npolygonmax };
    }
    if obj1.geom_type == GEOM_BOX || obj2.geom_type == GEOM_BOX {
        nmeshdegmax = if nmeshdegmax < 3 { 3 } else { nmeshdegmax };
        npolygonmax = if npolygonmax < 4 { 4 } else { npolygonmax };
    }
    if obj1.geom_type == GEOM_CAPSULE || obj2.geom_type == GEOM_CAPSULE {
        nmeshdegmax = if nmeshdegmax < 2 { 2 } else { nmeshdegmax };
        npolygonmax = if npolygonmax < 2 { 2 } else { npolygonmax };
    }
    let mut triface1i = [0; 3];
    let mut triface2i = [0; 3];
    let mut triface1 = [0.0; 9];
    let mut triface2 = [0.0; 9];
    let mut nface1 = 3;
    let mut nface2 = 3;
    if obj1.geom_type != GEOM_PLANE {
        let s1_1 = if obj1.nsupp > 1 { 1 } else { 0 };
        let s1_2 = if obj1.nsupp > 2 { 2 } else { 0 };
        triface1i[0] = obj1.suppind[0];
        triface1i[1] = obj1.suppind[s1_1];
        triface1i[2] = obj1.suppind[s1_2];
        copy3(at3_mut(&mut triface1, 0), at3(&obj1.suppvert, 0));
        copy3(at3_mut(&mut triface1, 3), at3(&obj1.suppvert, 3 * s1_1));
        copy3(at3_mut(&mut triface1, 6), at3(&obj1.suppvert, 3 * s1_2));
        nface1 = simplex_dim(&mut triface1i, &mut triface1);
    }
    if obj2.geom_type != GEOM_PLANE {
        let s2_1 = if obj2.nsupp > 1 { 1 } else { 0 };
        let s2_2 = if obj2.nsupp > 2 { 2 } else { 0 };
        triface2i[0] = obj2.suppind[0];
        triface2i[1] = obj2.suppind[s2_1];
        triface2i[2] = obj2.suppind[s2_2];
        copy3(at3_mut(&mut triface2, 0), at3(&obj2.suppvert, 0));
        copy3(at3_mut(&mut triface2, 3), at3(&obj2.suppvert, 3 * s2_1));
        copy3(at3_mut(&mut triface2, 6), at3(&obj2.suppvert, 3 * s2_2));
        nface2 = simplex_dim(&mut triface2i, &mut triface2);
    }
    // Reuse every scratch array without shrinking it between queries.
    if buffer.idx1.len() < nmeshdegmax {
        buffer.idx1.resize(nmeshdegmax, 0);
    }
    if buffer.idx2.len() < nmeshdegmax {
        buffer.idx2.resize(nmeshdegmax, 0);
    }
    if buffer.n1.len() < 3 * nmeshdegmax {
        buffer.n1.resize(3 * nmeshdegmax, 0.0);
    }
    if buffer.n2.len() < 3 * nmeshdegmax {
        buffer.n2.resize(3 * nmeshdegmax, 0.0);
    }
    if buffer.edge.len() < 3 * nmeshdegmax {
        buffer.edge.resize(3 * nmeshdegmax, 0.0);
    }
    if buffer.endverts.len() < 3 * nmeshdegmax {
        buffer.endverts.resize(3 * nmeshdegmax, 0.0);
    }
    if buffer.face1.len() < 3 * npolygonmax {
        buffer.face1.resize(3 * npolygonmax, 0.0);
    }
    if buffer.face2.len() < 3 * npolygonmax {
        buffer.face2.resize(3 * npolygonmax, 0.0);
    }
    if buffer.polygon.len() < 16 * npolygonmax {
        buffer.polygon.resize(16 * npolygonmax, 0.0);
    }
    let idx1 = &mut buffer.idx1;
    let idx2 = &mut buffer.idx2;
    let n1 = &mut buffer.n1;
    let n2 = &mut buffer.n2;
    let edge = &mut buffer.edge;
    let endverts = &mut buffer.endverts;
    let face1 = &mut buffer.face1;
    let face2 = &mut buffer.face2;
    let polygon = &mut buffer.polygon;
    let mut nnorms1 = 0;
    let mut nnorms2 = 0;
    let mut dir = [0.0; 3];
    let mut dir_neg = [0.0; 3];
    if obj1.geom_type == GEOM_PLANE {
        dir_neg[0] = obj1.mat[2];
        dir_neg[1] = obj1.mat[5];
        dir_neg[2] = obj1.mat[8];
        scl3(&mut dir, &dir_neg, -1.0);
    } else if obj2.geom_type == GEOM_PLANE {
        dir[0] = obj2.mat[2];
        dir[1] = obj2.mat[5];
        dir[2] = obj2.mat[8];
        scl3(&mut dir_neg, &dir, -1.0);
    } else {
        sub3(&mut dir, at3(x2, 0), at3(x1, 0));
        sub3(&mut dir_neg, at3(x1, 0), at3(x2, 0));
    }
    if obj1.geom_type == GEOM_BOX {
        nnorms1 = box_normals(n1, idx1, nface1, obj1, &triface1i, &dir_neg);
    } else if obj1.geom_type == GEOM_MESH {
        nnorms1 = mesh_normals(n1, idx1, nface1, obj1, &triface1i);
    } else if obj1.geom_type == GEOM_CYLINDER {
        nnorms1 = cylinder_normals(n1, idx1, nface1, obj1, &triface1i, &dir_neg);
    } else if obj1.geom_type == GEOM_PLANE {
        nnorms1 = plane_normals(n1, idx1, obj1);
    }
    if obj2.geom_type == GEOM_BOX {
        nnorms2 = box_normals(n2, idx2, nface2, obj2, &triface2i, &dir);
    } else if obj2.geom_type == GEOM_MESH {
        nnorms2 = mesh_normals(n2, idx2, nface2, obj2, &triface2i);
    } else if obj2.geom_type == GEOM_CYLINDER {
        nnorms2 = cylinder_normals(n2, idx2, nface2, obj2, &triface2i, &dir);
    } else if obj2.geom_type == GEOM_PLANE {
        nnorms2 = plane_normals(n2, idx2, obj2);
    }
    let mut res = [0; 2];
    let mut edgecon1 = false;
    let mut edgecon2 = false;
    let face_tol = if obj1.geom_type == GEOM_PLANE || obj2.geom_type == GEOM_PLANE {
        0.0
    } else {
        FACE_TOL
    };
    let af = aligned_faces(&mut res, n1, nnorms1, n2, nnorms2, &dir, face_tol);
    if !af {
        let mut order = [1, 2];
        if (nface2 < nface1 || nnorms2 == 0) && nface2 < 3 && nnorms1 != 0 {
            order[0] = 2;
            order[1] = 1;
        }
        for k in 0..2 {
            if order[k] == 1 && nface1 < 3 && nnorms2 != 0 {
                let nedge = geom_edge_normals(
                    edge,
                    endverts,
                    nface1,
                    obj1,
                    &mut triface1,
                    &triface1i,
                    at3(x1, 0),
                );
                if aligned_face_edge(&mut res, edge, nedge, n2, nnorms2, &dir) {
                    edgecon1 = true;
                    break;
                }
            } else if order[k] == 2 && nface2 < 3 && nnorms1 != 0 {
                let nedge = geom_edge_normals(
                    edge,
                    endverts,
                    nface2,
                    obj2,
                    &mut triface2,
                    &triface2i,
                    at3(x2, 0),
                );
                if aligned_face_edge(&mut res, edge, nedge, n1, nnorms1, &dir_neg) {
                    edgecon2 = true;
                    break;
                }
            }
        }
        if !edgecon1 && !edgecon2 {
            return 1;
        }
    }
    let i = res[0];
    let j = res[1];
    if edgecon1 {
        copy3(at3_mut(face1, 0), at3(&triface1, 0));
        copy3(at3_mut(face1, 3), at3(endverts, 3 * i));
        nface1 = 2;
    } else {
        if obj1.geom_type == GEOM_BOX {
            let ind = if edgecon2 { idx1[j] } else { idx1[i] };
            nface1 = box_face(face1, obj1, ind);
        } else if obj1.geom_type == GEOM_MESH {
            let ind = if edgecon2 { idx1[j] } else { idx1[i] };
            nface1 = mesh_face(face1, obj1, ind);
        } else if obj1.geom_type == GEOM_CYLINDER {
            let ind = if edgecon2 { idx1[j] } else { idx1[i] };
            nface1 = cylinder_face(face1, obj1, ind);
        } else if obj1.geom_type == GEOM_PLANE {
            nface1 = plane_face(face1, obj1);
        }
    }
    if edgecon2 {
        copy3(at3_mut(face2, 0), at3(&triface2, 0));
        copy3(at3_mut(face2, 3), at3(endverts, 3 * i));
        nface2 = 2;
    } else {
        if obj2.geom_type == GEOM_BOX {
            nface2 = box_face(face2, obj2, idx2[j]);
        } else if obj2.geom_type == GEOM_MESH {
            nface2 = mesh_face(face2, obj2, idx2[j]);
        } else if obj2.geom_type == GEOM_CYLINDER {
            nface2 = cylinder_face(face2, obj2, idx2[j]);
        } else if obj2.geom_type == GEOM_PLANE {
            nface2 = plane_face(face2, obj2);
        }
    }
    let mut wit_dir = [0.0; 3];
    let mut normal = [0.0; 3];
    let margin1 = if obj1.geom >= 0 && obj1.geom_type != GEOM_CAPSULE {
        0.5 * obj1.margin
    } else {
        0.0
    };
    let margin2 = if obj2.geom >= 0 && obj2.geom_type != GEOM_CAPSULE {
        0.5 * obj2.margin
    } else {
        0.0
    };
    let margin = margin1 + margin2;
    let nx;
    if edgecon1 || obj2.geom_type == GEOM_PLANE {
        scl3(&mut wit_dir, at3(n2, 3 * j), -1.0);
        copy3(&mut normal, &wit_dir);
        nx = polygon_clip(
            x2,
            x1,
            dist.as_deref_mut(),
            nconmax,
            face2,
            nface2,
            face1,
            nface1,
            at3(n2, 3 * j),
            &wit_dir,
            polygon,
            npolygonmax,
            margin,
        );
    } else if edgecon2 || obj1.geom_type == GEOM_PLANE {
        let ind = if edgecon2 { j } else { i };
        scl3(&mut wit_dir, at3(n1, 3 * ind), -1.0);
        copy3(&mut normal, at3(n1, 3 * ind));
        nx = polygon_clip(
            x1,
            x2,
            dist.as_deref_mut(),
            nconmax,
            face1,
            nface1,
            face2,
            nface2,
            at3(n1, 3 * ind),
            &wit_dir,
            polygon,
            npolygonmax,
            margin,
        );
    } else {
        copy3(&mut wit_dir, at3(n2, 3 * j));
        scl3(&mut normal, at3(n2, 3 * j), -1.0);
        nx = polygon_clip(
            x1,
            x2,
            dist.as_deref_mut(),
            nconmax,
            face1,
            nface1,
            face2,
            nface2,
            at3(n1, 3 * i),
            &wit_dir,
            polygon,
            npolygonmax,
            margin,
        );
    }
    if nx > 0 {
        inflate(nx, x1, x2, dist, margin1, margin2, Some(&normal));
        return nx;
    }
    1
}

// Byte size of the reference C workspace; Rust callers supply reusable CcdBuffer storage.
// return size in bytes of the buffer needed for mjc_ccd for a given number of iterations
pub fn ccd_size(mut npolygonmax: usize, mut nmeshdegmax: usize, iterations: usize) -> usize {
    let maxedges = 24 + if iterations > 35 { iterations - 35 } else { 0 };
    let epa_size = align8(std::mem::size_of::<Vertex>() * (5 + iterations))
        + align8(std::mem::size_of::<Face>() * 6 * iterations)
        + align8(std::mem::size_of::<*const Face>() * 6 * iterations)
        + align8(std::mem::size_of::<i32>() * maxedges)
        + align8(std::mem::size_of::<i32>() * maxedges);
    npolygonmax = if npolygonmax < 16 { 16 } else { npolygonmax };
    nmeshdegmax = if nmeshdegmax < 3 { 3 } else { nmeshdegmax };
    let multiccd_size = align8(std::mem::size_of::<f64>() * 3 * 2 * npolygonmax)
        + align8(std::mem::size_of::<f64>() * 3 * 2 * npolygonmax)
        + align8(std::mem::size_of::<f64>() * 3 * npolygonmax)
        + align8(std::mem::size_of::<f64>() * npolygonmax)
        + align8(std::mem::size_of::<i32>() * nmeshdegmax)
        + align8(std::mem::size_of::<i32>() * nmeshdegmax)
        + align8(std::mem::size_of::<f64>() * 3 * nmeshdegmax)
        + align8(std::mem::size_of::<f64>() * 3 * nmeshdegmax)
        + align8(std::mem::size_of::<f64>() * 3 * nmeshdegmax)
        + align8(std::mem::size_of::<f64>() * 3 * nmeshdegmax)
        + align8(std::mem::size_of::<f64>() * 3 * npolygonmax)
        + align8(std::mem::size_of::<f64>() * 3 * npolygonmax);
    if epa_size > multiccd_size {
        epa_size
    } else {
        multiccd_size
    }
}

// general convex collision detection
pub fn ccd(
    config: &mut CcdConfig<'_>,
    status: &mut CcdStatus,
    obj1: &mut CcdObj<'_>,
    obj2: &mut CcdObj<'_>,
) -> f64 {
    (obj1.center)(at3_mut(&mut status.x1, 0), obj1);
    (obj2.center)(at3_mut(&mut status.x2, 0), obj2);
    status.gjk_iterations = 0;
    status.epa_iterations = 0;
    status.epa_status = EPA_NOCONTACT;
    status.tolerance = config.tolerance;
    status.max_iterations = config.max_iterations;
    let maxcon = status.dist.len();
    status.max_contacts = if config.max_contacts < maxcon {
        config.max_contacts
    } else {
        maxcon
    };
    status.dist_cutoff = config.dist_cutoff;
    if obj1.geom_type == GEOM_SPHERE
        || obj2.geom_type == GEOM_SPHERE
        || obj1.geom_type == GEOM_CAPSULE
        || obj2.geom_type == GEOM_CAPSULE
        || (obj1.geom >= 0 && obj2.geom >= 0 && (obj1.margin > 0.0 || obj2.margin > 0.0))
    {
        let support1 = obj1.support;
        let support2 = obj2.support;
        let mut full_margin1 = 0.0;
        let mut full_margin2 = 0.0;
        let margin1 = obj1.margin;
        let margin2 = obj2.margin;
        if obj1.geom >= 0 {
            full_margin1 = 0.5 * margin1;
            obj1.margin = 0.0;
        }
        if obj1.geom_type == GEOM_SPHERE {
            full_margin1 += obj1.size[0];
            obj1.support = CcdSupport::Point;
        } else if obj1.geom_type == GEOM_CAPSULE {
            full_margin1 += obj1.size[0];
            obj1.support = CcdSupport::Line;
        }
        if obj2.geom >= 0 {
            full_margin2 = 0.5 * margin2;
            obj2.margin = 0.0;
        }
        if obj2.geom_type == GEOM_SPHERE {
            full_margin2 += obj2.size[0];
            obj2.support = CcdSupport::Point;
        } else if obj2.geom_type == GEOM_CAPSULE {
            full_margin2 += obj2.size[0];
            obj2.support = CcdSupport::Line;
        }
        status.dist_cutoff += full_margin1 + full_margin2;
        gjk_internal(status, obj1, obj2);
        status.dist_cutoff = config.dist_cutoff;
        obj1.margin = margin1;
        obj2.margin = margin2;
        obj1.support = support1;
        obj2.support = support2;
        if status.dist[0] > status.tolerance {
            inflate(
                status.nx,
                &mut status.x1,
                &mut status.x2,
                Some(&mut status.dist),
                full_margin1,
                full_margin2,
                None,
            );
            if status.dist[0] > status.dist_cutoff {
                status.dist[0] = MAX_LIMIT;
            } else if config.max_contacts > 1
                && status.dist[0] < 0.0
                && config.buffer.is_some()
                && obj1.nsupp > 0
            {
                status.nx = multicontact(
                    obj1,
                    obj2,
                    &mut status.x1,
                    &mut status.x2,
                    Some(&mut status.dist),
                    status.max_contacts,
                    config.nmeshdegmax,
                    config.npolygonmax,
                    config.buffer.as_deref_mut().unwrap(),
                );
                let mut min_dist = status.dist[0];
                for i in 1..status.nx {
                    if status.dist[i] < min_dist {
                        min_dist = status.dist[i];
                    }
                }
                return min_dist;
            }
            return status.dist[0];
        }
        if config.max_contacts == 0 {
            status.nx = 0;
            status.dist[0] = 0.0;
            return 0.0;
        }
        status.gjk_iterations = 0;
        (obj1.center)(at3_mut(&mut status.x1, 0), obj1);
        (obj2.center)(at3_mut(&mut status.x2, 0), obj2);
    }
    gjk_internal(status, obj1, obj2);
    if config.max_contacts == 0 {
        return status.dist[0];
    }
    if status.dist[0] <= config.tolerance
        && obj1.nsupp > 1
        && config.buffer.is_some()
        && !status.separated
    {
        let gjk_dist = status.dist[0];
        status.dist[0] = 0.0;
        let buffer = config.buffer.as_deref_mut().unwrap();
        let pt = &mut buffer.polytope;
        pt.nfaces = 0;
        pt.nmap = 0;
        pt.nverts = 0;
        pt.horizon.nedges = 0;
        let n = config.max_iterations;
        pt.maxfaces = 6 * n;
        pt.horizon.maxedges = 24 + if n > 35 { n - 35 } else { 0 };
        if pt.verts.len() < 5 + n {
            pt.verts.resize(5 + n, Vertex::default());
        }
        if pt.faces.len() < 6 * n {
            pt.faces.resize(6 * n, Face::default());
        }
        if pt.map.len() < 6 * n {
            pt.map.resize(6 * n, 0);
        }
        if pt.horizon.indices.len() < pt.horizon.maxedges {
            pt.horizon.indices.resize(pt.horizon.maxedges, 0);
        }
        if pt.horizon.edges.len() < pt.horizon.maxedges {
            pt.horizon.edges.resize(pt.horizon.maxedges, 0);
        }
        let ret;
        if obj1.nsupp == 2 {
            ret = polytope2(pt, status, obj1, obj2);
        } else if obj1.nsupp == 3 {
            ret = polytope3(pt, status, obj1, obj2, true);
        } else {
            ret = polytope4(pt, status, obj1, obj2);
        }
        status.epa_status = ret;
        if ret == 0 {
            let face = epa(status, pt, obj1, obj2);
            if config.max_contacts > 1 && face.is_some() {
                status.nx = multicontact(
                    obj1,
                    obj2,
                    &mut status.x1,
                    &mut status.x2,
                    Some(&mut status.dist),
                    status.max_contacts,
                    config.nmeshdegmax,
                    config.npolygonmax,
                    buffer,
                );
            }
            if face.is_none() && config.dist_cutoff > 0.0 && status.epa_status == EPA_NOCONTACT {
                status.nx = 1;
                status.dist[0] = gjk_dist;
            }
        }
    }
    let mut min_dist = status.dist[0];
    for i in 1..status.nx {
        if status.dist[i] < min_dist {
            min_dist = status.dist[i];
        }
    }
    min_dist
}

// warm-started GJK distance query (no EPA), returns positive distance or 0 if penetrating
pub fn gjk(
    obj1: &mut CcdObj<'_>,
    obj2: &mut CcdObj<'_>,
    x1: &mut [f64; 3],
    x2: &mut [f64; 3],
    dist_cutoff: f64,
    tolerance: f64,
    max_iterations: usize,
) -> f64 {
    let mut status = CcdStatus::default();
    crate::util_blas::copy3(at3_mut(&mut status.x1, 0), x1);
    crate::util_blas::copy3(at3_mut(&mut status.x2, 0), x2);
    status.dist_cutoff = dist_cutoff;
    status.tolerance = tolerance;
    status.max_iterations = max_iterations;
    gjk_internal(&mut status, obj1, obj2);
    crate::util_blas::copy3(x1, at3(&status.x1, 0));
    crate::util_blas::copy3(x2, at3(&status.x2, 0));
    status.dist[0]
}
