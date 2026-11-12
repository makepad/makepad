//! Collision counts, filters, broad phase, BVH traversal and contact dispatch.
//!
//! Translated from `src/engine/engine_collision_driver.c` and
//! `src/engine/engine_collision_driver.h` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust; arena and stack storage use retained vectors; a sleeping body skips a
//! mocap body that holds still as it skips the world.

use crate::collision_box::{box_box, capsule_box, sphere_box};
use crate::collision_continuous::{advance_geom_pair, geom_step_motion, geom_supported};
use crate::collision_convex::{convex, convex_hfield, plane_convex};
use crate::collision_gjk::ccd_size;
use crate::collision_primitive::*;
use crate::collision_sdf::{hfield_sdf, mesh_sdf, sdf};
use crate::core_constraint::{assign_friction, assign_imp, assign_margin, assign_ref, contact_param};
use crate::core_util::geom_surface_velocity;
use crate::data::{Contact, Data, PreContact};
use crate::inline::{self as inline, at3, at9};
use crate::memory::clear_efc;
use crate::model::Model;
use crate::sleep::sleep_state;
use crate::sort::sort;
use crate::memory::Scratch;
use crate::thread::{num_thread, run_parts};
use crate::types::*;
use crate::util_blas as u;
use crate::util_errmem::warning;
use crate::util_misc::{clip, max, min};
use crate::util_spatial::{eig3tol, make_frame};

/// A collision function of COLLISIONFUNC (C: `mjfCollision`, one variant per function). C compares
/// the table's function pointers (mj_geomDistance, the CCD pair test); Rust gives a function no single
/// address (codegen units can hold copies), so the table holds this tag and `call` dispatches.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CollisionFn { PlaneSphere, PlaneCapsule, PlaneConvex, PlaneCylinder, PlaneBox, ConvexHfield, HfieldSdf, SphereSphere, SphereCapsule, Convex, SphereCylinder, SphereBox, Sdf, CapsuleCapsule, CapsuleBox, BoxBox, MeshSdf }
impl CollisionFn {
    /// Collides geoms g1 and g2 into `con`, returning the number of contacts; `t` is the calling task's.
    pub fn call(self, m: &Model, d: &Data, t: &mut CollisionTask, con: &mut [PreContact], g1: i32, g2: i32, margin: f64) -> usize {
        match self {
            CollisionFn::PlaneSphere => plane_sphere(m, d, con, g1, g2, margin),
            CollisionFn::PlaneCapsule => plane_capsule(m, d, con, g1, g2, margin),
            CollisionFn::PlaneConvex => plane_convex(m, d, con, g1, g2, margin),
            CollisionFn::PlaneCylinder => plane_cylinder(m, d, con, g1, g2, margin),
            CollisionFn::PlaneBox => plane_box(m, d, con, g1, g2, margin),
            CollisionFn::ConvexHfield => convex_hfield(m, d, &mut t.scratch, con, g1, g2, margin),
            CollisionFn::HfieldSdf => hfield_sdf(m, d, con, g1, g2, margin),
            CollisionFn::SphereSphere => sphere_sphere(m, d, con, g1, g2, margin),
            CollisionFn::SphereCapsule => sphere_capsule(m, d, con, g1, g2, margin),
            CollisionFn::Convex => convex(m, d, &mut t.scratch, con, g1, g2, margin),
            CollisionFn::SphereCylinder => sphere_cylinder(m, d, con, g1, g2, margin),
            CollisionFn::SphereBox => sphere_box(m, d, con, g1, g2, margin),
            CollisionFn::Sdf => sdf(m, d, t, con, g1, g2, margin),
            CollisionFn::CapsuleCapsule => capsule_capsule(m, d, con, g1, g2, margin),
            CollisionFn::CapsuleBox => capsule_box(m, d, con, g1, g2, margin),
            CollisionFn::BoxBox => box_box(m, d, con, g1, g2, margin),
            CollisionFn::MeshSdf => mesh_sdf(m, d, t, con, g1, g2, margin),
        }
    }
}

/// What one narrowphase task changes besides its pairs' contacts (the reference's per-thread stack and
/// the Data its colliders share): its pooled vectors, and what the SDF colliders record, which
/// [`merge_collision_tasks`] moves into Data after the phase in pair order, as one task in order leaves it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollisionTask {
    pub scratch: Scratch,
    /// the field pairs the task collided, and the seeds each leaves for its next collision (Data.sdf_seeds)
    pub(crate) sdf_ran: Vec<(i32, i32)>,
    pub(crate) sdf_seeds: Vec<(i32, i32, [f64; 3])>,
    /// walks the bound on their steps stopped (Data.sdf_walks_bounded), and the first one's geoms
    pub(crate) sdf_walks_bounded: u64,
    pub(crate) sdf_first_bounded: Option<(i32, i32)>,
    /// BVH nodes the mesh-field collider found active (Data.bvh_active)
    pub(crate) bvh_marks: Vec<usize>,
}

/// Moves what `tasks` recorded into `d`: the seeds of the field pairs they collided give way to the new
/// ones, appended in task order; the bounded walks add up, logged at the first ever; the marked BVH nodes.
pub(crate) fn merge_collision_tasks(m: &Model, d: &mut Data, tasks: &mut [CollisionTask]) {
    if tasks.iter().any(|t| !t.sdf_ran.is_empty()) {
        d.sdf_seeds.retain(|&(a, b, _)| !tasks.iter().any(|t| t.sdf_ran.contains(&(a, b))));
        for t in tasks.iter_mut() {
            d.sdf_seeds.extend_from_slice(&t.sdf_seeds);
            t.sdf_ran.clear();
            t.sdf_seeds.clear();
        }
    }
    for t in tasks.iter_mut() {
        if let Some((g1, g2)) = t.sdf_first_bounded.take() {
            if d.sdf_walks_bounded == 0 {
                crate::util_errmem::warning_text(&format!(
                    "SDF collision: a walk of geoms {g1} and {g2} took sdf_iterations ({}) steps without settling; it carries on in the next collision. Time = {:.4}.",
                    m.opt.sdf_iterations, d.time
                ));
            }
        }
        d.sdf_walks_bounded += t.sdf_walks_bounded;
        t.sdf_walks_bounded = 0;
        for &node in &t.bvh_marks {
            d.bvh_active[node] = 1;
        }
        t.bvh_marks.clear();
    }
}

pub const COLLISIONFUNC: [[Option<CollisionFn>; NGEOMTYPES as usize]; NGEOMTYPES as usize] = [
    [None, None, Some(CollisionFn::PlaneSphere), Some(CollisionFn::PlaneCapsule), Some(CollisionFn::PlaneConvex), Some(CollisionFn::PlaneCylinder), Some(CollisionFn::PlaneBox), Some(CollisionFn::PlaneConvex), Some(CollisionFn::PlaneConvex)],
    [None, None, Some(CollisionFn::ConvexHfield), Some(CollisionFn::ConvexHfield), Some(CollisionFn::ConvexHfield), Some(CollisionFn::ConvexHfield), Some(CollisionFn::ConvexHfield), Some(CollisionFn::ConvexHfield), Some(CollisionFn::HfieldSdf)],
    [None, None, Some(CollisionFn::SphereSphere), Some(CollisionFn::SphereCapsule), Some(CollisionFn::Convex), Some(CollisionFn::SphereCylinder), Some(CollisionFn::SphereBox), Some(CollisionFn::Convex), Some(CollisionFn::Sdf)],
    [None, None, None, Some(CollisionFn::CapsuleCapsule), Some(CollisionFn::Convex), Some(CollisionFn::Convex), Some(CollisionFn::CapsuleBox), Some(CollisionFn::Convex), Some(CollisionFn::Sdf)],
    [None, None, None, None, Some(CollisionFn::Convex), Some(CollisionFn::Convex), Some(CollisionFn::Convex), Some(CollisionFn::Convex), Some(CollisionFn::Sdf)],
    [None, None, None, None, None, Some(CollisionFn::Convex), Some(CollisionFn::Convex), Some(CollisionFn::Convex), Some(CollisionFn::Sdf)],
    [None, None, None, None, None, None, Some(CollisionFn::BoxBox), Some(CollisionFn::Convex), Some(CollisionFn::Sdf)],
    [None, None, None, None, None, None, None, Some(CollisionFn::Convex), Some(CollisionFn::MeshSdf)],
    [None, None, None, None, None, None, None, None, Some(CollisionFn::Sdf)],
];

// has_margin is ignored in C.
pub fn max_contact(m: &Model, g1: i32, g2: i32, _has_margin: bool) -> usize {
    let type1 = m.geom_type[g1 as usize];
    let type2 = m.geom_type[g2 as usize];
    if type1 == GEOM_SDF || type2 == GEOM_SDF {
        if type1 == GEOM_PLANE || type2 == GEOM_PLANE { return 4; }
        if type1 == GEOM_MESH || type2 == GEOM_MESH { return MAXCONPAIR; }
        if type1 == GEOM_HFIELD || type2 == GEOM_HFIELD { return 0; }
        return MAXCONPAIR.min(m.opt.sdf_initpoints.max(0) as usize);
    }
    if type1 == GEOM_HFIELD || type2 == GEOM_HFIELD {
        let type_ = if type1 == GEOM_HFIELD { type2 } else { type1 };
        return if type_ != GEOM_PLANE && type_ != GEOM_HFIELD { MAXCONPAIR } else { 0 };
    }
    if type1 == GEOM_SPHERE || type1 == GEOM_ELLIPSOID || type2 == GEOM_SPHERE || type2 == GEOM_ELLIPSOID { return 1; }
    if type1 == GEOM_BOX && type2 == GEOM_BOX { return 8; }
    if type1 == GEOM_CAPSULE && type2 == GEOM_CAPSULE { return 2; }
    if (type1 == GEOM_CAPSULE && type2 == GEOM_BOX) || (type1 == GEOM_BOX && type2 == GEOM_CAPSULE) { return 4; }
    let hasface1 = type1 == GEOM_BOX || type1 == GEOM_MESH || type1 == GEOM_CYLINDER;
    let hasface2 = type2 == GEOM_BOX || type2 == GEOM_MESH || type2 == GEOM_CYLINDER;
    let hasedge1 = type1 == GEOM_CAPSULE;
    let hasedge2 = type2 == GEOM_CAPSULE;
    if type1 == GEOM_PLANE || type2 == GEOM_PLANE {
        if hasface1 || hasface2 { return 4; }
        if hasedge1 || hasedge2 { return 2; }
        return 0;
    }
    let is_multiccd = m.opt.disableflags & DSBL_MULTICCD == 0;
    if !is_multiccd { return 1; }
    if m.opt.disableflags & DSBL_NATIVECCD != 0 { return 5; }
    if hasface1 && hasface2 { return 4; }
    if (hasedge1 && hasface2) || (hasface1 && hasedge2) || (hasedge1 && hasedge2) { return 2; }
    1
}

fn get_margin(m: &Model, g1: i32, g2: i32, ipair: i32) -> f64 {
    if ipair >= 0 { return assign_margin(m, m.pair_margin[ipair as usize]); }
    assign_margin(m, m.geom_margin[g1 as usize] + m.geom_margin[g2 as usize])
}

fn get_gap(m: &Model, g1: i32, g2: i32, ipair: i32) -> f64 {
    if ipair >= 0 { return m.pair_gap[ipair as usize]; }
    m.geom_gap[g1 as usize] + m.geom_gap[g2 as usize]
}

fn reset_arena(d: &mut Data) {
    // Like the reference's arena the contact vector keeps its length: the contacts past ncon hold what an
    // earlier step left, and every contact added is written whole before it is read.
    d.parena = d.ncon as usize * std::mem::size_of::<Contact>();
}

// alignArena is unnecessary for separately allocated, typed arena vectors.

fn plane_geom_dist(_m: &Model, d: &Data, g1: i32, g2: i32) -> f64 {
    let mat1 = at9(&d.geom_xmat, 9 * g1 as usize);
    let norm = [mat1[2], mat1[5], mat1[8]];
    let mut dif = [0.0; 3];
    u::sub3(&mut dif, at3(&d.geom_xpos, 3 * g2 as usize), at3(&d.geom_xpos, 3 * g1 as usize));
    u::dot3(&dif, &norm)
}

fn has_plane(m: &Model, body: usize) -> bool {
    let start = m.body_geomadr[body];
    let end = m.body_geomadr[body] + m.body_geomnum[body];
    for g in start..end { if m.geom_type[g as usize] == GEOM_PLANE { return true; } }
    false
}

fn filter_bitmask(contype1: i32, conaffinity1: i32, contype2: i32, conaffinity2: i32) -> bool {
    contype1 & conaffinity2 == 0 && contype2 & conaffinity1 == 0
}

// filterBox and filterSphereBox are only used by the omitted flex paths.

fn speculative_enabled(m: &Model) -> bool {
    m.opt.enableflags & ENBL_SPECULATIVE != 0 && m.opt.disableflags & DSBL_NATIVECCD == 0
}
fn geom_min_size(m: &Model, g: i32) -> f64 {
    let size = at3(&m.geom_aabb, 6 * g as usize + 3);
    min(size[0], min(size[1], size[2]))
}
fn pair_min_size(m: &Model, g1: i32, g2: i32) -> f64 {
    if m.geom_type[g1 as usize] == GEOM_PLANE { return geom_min_size(m, g2); }
    if m.geom_type[g2 as usize] == GEOM_PLANE { return geom_min_size(m, g1); }
    min(geom_min_size(m, g1), geom_min_size(m, g2))
}
fn geom_step_sweep(m: &Model, d: &Data, g: i32, min_size: f64, mut dx: Option<&mut [f64; 3]>, mut dw: Option<&mut [f64; 3]>) -> f64 {
    if let Some(dx) = dx.as_deref_mut() { inline::zero3(dx); }
    if let Some(dw) = dw.as_deref_mut() { inline::zero3(dw); }
    if g < 0 || m.geom_type[g as usize] == GEOM_PLANE || geom_supported(m.geom_type[g as usize]) == 0 { return 0.0; }
    let mut dx_copy = [0.0; 3];
    let mut dw_copy = [0.0; 3];
    let s = geom_step_motion(m, d, g, m.opt.timestep, &mut dx_copy, &mut dw_copy);
    if s < min_size { return 0.0; }
    if let Some(dx) = dx { inline::copy3(dx, &dx_copy); }
    if let Some(dw) = dw { inline::copy3(dw, &dw_copy); }
    s
}
fn body_pair_has_fast_geom(m: &Model, d: &Data, b1: usize, b2: usize) -> bool {
    let adr1 = m.body_geomadr[b1]; let num1 = m.body_geomnum[b1];
    let adr2 = m.body_geomadr[b2]; let num2 = m.body_geomnum[b2];
    let mut min_size = MAXVAL;
    for i in 0..num1 {
        let g = adr1 + i;
        if m.geom_type[g as usize] != GEOM_PLANE && geom_supported(m.geom_type[g as usize]) != 0 { min_size = min(min_size, geom_min_size(m, g)); }
    }
    for i in 0..num2 {
        let g = adr2 + i;
        if m.geom_type[g as usize] != GEOM_PLANE && geom_supported(m.geom_type[g as usize]) != 0 { min_size = min(min_size, geom_min_size(m, g)); }
    }
    if min_size >= MAXVAL { return false; }
    for i in 0..num1 { if geom_step_sweep(m, d, adr1 + i, min_size, None, None) > 0.0 { return true; } }
    for i in 0..num2 { if geom_step_sweep(m, d, adr2 + i, min_size, None, None) > 0.0 { return true; } }
    false
}
fn filter_sphere(m: &Model, d: &Data, g1: i32, g2: i32, margin: f64) -> bool {
    let mut dx1 = [0.0; 3]; let mut dx2 = [0.0; 3];
    let mut s1 = 0.0; let mut s2 = 0.0;
    if speculative_enabled(m) {
        let min_size = pair_min_size(m, g1, g2);
        s1 = geom_step_sweep(m, d, g1, min_size, Some(&mut dx1), None);
        s2 = geom_step_sweep(m, d, g2, min_size, Some(&mut dx2), None);
    }
    let g1 = g1 as usize; let g2 = g2 as usize;
    if m.geom_rbound[g1] > 0.0 && m.geom_rbound[g2] > 0.0 {
        let bound = m.geom_rbound[g1] + m.geom_rbound[g2] + margin;
        let pos1 = at3(&d.geom_xpos, 3*g1); let pos2 = at3(&d.geom_xpos, 3*g2);
        let diff = [pos2[0]-pos1[0], pos2[1]-pos1[1], pos2[2]-pos1[2]];
        if s1 == 0.0 && s2 == 0.0 { return diff[0]*diff[0] + diff[1]*diff[1] + diff[2]*diff[2] > bound*bound; }
        let mut v = [0.0; 3]; let mut closest = [0.0; 3];
        u::sub3(&mut v, &dx2, &dx1);
        let vv = u::dot3(&v, &v);
        let tau = if vv > MINVAL { clip(-u::dot3(&diff, &v) / vv, 0.0, 1.0) } else { 0.0 };
        inline::add_scl3(&mut closest, &diff, &v, tau);
        return u::dot3(&closest, &closest) > bound * bound;
    }
    if m.geom_type[g1] == GEOM_PLANE && m.geom_rbound[g2] > 0.0 {
        let mut dist = plane_geom_dist(m, d, g1 as i32, g2 as i32);
        if s2 > 0.0 {
            let mat1 = at9(&d.geom_xmat, 9*g1);
            let n1 = [mat1[2], mat1[5], mat1[8]];
            dist = min(dist, dist + u::dot3(&n1, &dx2));
        }
        if dist > margin + m.geom_rbound[g2] { return true; }
    }
    if m.geom_type[g2] == GEOM_PLANE && m.geom_rbound[g1] > 0.0 {
        let mut dist = plane_geom_dist(m, d, g2 as i32, g1 as i32);
        if s1 > 0.0 {
            let mat2 = at9(&d.geom_xmat, 9*g2);
            let n2 = [mat2[2], mat2[5], mat2[8]];
            dist = min(dist, dist + u::dot3(&n2, &dx1));
        }
        if dist > margin + m.geom_rbound[g1] { return true; }
    }
    false
}
// static1/2: the weld is the world's or a mocap body's that holds still.
fn filter_body_pair(weldbody1: i32, weldparent1: i32, asleep1: bool, static1: bool, dofnum1: i32, weldbody2: i32, weldparent2: i32, asleep2: bool, static2: bool, dofnum2: i32, dsbl_filterparent: bool) -> bool {
    if weldbody1 == weldbody2 { return true; }
    if dofnum1 == 0 && dofnum2 == 0 { return true; }
    if asleep1 && asleep2 { return true; }
    if (asleep1 && static2) || (asleep2 && static1) { return true; }
    if (!dsbl_filterparent && weldbody1 != 0 && weldbody2 != 0) && (weldbody1 == weldparent2 || weldbody2 == weldparent1) { return true; }
    false
}
// The world's weld, or a weld that is static to sleep (a mocap body holding still).
fn still_weld(m: &Model, d: &Data, weld: i32, sleep_filter: bool) -> bool {
    weld == 0 || (sleep_filter && m.body_treeid[weld as usize] < 0 && d.body_awake[weld as usize] == S_STATIC)
}
fn can_collide(m: &Model, bf: usize) -> bool {
    if bf < m.nbody { m.body_contype[bf] != 0 || m.body_conaffinity[bf] != 0 }
    else { let f = bf-m.nbody; m.flex_contype[f] != 0 || m.flex_conaffinity[f] != 0 }
}
fn can_collide2(m: &Model, bf1: usize, bf2: usize) -> bool {
    let nbody = m.nbody;
    let contype1 = if bf1 < nbody { m.body_contype[bf1] } else { m.flex_contype[bf1-nbody] };
    let conaffinity1 = if bf1 < nbody { m.body_conaffinity[bf1] } else { m.flex_conaffinity[bf1-nbody] };
    let contype2 = if bf2 < nbody { m.body_contype[bf2] } else { m.flex_contype[bf2-nbody] };
    let conaffinity2 = if bf2 < nbody { m.body_conaffinity[bf2] } else { m.flex_conaffinity[bf2-nbody] };
    !filter_bitmask(contype1, conaffinity1, contype2, conaffinity2)
}
fn has_awake_dofs(m: &Model, d: &Data, bf: usize, sleep_filter: bool) -> bool {
    if bf < m.nbody { return m.body_treeid[bf] >= 0 && (!sleep_filter || d.body_awake[bf] as i32 == S_AWAKE); }
    let state = sleep_state(m, d, OBJ_FLEX, (bf-m.nbody) as i32);
    state != S_STATIC && (!sleep_filter || state == S_AWAKE)
}
pub fn is_elem_active(m: &Model, f: usize, e: usize) -> bool {
    if m.flex_dim[f] < 3 { true } else { m.flex_elemlayer[m.flex_elemadr[f] as usize + e] < m.flex_activelayers[f] }
}

// Only the geom-geom union member is stored: flex candidate creation is unported.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Pair {
    conpos: usize,
    group: i32,
    g1: i32,
    g2: i32,
    ipair: i32,
    spec: i32,
}
fn default_pair(pair: &mut Pair) {
    pair.group = -1;
    pair.g1 = -1; pair.g2 = -1; pair.ipair = -1; pair.spec = -1;
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Packed32 { pub hi: u16, pub lo: u16 }
fn pair_objects(p: &Pair, obj1: &mut i32, obj2: &mut i32) { *obj1 = p.g1; *obj2 = p.g2; }
fn pair_compare(p1: &Pair, p2: &Pair) -> i32 {
    if p1.group >= 0 && p2.group >= 0 && p1.group != p2.group { return if p1.group < p2.group { -1 } else { 1 }; }
    // All translated pairs are mjCPAIR_GEOM_GEOM.
    let (mut obj1_1, mut obj1_2, mut obj2_1, mut obj2_2) = (0,0,0,0);
    pair_objects(p1, &mut obj1_1, &mut obj1_2); pair_objects(p2, &mut obj2_1, &mut obj2_2);
    if obj1_1 < obj2_1 { return -1; } if obj1_1 > obj2_1 { return 1; }
    if obj1_2 < obj2_2 { return -1; } if obj1_2 > obj2_2 { return 1; }
    0
}
fn push_pair_arena(pairs: &mut Vec<Pair>, pair: &Pair, npair: usize) -> usize {
    pairs.push(*pair); npair + 1
}
fn push_geom_geom(m: &Model, pairs: &mut Vec<Pair>, mut g1: i32, mut g2: i32, ipair: i32, npair: usize) -> usize {
    if m.geom_type[g1 as usize] > m.geom_type[g2 as usize] { std::mem::swap(&mut g1, &mut g2); }
    let mut pair = Pair::default(); default_pair(&mut pair);
    pair.g1 = g1; pair.g2 = g2; pair.ipair = ipair; pair.spec = 0;
    push_pair_arena(pairs, &pair, npair)
}
// flexReducible, geomGroup, pushGeomFlex, pushGeomElem, pushElemElem and
// pushSelfElemElem are omitted with their flex-only callers.
fn filter_collision_pair(m: &Model, d: &Data, g1: i32, g2: i32, ipair: i32, merged: bool, startadr: usize, pairadr: usize) -> bool {
    if merged {
        for k in startadr..pairadr {
            if (m.pair_geom1[k] == g1 && m.pair_geom2[k] == g2) || (m.pair_geom1[k] == g2 && m.pair_geom2[k] == g1) { return false; }
        }
    }
    if ipair >= 0 && m.opt.enableflags & ENBL_SLEEP != 0 {
        let b1 = m.geom_bodyid[g1 as usize] as usize; let b2 = m.geom_bodyid[g2 as usize] as usize;
        if d.body_awake[b1] as i32 != S_AWAKE && d.body_awake[b2] as i32 != S_AWAKE { return false; }
    }
    // mjcb_contactfilter is unset.
    if ipair < 0 && filter_bitmask(m.geom_contype[g1 as usize], m.geom_conaffinity[g1 as usize], m.geom_contype[g2 as usize], m.geom_conaffinity[g2 as usize]) { return false; }
    let margin = get_margin(m, g1, g2, ipair); let gap = get_gap(m, g1, g2, ipair);
    if filter_sphere(m, d, g1, g2, margin + gap) { return false; }
    let type1 = m.geom_type[g1 as usize].min(m.geom_type[g2 as usize]);
    let type2 = m.geom_type[g1 as usize].max(m.geom_type[g2 as usize]);
    COLLISIONFUNC[type1 as usize][type2 as usize].is_some()
}

pub fn collision(m: &Model, d: &mut Data) {
    let nexclude = m.nexclude; let npair = m.npair; let nbody = m.nbody;
    let nbodyflex = m.nbody + m.nflex;
    let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && (d.nbody_awake as usize) < nbody;
    d.ncon = 0; reset_arena(d); clear_efc(d);
    if m.opt.disableflags & DSBL_CONSTRAINT != 0 || m.opt.disableflags & DSBL_CONTACT != 0 || nbodyflex < 2 { return; }
    // Model stores mjVisual as its little-endian file record; global.bvactive is word 12.
    if i32::from_le_bytes(m.vis[48..52].try_into().unwrap()) != 0 { d.bvh_active.fill(0); }
    let nmaxpairs = nbodyflex*(nbodyflex-1)/2;
    let mut broadphasepair = d.scratch.packed_pair(nmaxpairs);
    let nbfpair = broadphase(m, d, &mut broadphasepair, nmaxpairs);
    let mut last_signature = u32::MAX;
    let (mut pairadr, mut exadr, mut ncandidate) = (0, 0, 0);
    // Typed arena storage, retained in Scratch after the collision call.
    let mut pairs = d.scratch.pair(0);
    for i in 0..nbfpair {
        let bf1 = broadphasepair[i].hi as usize; let bf2 = broadphasepair[i].lo as usize;
        let signature = ((bf1 as u32) << 16) + bf2 as u32;
        if signature == last_signature { continue; } last_signature = signature;
        let mut merged = false; let startadr = pairadr;
        while pairadr < npair && m.pair_signature[pairadr] as u32 <= signature {
            merged = m.pair_signature[pairadr] as u32 == signature;
            let g1 = m.pair_geom1[pairadr]; let g2 = m.pair_geom2[pairadr];
            if filter_collision_pair(m, d, g1, g2, pairadr as i32, false, 0, 0) {
                ncandidate = push_geom_geom(m, &mut pairs, g1, g2, pairadr as i32, ncandidate);
            }
            pairadr += 1;
        }
        if !can_collide2(m, bf1, bf2) { continue; }
        if bf1 >= nbody && bf2 >= nbody { crate::unported!("mjc_ipcOwnsFlexFlex"); }
        if nexclude != 0 {
            while exadr < nexclude && (m.exclude_signature[exadr] as u32) < signature { exadr += 1; }
            if exadr < nexclude && m.exclude_signature[exadr] as u32 == signature { continue; }
        }
        let isbody1 = bf1 < nbody; let isbody2 = bf2 < nbody;
        let bvh1 = if isbody1 { m.body_bvhadr[bf1] } else { m.flex_bvhadr[bf1-nbody] };
        let bvh2 = if isbody2 { m.body_bvhadr[bf2] } else { m.flex_bvhadr[bf2-nbody] };
        let geomadr1 = if isbody1 { m.body_geomadr[bf1] } else { -1 };
        let geomadr2 = if isbody2 { m.body_geomadr[bf2] } else { -1 };
        if isbody1 && isbody2 && m.body_geomnum[bf1] == 1 && m.body_geomnum[bf2] == 1 {
            if filter_collision_pair(m, d, geomadr1, geomadr2, -1, merged, startadr, pairadr) {
                ncandidate = push_geom_geom(m, &mut pairs, geomadr1, geomadr2, -1, ncandidate);
            }
        } else if m.opt.disableflags & DSBL_MIDPHASE == 0 && bvh1 >= 0 && bvh2 >= 0 && !(speculative_enabled(m) && isbody1 && isbody2 && body_pair_has_fast_geom(m, d, bf1, bf2)) {
            ncandidate = collide_tree(m, d, bf1, bf2, merged, startadr, pairadr, &mut pairs, ncandidate);
        } else if isbody1 && isbody2 {
            let geomadr_end1 = geomadr1 + m.body_geomnum[bf1];
            let geomadr_end2 = geomadr2 + m.body_geomnum[bf2];
            for g1 in geomadr1..geomadr_end1 { for g2 in geomadr2..geomadr_end2 {
                if filter_collision_pair(m, d, g1, g2, -1, merged, startadr, pairadr) { ncandidate = push_geom_geom(m, &mut pairs, g1, g2, -1, ncandidate); }
            } }
        } else if isbody1 { crate::unported!("mjc_ipcOwnsFlexGeom"); }
        else { crate::unported!("mjc_ElemElem"); }
    }
    d.scratch.put_packed_pair(broadphasepair);
    while pairadr < npair {
        let g1 = m.pair_geom1[pairadr]; let g2 = m.pair_geom2[pairadr];
        if filter_collision_pair(m, d, g1, g2, pairadr as i32, false, 0, 0) { ncandidate = push_geom_geom(m, &mut pairs, g1, g2, pairadr as i32, ncandidate); }
        pairadr += 1;
    }
    for f in 0..m.nflex {
        if m.flex_rigid[f] == 0 && m.flex_contype[f] & m.flex_conaffinity[f] != 0 {
            if !has_awake_dofs(m, d, nbody+f, sleep_filter) { continue; }
            // Includes mj_collideFlexCellPairs, mj_collideFlexSAPPairs and self BVH pairs.
            crate::unported!("mjc_ipcOwnsFlexFlex");
        }
    }
    // Vec-backed arena grows; no candidate or contact is dropped for arena exhaustion.
    let npair_arena = pairs.len();
    if ncandidate > npair_arena { warning(d, WARN_CONTACTFULL, ncandidate as i32); }
    if npair_arena > 0 { narrowphase(m, d, &mut pairs); }
    d.scratch.put_pair(pairs);
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CollisionTree { node1: usize, node2: usize }

pub fn collide_obb(aabb1: &[f64; 6], aabb2: &[f64; 6], xpos1: Option<&[f64; 3]>, xmat1: Option<&[f64; 9]>, xpos2: Option<&[f64; 3]>, xmat2: Option<&[f64; 9]>, margin: f64, mut product: Option<&mut [f64; 36]>, mut offset: Option<&mut [f64; 12]>, initialize: &mut bool) -> bool {
    let inf1 = [aabb1[3] >= MAXVAL, aabb1[4] >= MAXVAL, aabb1[5] >= MAXVAL];
    let inf2 = [aabb2[3] >= MAXVAL, aabb2[4] >= MAXVAL, aabb2[5] >= MAXVAL];
    if (inf1[0] && inf1[1] && inf1[2]) || (inf2[0] && inf2[1] && inf2[2]) { return true; }
    let aabb = [aabb1, aabb2]; let xmat = [xmat1, xmat2]; let xpos = [xpos1, xpos2];
    let mut xcenter = [[0.0; 3]; 2]; let mut normal = [[[0.0; 3]; 3]; 2];
    let mut proj = [0.0; 2]; let mut radius = [0.0; 2];
    let infinite = [inf1[0] || inf1[1] || inf1[2], inf2[0] || inf2[1] || inf2[2]];
    if product.is_none() {
        for i in 0..2 {
            if let Some(mat) = xmat[i] { u::mul_mat_vec3(&mut xcenter[i], mat, at3(aabb[i], 0)); }
            else { u::copy3(&mut xcenter[i], at3(aabb[i], 0)); }
            if let Some(pos) = xpos[i] { u::add_to3(&mut xcenter[i], pos); }
        }
    }
    for i in 0..2 { for j in 0..3 { for k in 0..3 {
        normal[i][j][k] = if let Some(mat) = xmat[i] { mat[3*k+j] } else { (j == k) as i32 as f64 };
    } } }
    if let (Some(product), Some(offset)) = (product.as_deref_mut(), offset.as_deref_mut()) {
        if *initialize {
            for i in 0..2 { for j in 0..2 { for k in 0..3 {
                for l in 0..3 { product[18*i + 9*j + 3*k + l] = u::dot3(&normal[i][l], &normal[j][k]); }
                offset[6*i + 3*j + k] = if let Some(pos) = xpos[i] { u::dot3(pos, &normal[j][k]) } else { 0.0 };
            } } }
            *initialize = false;
        }
    }
    for j in 0..2 {
        if infinite[1-j] { continue; }
        for k in 0..3 {
            for i in 0..2 {
                if let Some(product) = product.as_deref() {
                    let adr = 18*i + 9*j + 3*k;
                    proj[i] = aabb[i][0]*product[adr] + aabb[i][1]*product[adr+1] + aabb[i][2]*product[adr+2] + offset.as_deref().unwrap()[6*i+3*j+k];
                    radius[i] = (aabb[i][3]*product[adr]).abs() + (aabb[i][4]*product[adr+1]).abs() + (aabb[i][5]*product[adr+2]).abs();
                } else {
                    proj[i] = u::dot3(&xcenter[i], &normal[j][k]);
                    radius[i] = (aabb[i][3]*u::dot3(&normal[i][0], &normal[j][k])).abs() + (aabb[i][4]*u::dot3(&normal[i][1], &normal[j][k])).abs() + (aabb[i][5]*u::dot3(&normal[i][2], &normal[j][k])).abs();
                }
            }
            if radius[0]+radius[1]+margin < (proj[1]-proj[0]).abs() { return false; }
        }
    }
    true
}

fn collide_tree(m: &Model, d: &mut Data, bf1: usize, bf2: usize, merged: bool, startadr: usize, pairadr: usize, pairs: &mut Vec<Pair>, mut npair: usize) -> usize {
    let isbody1 = bf1 < m.nbody; let isbody2 = bf2 < m.nbody;
    if !can_collide2(m, bf1, bf2) { return npair; }
    if isbody1 && !isbody2 { crate::unported!("mj_collideTree: geom-flex"); }
    if !isbody1 && isbody2 { panic!("BVH flex : body collision should not occur"); }
    if !isbody1 { crate::unported!("mj_collideTree: elem-elem"); }
    let mark_active = i32::from_le_bytes(m.vis[48..52].try_into().unwrap()) != 0;
    let bvhadr1 = m.body_bvhadr[bf1] as usize; let bvhadr2 = m.body_bvhadr[bf2] as usize;
    let child1 = &m.bvh_child[2*bvhadr1..]; let child2 = &m.bvh_child[2*bvhadr2..];
    let bvh1 = &m.bvh_aabb[6*bvhadr1..]; let bvh2 = &m.bvh_aabb[6*bvhadr2..];
    let mut product = [0.0; 36]; let mut offset = [0.0; 12]; let mut initialize = true;
    let max_stack = (m.body_bvhnum[bf1] + m.body_bvhnum[bf2]) as usize;
    let mut stack = d.scratch.collision_tree(max_stack);
    let mut nstack = 1;
    stack[0].node1 = 0; stack[0].node2 = 0;
    let start_pair = pairs.len();
    while nstack != 0 {
        nstack -= 1;
        let node1 = stack[nstack].node1; let node2 = stack[nstack].node2;
        let isleaf1 = child1[2*node1] < 0 && child1[2*node1+1] < 0;
        let isleaf2 = child2[2*node2] < 0 && child2[2*node2+1] < 0;
        let nodeid1 = m.bvh_nodeid[bvhadr1+node1]; let nodeid2 = m.bvh_nodeid[bvhadr2+node2];
        if (isleaf1 && nodeid1 < 0) || (isleaf2 && nodeid2 < 0) { panic!("BVH leaf has invalid node id"); }
        if bf1 == bf2 && node1 > node2 { continue; }
        if isleaf1 && isleaf2 {
            let g1 = nodeid1 as usize; let g2 = nodeid2 as usize;
            let margin = assign_margin(m, m.geom_margin[g1] + m.geom_margin[g2]);
            let gap = m.geom_gap[g1] + m.geom_gap[g2];
            if !filter_sphere(m, d, nodeid1, nodeid2, margin + gap) {
                if collide_obb(m.geom_aabb[6*g1..6*g1+6].try_into().unwrap(), m.geom_aabb[6*g2..6*g2+6].try_into().unwrap(), Some(at3(&d.geom_xpos, 3*g1)), Some(at9(&d.geom_xmat, 9*g1)), Some(at3(&d.geom_xpos, 3*g2)), Some(at9(&d.geom_xmat, 9*g2)), margin + gap, None, None, &mut initialize) {
                    if filter_collision_pair(m, d, nodeid1, nodeid2, -1, merged, startadr, pairadr) { npair = push_geom_geom(m, pairs, nodeid1, nodeid2, -1, npair); }
                    if mark_active { d.bvh_active[node1+bvhadr1] = 1; d.bvh_active[node2+bvhadr2] = 1; }
                }
            }
            continue;
        }
        let margin = assign_margin(m, m.body_margin[bf1] + m.body_margin[bf2]);
        if !collide_obb(bvh1[6*node1..6*node1+6].try_into().unwrap(), bvh2[6*node2..6*node2+6].try_into().unwrap(), Some(at3(&d.xipos, 3*bf1)), Some(at9(&d.ximat, 9*bf1)), Some(at3(&d.xipos, 3*bf2)), Some(at9(&d.ximat, 9*bf2)), margin, Some(&mut product), Some(&mut offset), &mut initialize) { continue; }
        if mark_active { d.bvh_active[node1+bvhadr1] = 1; d.bvh_active[node2+bvhadr2] = 1; }
        if !isleaf1 && isleaf2 {
            for i in 0..2 {
                if child1[2*node1+i] != -1 {
                    if nstack >= max_stack { panic!("BVH stack depth exceeded."); }
                    stack[nstack].node1 = child1[2*node1+i] as usize; stack[nstack].node2 = node2; nstack += 1;
                }
            }
        } else if isleaf1 && !isleaf2 {
            for i in 0..2 {
                if child2[2*node2+i] != -1 {
                    if nstack >= max_stack { panic!("BVH stack depth exceeded."); }
                    stack[nstack].node1 = node1; stack[nstack].node2 = child2[2*node2+i] as usize; nstack += 1;
                }
            }
        } else {
            let x1 = bvh1[6*node1+3]; let y1 = bvh1[6*node1+4]; let z1 = bvh1[6*node1+5];
            let x2 = bvh2[6*node2+3]; let y2 = bvh2[6*node2+4]; let z2 = bvh2[6*node2+5];
            let surface1 = x1*y1 + y1*z1 + z1*x1;
            let surface2 = x2*y2 + y2*z2 + z2*x2;
            if surface1 > surface2 {
                for i in 0..2 {
                    if child1[2*node1+i] != -1 {
                        if nstack >= max_stack { panic!("BVH stack depth exceeded."); }
                        stack[nstack].node1 = child1[2*node1+i] as usize; stack[nstack].node2 = node2; nstack += 1;
                    }
                }
            } else {
                for i in 0..2 {
                    if child2[2*node2+i] != -1 {
                        if nstack >= max_stack { panic!("BVH stack depth exceeded."); }
                        stack[nstack].node1 = node1; stack[nstack].node2 = child2[2*node2+i] as usize; nstack += 1;
                    }
                }
            }
        }
    }
    let end_pair = pairs.len(); let n = end_pair-start_pair;
    if n > 1 {
        let mut buf = d.scratch.pair(n);
        sort(&mut pairs[start_pair..], &mut buf, n, &mut pair_compare);
        d.scratch.put_pair(buf);
    }
    d.scratch.put_collision_tree(stack);
    npair
}

fn make_aamm(m: &Model, d: &Data, x_min: &mut f64, y_min: &mut f64, z_min: &mut f64, x_max: &mut f64, y_max: &mut f64, z_max: &mut f64, bf: usize, frame: &[f64; 9]) {
    let mut aamm = [0.0; 6];
    let override_margin = if m.opt.enableflags & ENBL_OVERRIDE != 0 { 0.5*m.opt.o_margin } else { 0.0 };
    if bf < m.nbody {
        let body = bf; let body_geomnum = m.body_geomnum[body];
        for i in 0..body_geomnum {
            let geom = (m.body_geomadr[body]+i) as usize;
            let margin = if override_margin != 0.0 { override_margin } else { m.geom_margin[geom] + m.geom_gap[geom] };
            let mut geom_aamm = [0.0; 6]; // C's _aamm, distinct from the body aamm.
            let aabb = at3(&m.geom_aabb, 6*geom); let size = at3(&m.geom_aabb, 6*geom+3);
            let xpos = at3(&d.geom_xpos, 3*geom); let xmat = at9(&d.geom_xmat, 9*geom);
            let mut pos = [0.0; 3]; inline::mul_mat_vec3(&mut pos, xmat, aabb); u::add_to3(&mut pos, xpos);
            let mut axis = [0.0; 9]; inline::transpose3(&mut axis, xmat);
            let r_half = m.geom_rbound[geom];
            let mut dx = [0.0; 3]; let mut dw = [0.0; 3];
            let s = if speculative_enabled(m) { geom_step_sweep(m, d, geom as i32, 0.0, Some(&mut dx), Some(&mut dw)) } else { 0.0 };
            let s_rot = if s > 0.0 { u::norm3(&dw)*r_half } else { 0.0 };
            for j in 0..3 {
                let frame_j = at3(frame, 3*j);
                let aabb_cen = u::dot3(&pos, frame_j);
                let aabb_half = (size[0]*u::dot3(at3(&axis, 0), frame_j)).abs() + (size[1]*u::dot3(at3(&axis, 3), frame_j)).abs() + (size[2]*u::dot3(at3(&axis, 6), frame_j)).abs() + s_rot;
                let r_cen = u::dot3(xpos, frame_j);
                let mut lo = max(r_cen-r_half, aabb_cen-aabb_half);
                let mut hi = min(r_cen+r_half, aabb_cen+aabb_half);
                if s > 0.0 { let d_proj = u::dot3(&dx, frame_j); lo += min(0.0, d_proj); hi += max(0.0, d_proj); }
                geom_aamm[j] = lo-margin; geom_aamm[j+3] = hi+margin;
            }
            if i == 0 { aamm = geom_aamm; }
            else { for j in 0..3 { aamm[j] = min(aamm[j], geom_aamm[j]); aamm[j+3] = max(aamm[j+3], geom_aamm[j+3]); } }
        }
    } else { crate::unported!("mj_rodBoxSize"); }
    *x_min = aamm[0]; *y_min = aamm[1]; *z_min = aamm[2];
    *x_max = aamm[3]; *y_max = aamm[4]; *z_max = aamm[5];
}

fn add_pair(m: Option<&Model>, bf1: usize, bf2: usize, npair: &mut usize, pair: &mut [Packed32], maxpair: usize) {
    if *npair < maxpair {
        if let Some(m) = m {
            let nbody = m.nbody;
            let (mut contype1, mut conaffinity1, mut contype2, mut conaffinity2) = (0,0,0,0);
            if bf1 < nbody {
                let body_geomadr1 = m.body_geomadr[bf1]; let body_geomnum1 = m.body_geomnum[bf1];
                for i in body_geomadr1..body_geomadr1+body_geomnum1 { contype1 |= m.geom_contype[i as usize]; conaffinity1 |= m.geom_conaffinity[i as usize]; }
            } else { contype1 = m.flex_contype[bf1-nbody]; conaffinity1 = m.flex_conaffinity[bf1-nbody]; }
            if bf2 < nbody {
                let body_geomadr2 = m.body_geomadr[bf2]; let body_geomnum2 = m.body_geomnum[bf2];
                for i in body_geomadr2..body_geomadr2+body_geomnum2 { contype2 |= m.geom_contype[i as usize]; conaffinity2 |= m.geom_conaffinity[i as usize]; }
            } else { contype2 = m.flex_contype[bf2-nbody]; conaffinity2 = m.flex_conaffinity[bf2-nbody]; }
            if contype1 & conaffinity2 == 0 && contype2 & conaffinity1 == 0 { return; }
        }
        let n = *npair; *npair += 1;
        pair[n].hi = if bf1 < bf2 { bf1 } else { bf2 } as u16;
        pair[n].lo = if bf1 < bf2 { bf2 } else { bf1 } as u16;
    } else { panic!("broadphase buffer full"); }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Sap { value: f32, id_ismax: i32 }
fn sapcmp(obj1: &Sap, obj2: &Sap) -> i32 {
    if obj1.value < obj2.value { -1 }
    else if obj1.value == obj2.value { (obj1.id_ismax & 0x10000) - (obj2.id_ismax & 0x10000) }
    else { 1 }
}

// Public for the driver integration test; C's mj_SAP is file-local.
pub fn sap(d: &mut Data, aamm: &[f64], n: usize, axis_x: i32, pair: &mut [Packed32], maxpair: usize) -> i32 {
    if n >= 0x10000 || axis_x < 0 || axis_x > 2 || maxpair < 1 { return -1; }
    let mut sortbuf = d.scratch.sap(2*n); let mut activebuf = d.scratch.sap(2*n);
    let x_min = &aamm[n*axis_x as usize..]; let x_max = &aamm[n*(axis_x as usize+3)..];
    for i in 0..n {
        sortbuf[2*i].id_ismax = i as i32; sortbuf[2*i].value = x_min[i] as f32;
        sortbuf[2*i+1].id_ismax = i as i32 + 0x10000; sortbuf[2*i+1].value = x_max[i] as f32;
    }
    let mut buf = d.scratch.sap(2*n);
    sort(&mut sortbuf, &mut buf, 2*n, &mut sapcmp);
    let (axis_y, axis_z) = if axis_x == 0 { (1,2) } else if axis_x == 1 { (0,2) } else { (0,1) };
    let y_min = &aamm[n*axis_y..]; let y_max = &aamm[n*(axis_y+3)..];
    let z_min = &aamm[n*axis_z..]; let z_max = &aamm[n*(axis_z+3)..];
    let mut cnt = 0; let mut npair = 0;
    'sweep: for i in 0..2*n {
        if sortbuf[i].id_ismax & 0x10000 == 0 {
            for j in 0..cnt {
                let id1 = activebuf[j].id_ismax as usize; let id2 = sortbuf[i].id_ismax as usize;
                if y_min[id1] > y_max[id2] || y_min[id2] > y_max[id1] || z_min[id1] > z_max[id2] || z_min[id2] > z_max[id1] { continue; }
                if npair >= maxpair { npair = maxpair + 1; break 'sweep; }
                pair[npair].hi = id1 as u16; pair[npair].lo = id2 as u16; npair += 1;
            }
            activebuf[cnt] = sortbuf[i]; cnt += 1;
        } else {
            let toremove = sortbuf[i].id_ismax & 0xFFFF;
            for j in 0..cnt {
                if activebuf[j].id_ismax == toremove {
                    if j < cnt-1 { activebuf.copy_within(j+1..cnt, j); }
                    cnt -= 1; break;
                }
            }
        }
    }
    d.scratch.put_sap(buf); d.scratch.put_sap(activebuf); d.scratch.put_sap(sortbuf);
    npair as i32
}
fn update_cov(cov: &mut [f64; 9], vec: &[f64; 3], cen: &[f64; 3]) {
    let dif = [vec[0]-cen[0], vec[1]-cen[1], vec[2]-cen[2]];
    let d00 = dif[0]*dif[0]; let d01 = dif[0]*dif[1]; let d02 = dif[0]*dif[2];
    let d11 = dif[1]*dif[1]; let d12 = dif[1]*dif[2]; let d22 = dif[2]*dif[2];
    cov[0] += d00; cov[1] += d01; cov[2] += d02; cov[3] += d01; cov[4] += d11; cov[5] += d12; cov[6] += d02; cov[7] += d12; cov[8] += d22;
}
fn packed32cmp(i: &Packed32, j: &Packed32) -> i32 {
    if i.hi < j.hi { return -1; } if i.hi > j.hi { return 1; }
    if i.lo < j.lo { return -1; } if i.lo > j.lo { return 1; } 0
}
fn broadphase(m: &Model, d: &mut Data, bfpair: &mut [Packed32], maxpair: usize) -> usize {
    let mut npair = 0; let nbody = m.nbody; let ngeom = m.ngeom;
    let nvert = m.nflexvert; let nflex = m.nflex; let nbodyflex = m.nbody+m.nflex;
    let dsbl_filterparent = m.opt.disableflags & DSBL_FILTERPARENT != 0;
    let sleep_filter = m.opt.enableflags & ENBL_SLEEP != 0 && (d.nbody_awake as usize) < nbody;
    for b1 in 0..nbody {
        if !can_collide(m, b1) { continue; }
        if (b1 == 0 && m.body_geomnum[b1] > 0) || (m.body_dofnum[m.body_weldid[b1] as usize] == 0 && has_plane(m, b1)) {
            let weld1 = m.body_weldid[b1]; let parent_weld1 = m.body_weldid[m.body_parentid[weld1 as usize] as usize]; let dofnum1 = m.body_dofnum[weld1 as usize];
            for b2 in 0..nbody {
                if !can_collide(m, b2) { continue; }
                let weld2 = m.body_weldid[b2]; let parent_weld2 = m.body_weldid[m.body_parentid[weld2 as usize] as usize];
                let asleep2 = sleep_filter && d.body_awake[b2] as i32 == S_ASLEEP; let dofnum2 = m.body_dofnum[weld2 as usize];
                if filter_body_pair(weld1, parent_weld1, false, still_weld(m, d, weld1, sleep_filter), dofnum1, weld2, parent_weld2, asleep2, still_weld(m, d, weld2, sleep_filter), dofnum2, dsbl_filterparent) { continue; }
                add_pair(Some(m), b1, b2, &mut npair, bfpair, maxpair);
            }
            for f in 0..nflex {
                if !has_awake_dofs(m, d, nbody+f, sleep_filter) { continue; }
                add_pair(Some(m), b1, nbody+f, &mut npair, bfpair, maxpair);
            }
        }
    }
    let mut cnt = 0; let mut cen = [0.0; 3];
    for i in 0..ngeom { if m.geom_bodyid[i] != 0 { u::add_to3(&mut cen, at3(&d.geom_xpos, 3*i)); cnt += 1; } }
    for i in 0..nvert { if m.flex_vertbodyid[i] != 0 { u::add_to3(&mut cen, at3(&d.flexvert_xpos, 3*i)); cnt += 1; } }
    if cnt == 0 { return npair; }
    let cen_copy = cen; u::scl3(&mut cen, &cen_copy, 1.0/cnt as f64);
    let mut cov = [0.0; 9];
    for i in 0..ngeom { if m.geom_bodyid[i] != 0 { update_cov(&mut cov, at3(&d.geom_xpos, 3*i), &cen); } }
    for i in 0..nvert { if m.flex_vertbodyid[i] != 0 { update_cov(&mut cov, at3(&d.flexvert_xpos, 3*i), &cen); } }
    let cov_copy = cov; u::scl(&mut cov, &cov_copy, 1.0/cnt as f64, 9);
    let mut eigval = [0.0; 3]; let mut eigvec = [0.0; 9]; let mut frame = [0.0; 9]; let mut quat = [0.0; 4];
    eig3tol(&mut eigval, &mut eigvec, &mut quat, &cov, 1e-3);
    inline::transpose3(&mut frame, &eigvec);
    let mut bfid = d.scratch.int(nbodyflex); let mut ncollide = 0;
    for i in 1..nbodyflex { if can_collide(m, i) { bfid[ncollide] = i as i32; ncollide += 1; } }
    if ncollide > 1 {
        let mut aamm = d.scratch.num(6*ncollide);
        for i in 0..ncollide {
            let (mut xmin, mut ymin, mut zmin, mut xmax, mut ymax, mut zmax) = (0.0,0.0,0.0,0.0,0.0,0.0);
            make_aamm(m, d, &mut xmin, &mut ymin, &mut zmin, &mut xmax, &mut ymax, &mut zmax, bfid[i] as usize, &frame);
            aamm[i] = xmin; aamm[ncollide+i] = ymin; aamm[2*ncollide+i] = zmin;
            aamm[3*ncollide+i] = xmax; aamm[4*ncollide+i] = ymax; aamm[5*ncollide+i] = zmax;
        }
        let maxsappair = ncollide*(ncollide-1)/2; let mut sappair = d.scratch.packed_pair(maxsappair);
        let mut nsappair = sap(d, &aamm, ncollide, 0, &mut sappair, maxsappair);
        if nsappair < 0 { panic!("SAP failed"); }
        if nsappair as usize > maxsappair { warning(d, WARN_CONTACTFULL, maxsappair as i32); nsappair = maxsappair as i32; }
        for i in 0..nsappair as usize {
            let bf1 = bfid[sappair[i].hi as usize] as usize; let bf2 = bfid[sappair[i].lo as usize] as usize;
            if bf1 < nbody && bf2 < nbody {
                let weld1 = m.body_weldid[bf1]; let weld2 = m.body_weldid[bf2];
                let parent_weld1 = m.body_weldid[m.body_parentid[weld1 as usize] as usize]; let parent_weld2 = m.body_weldid[m.body_parentid[weld2 as usize] as usize];
                let asleep1 = sleep_filter && d.body_awake[bf1] as i32 == S_ASLEEP; let asleep2 = sleep_filter && d.body_awake[bf2] as i32 == S_ASLEEP;
                let dofnum1 = m.body_dofnum[weld1 as usize]; let dofnum2 = m.body_dofnum[weld2 as usize];
                if filter_body_pair(weld1, parent_weld1, asleep1, still_weld(m, d, weld1, sleep_filter), dofnum1, weld2, parent_weld2, asleep2, still_weld(m, d, weld2, sleep_filter), dofnum2, dsbl_filterparent) { continue; }
            } else if !has_awake_dofs(m, d, bf1.min(bf2), sleep_filter) && !has_awake_dofs(m, d, bf1.max(bf2), sleep_filter) { continue; }
            add_pair(Some(m), bf1, bf2, &mut npair, bfpair, maxpair);
        }
        d.scratch.put_packed_pair(sappair); d.scratch.put_num(aamm);
    }
    if npair > 1 {
        let mut buf = d.scratch.packed_pair(npair);
        sort(bfpair, &mut buf, npair, &mut packed32cmp);
        d.scratch.put_packed_pair(buf);
    }
    d.scratch.put_int(bfid);
    npair
}

fn set_contact(m: &Model, con: &mut Contact, condim: i32, includemargin: f64, solref: &[f64; NREF], solreffriction: &[f64; NREF], solimp: &[f64; NIMP], friction: &[f64; 5], adhesion: f64, speculative: i32) {
    con.dim = condim; con.includemargin = includemargin; con.adhesion = adhesion;
    assign_ref(m, &mut con.solref, solref); assign_ref(m, &mut con.solreffriction, solreffriction);
    assign_imp(m, &mut con.solimp, solimp); assign_friction(m, &mut con.friction, friction);
    con.exclude = (con.dist >= includemargin && adhesion == 0.0 && speculative == 0) as i32;
    if adhesion != 0.0 && speculative == 0 && con.dist >= includemargin { con.dim = 1; }
    make_frame(&mut con.frame);
    inline::zero3(&mut con.surfacevel);
    con.efc_address = -1; con.mu = 0.0; u::zero(&mut con.h, 36);
    con.geom1 = con.geom[0]; con.geom2 = con.geom[1];
}

// The surface-velocity part of mj_setContact, for a contact already in d: it reads the geoms' motion from d,
// so set_contact, which writes the contact in place, leaves it to this.
fn set_contact_surfacevel(m: &Model, d: &mut Data, i: usize) {
    let (geom, pos, frame) = (d.contact[i].geom, d.contact[i].pos, d.contact[i].frame);
    let mut svel = [0.0; 3]; let mut sang = [0.0; 3]; let mut active = false;
    for side in 0..2 {
        let g = geom[side];
        if g < 0 { continue; }
        let sv = &m.geom_surfacevel[6*g as usize..];
        if sv[0] == 0.0 && sv[1] == 0.0 && sv[2] == 0.0 && sv[3] == 0.0 && sv[4] == 0.0 && sv[5] == 0.0 { continue; }
        active = true;
        let sgn = if side != 0 { 1.0 } else { -1.0 };
        let mut vw = [0.0; 3]; let mut ww = [0.0; 3];
        geom_surface_velocity(m, d, g as usize, &pos, &mut vw, &mut ww);
        inline::add_to_scl3(&mut svel, &vw, sgn); inline::add_to_scl3(&mut sang, &ww, sgn);
    }
    if active {
        let con = &mut d.contact[i];
        con.surfacevel[0] = u::dot3(at3(&frame, 3), &svel);
        con.surfacevel[1] = u::dot3(at3(&frame, 6), &svel);
        con.surfacevel[2] = u::dot3(at3(&frame, 0), &sang);
    }
}

fn pair_max_contact(m: &Model, pair: &Pair) -> usize {
    let g1 = pair.g1; let g2 = pair.g2; let ipair = pair.ipair;
    let margin = get_margin(m, g1, g2, ipair); let gap = get_gap(m, g1, g2, ipair);
    max_contact(m, g1, g2, margin+gap > 0.0)
}
fn pair_needs_ccd(m: &Model, pair: &Pair) -> bool {
    if m.opt.disableflags & DSBL_NATIVECCD != 0 { return false; }
    if speculative_enabled(m) { return true; }
    let type1 = m.geom_type[pair.g1 as usize]; let type2 = m.geom_type[pair.g2 as usize];
    let Some(func) = COLLISIONFUNC[type1 as usize][type2 as usize] else { return false; };
    use CollisionFn::*;
    if matches!(func, PlaneSphere | PlaneCapsule | PlaneCylinder | PlaneBox | PlaneConvex | SphereSphere | SphereCapsule
        | SphereCylinder | SphereBox | CapsuleCapsule | CapsuleBox | BoxBox | Sdf | HfieldSdf | MeshSdf) { return false; }
    true
}
// One task's chunk of pairs: the pairs, their contact counts, and the contact buffer from the first pair's
// conpos (`conbase`) to the next chunk's.
struct ContactChunk<'a> {
    pairs: &'a mut [Pair],
    ncon: &'a mut [i32],
    con: &'a mut [PreContact],
    conbase: usize,
    task: &'a mut CollisionTask,
}
fn collision_task(m: &Model, d: &Data, c: ContactChunk<'_>, ccd_size: usize) {
    let ContactChunk { pairs, ncon, con, conbase, task } = c;
    // mjc_setCCDBuffer binds the task's retained pool in this port. Convex
    // colliders take/return it themselves; continuous collision borrows it below.
    if ccd_size > 0 {
        let buffer = task.scratch.ccd_buffer(ccd_size);
        task.scratch.put_ccd_buffer(buffer, ccd_size);
    }
    let n = pairs.len();
    for i in 0..n {
        let conpos = pairs[i].conpos - conbase;
        let g1 = pairs[i].g1; let g2 = pairs[i].g2; let ipair = pairs[i].ipair;
        let collision_func = COLLISIONFUNC[m.geom_type[g1 as usize] as usize][m.geom_type[g2 as usize] as usize].unwrap();
        let margin = get_margin(m, g1, g2, ipair); let gap = get_gap(m, g1, g2, ipair);
        ncon[i] = collision_func.call(m, d, task, &mut con[conpos..], g1, g2, margin+gap) as i32;
        if speculative_enabled(m) {
            let mut active = false;
            for k in 0..ncon[i] as usize {
                if con[conpos+k].dist < margin { active = true; break; }
            }
            let min_size = pair_min_size(m, g1, g2);
            if !active && (geom_step_sweep(m, d, g1, min_size, None, None) > 0.0 || geom_step_sweep(m, d, g2, min_size, None, None) > 0.0) {
                let mut buffer = task.scratch.ccd_buffer(ccd_size);
                let nccd = advance_geom_pair(m, d, g1, g2, margin, Some(&mut con[conpos..]), Some(&mut buffer));
                task.scratch.put_ccd_buffer(buffer, ccd_size);
                if nccd > 0 { ncon[i] = nccd as i32; pairs[i].spec = 1; }
            }
        }
        let expected_max = (if i+1 < n { pairs[i+1].conpos - conbase } else { con.len() }) - conpos;
        if ncon[i] as usize > expected_max { panic!("collision function returned {} contacts, expected at most {} from pairMaxContact", ncon[i], expected_max); }
    }
}
fn add_pair_contacts(m: &Model, d: &mut Data, precon: &[PreContact], ncon: usize, pair: &Pair, _total_demand: i32) -> bool {
    if ncon == 0 { return true; }
    // The Vec arena grows instead of failing mj_arenaAllocByte. C has no
    // nconmax clamp here: max-contact bounds are per pair, not a global cap.
    let start = d.ncon as usize;
    if d.contact.len() < start+ncon { d.contact.resize(start+ncon, Contact::default()); }
    d.ncon += ncon as i32;
    d.parena = d.ncon as usize * std::mem::size_of::<Contact>();
    let mut condim = 0;
    let mut friction = [0.0; 5]; let mut solref = [0.0; NREF]; let mut solimp = [0.0; NIMP]; let mut adhesion = 0.0;
    let mut solreffriction = [0.0; NREF];
    let g1 = pair.g1; let g2 = pair.g2; let ipair = pair.ipair; let spec = pair.spec;
    let margin = get_margin(m, g1, g2, ipair);
    if ipair >= 0 {
        let ipair = ipair as usize;
        condim = m.pair_dim[ipair];
        u::copy(&mut solref, &m.pair_solref[NREF*ipair..], NREF);
        u::copy(&mut solimp, &m.pair_solimp[NIMP*ipair..], NIMP);
        u::copy(&mut friction, &m.pair_friction[5*ipair..], 5);
        adhesion = m.pair_adhesion[ipair];
        if m.pair_solreffriction[NREF*ipair] != 0.0 || m.pair_solreffriction[NREF*ipair+1] != 0.0 { u::copy(&mut solreffriction, &m.pair_solreffriction[NREF*ipair..], NREF); }
    } else { contact_param(m, &mut condim, &mut solref, &mut solimp, &mut friction, &mut adhesion, g1, g2, -1, -1); }
    for i in 0..ncon {
        let con = &mut d.contact[start+i];
        con.dist = precon[i].dist; inline::copy3(&mut con.pos, &precon[i].pos);
        inline::copy3(crate::inline::at3_mut(&mut con.frame, 0), &precon[i].normal);
        inline::copy3(crate::inline::at3_mut(&mut con.frame, 3), &precon[i].tangent);
        con.geom = [g1,g2]; con.flex = [-1,-1]; con.elem = [-1,-1]; con.vert = [-1,-1];
        set_contact(m, con, condim, margin, &solref, &solreffriction, &solimp, &friction, adhesion, spec);
        if m.flg_surfacevel != 0 { set_contact_surfacevel(m, d, start+i); }
    }
    true
}

// filterPreContacts is omitted: only untranslated flex pairs create filter groups.
fn narrowphase(m: &Model, d: &mut Data, pairs: &mut [Pair]) {
    let npair = pairs.len();
    let nthread = num_thread(d);
    let chunksize = if nthread == 1 { npair } else { 1.max(npair/(5*nthread)) };
    let nchunk = (npair+chunksize-1)/chunksize;
    let mut pairbuffer = d.scratch.pair(npair);
    let mut maxcon = 0; let mut has_ccd = false;
    for i in 0..npair {
        pairbuffer[i] = pairs[i]; pairbuffer[i].conpos = maxcon;
        maxcon += pair_max_contact(m, &pairs[i]);
        if !has_ccd && pair_needs_ccd(m, &pairs[i]) { has_ccd = true; }
    }
    let mut ccd_size_ = 0; // C's ccd_size local, distinct from the callee.
    if has_ccd {
        let npolygonmax = if m.opt.disableflags & DSBL_MULTICCD != 0 { 0 } else { m.npolygonmax };
        let nmeshdegmax = if m.opt.disableflags & DSBL_MULTICCD != 0 { 0 } else { m.nmeshdegmax };
        ccd_size_ = ccd_size(npolygonmax, nmeshdegmax, m.opt.ccd_iterations as usize);
    }
    reset_arena(d);
    let mut nconbuffer = d.scratch.contact_count(npair);
    let mut conbuffer = d.scratch.pre_contact(maxcon);
    // each chunk of pairs is a task with its own contacts and records, Data shared read-only
    let mut tasks = std::mem::take(&mut d.collision_tasks);
    if tasks.len() < nchunk { tasks.resize_with(nchunk, CollisionTask::default); }
    {
        let mut con_rest = &mut conbuffer[..];
        let mut conbase = 0;
        let chunks = pairbuffer.chunks_mut(chunksize).zip(nconbuffer.chunks_mut(chunksize)).zip(tasks.iter_mut()).map(|((pairs, ncon), task)| {
            let conend = pairs.last().map_or(conbase, |p| p.conpos + pair_max_contact(m, p));
            let (con, rest) = std::mem::take(&mut con_rest).split_at_mut(conend - conbase);
            con_rest = rest;
            let chunk = ContactChunk { pairs, ncon, con, conbase, task };
            conbase = conend;
            chunk
        });
        let d: &Data = d;
        run_parts(&d.threadpool, chunks, |_, chunk| collision_task(m, d, chunk, ccd_size_));
    }
    merge_collision_tasks(m, d, &mut tasks[..nchunk]);
    d.collision_tasks = tasks;
    let mut total_demand = d.ncon;
    for i in 0..npair {
        if pairbuffer[i].group >= 0 { crate::unported!("filterPreContacts: flex groups"); }
        total_demand += nconbuffer[i];
    }
    for i in 0..npair {
        let ncon = nconbuffer[i] as usize;
        if ncon == 0 { continue; }
        if !add_pair_contacts(m, d, &conbuffer[pairbuffer[i].conpos..], ncon, &pairbuffer[i], total_demand) { break; }
    }
    d.scratch.put_pre_contact(conbuffer); d.scratch.put_contact_count(nconbuffer); d.scratch.put_pair(pairbuffer);
}

// mj_collideFlexSAPPairs and mj_collideFlexCellPairs are omitted with flex
// self-collision. Their call path stops at mjc_ipcOwnsFlexFlex above.
