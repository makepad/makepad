//! Continuous geometry and conservative advancement of geom pairs.
//!
//! Translated from `src/engine/engine_collision_continuous.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2026 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::collision_convex::init_ccd_obj;
use crate::collision_gjk::{gjk, multicontact, CcdBuffer, CcdObj, CcdSupport};
use crate::collision_primitive::{seg_weights, tri_weights};
use crate::core_constraint::{assign_imp, assign_margin, assign_ref, contact_param};
use crate::core_util::is_metric;
use crate::data::{Data, PreContact};
use crate::inline::{self as inl, at3, at3_mut, at9};
use crate::model::{default_sol_ref_imp, disabled, Model};
use crate::ray::{ray_geom, ray_mesh};
use crate::types::*;
use crate::util_blas as u;
use crate::util_misc::{max, min};
use crate::util_spatial::{axis_angle2quat, quat2mat, transform_spatial};

// Point-triangle distance, closest point and barycentric weights.
pub fn pt_tri(p: &[f64; 3], a: &[f64; 3], b: &[f64; 3], c: &[f64; 3], cp: &mut [f64; 3], w: &mut [f64; 3]) -> f64 {
    tri_weights(w, p, a, b, c);
    for k in 0..3 { cp[k] = w[0] * a[k] + w[1] * b[k] + w[2] * c[k]; }
    let mut dd = [0.0; 3];
    u::sub3(&mut dd, p, cp);
    u::dot3(&dd, &dd).sqrt()
}

// Segment-segment distance, closest points and segment parameters.
pub fn seg_seg(p1: &[f64; 3], p2: &[f64; 3], q1: &[f64; 3], q2: &[f64; 3], cp1: &mut [f64; 3], cp2: &mut [f64; 3], st: &mut [f64; 2]) -> f64 {
    let mut w = [0.0; 4];
    seg_weights(&mut w, p1, p2, q1, q2);
    st[0] = w[0];
    st[1] = w[1];
    for k in 0..3 {
        cp1[k] = p1[k] + st[0] * (p2[k] - p1[k]);
        cp2[k] = q1[k] + st[1] * (q2[k] - q1[k]);
    }
    let mut dd = [0.0; 3];
    u::sub3(&mut dd, cp1, cp2);
    u::dot3(&dd, &dd).sqrt()
}

pub fn geom_supported(type_: i32) -> i32 {
    (type_ == GEOM_PLANE || type_ == GEOM_SPHERE || type_ == GEOM_CAPSULE || type_ == GEOM_ELLIPSOID || type_ == GEOM_CYLINDER || type_ == GEOM_BOX || type_ == GEOM_MESH) as i32
}

// Signed surface distance and outward normal at a world point.
pub fn geom_dist(m: &Model, gi: i32, gpos: &[f64; 3], gmat: &[f64; 9], x: &[f64; 3], n: &mut [f64; 3], distmax: f64) -> f64 {
    let gi_idx = gi as usize;
    let type_ = m.geom_type[gi_idx];
    let size = at3(&m.geom_size, 3 * gi_idx);
    let dx = [x[0] - gpos[0], x[1] - gpos[1], x[2] - gpos[2]];
    n[0] = 0.0; n[1] = 0.0; n[2] = 1.0;
    if type_ == GEOM_PLANE {
        n[0] = gmat[2]; n[1] = gmat[5]; n[2] = gmat[8];
        return u::dot3(n, &dx);
    }
    if type_ == GEOM_SPHERE {
        let l = u::dot3(&dx, &dx).sqrt();
        if l < 1e-12 { return -size[0]; }
        inl::scl3(n, &dx, 1.0 / l);
        return l - size[0];
    }
    let mut p = [0.0; 3];
    u::mul_mat_tvec3(&mut p, gmat, &dx);
    let mut nl = [0.0; 3];
    let mut dist;
    if type_ == GEOM_CAPSULE {
        let hz = size[1];
        let zc = if p[2] > hz { hz } else if p[2] < -hz { -hz } else { p[2] };
        let q = [p[0], p[1], p[2] - zc];
        let l = u::dot3(&q, &q).sqrt();
        if l < 1e-12 { nl[0] = 1.0; } else { inl::scl3(&mut nl, &q, 1.0 / l); }
        dist = l - size[0];
    } else if type_ == GEOM_BOX {
        let mut q = [0.0; 3];
        let mut outside = false;
        for k in 0..3 {
            let c = if p[k] > size[k] { size[k] } else if p[k] < -size[k] { -size[k] } else { p[k] };
            q[k] = p[k] - c;
            if q[k] != 0.0 { outside = true; }
        }
        if outside {
            let l = u::dot3(&q, &q).sqrt();
            dist = l;
            inl::scl3(&mut nl, &q, 1.0 / l);
        } else {
            let mut ax = 0;
            let mut best = 1e30;
            for k in 0..3 {
                let pen = size[k] - if p[k] < 0.0 { -p[k] } else { p[k] };
                if pen < best { best = pen; ax = k; }
            }
            dist = -best;
            nl[ax] = if p[ax] > 0.0 { 1.0 } else { -1.0 };
        }
    } else if type_ == GEOM_MESH || type_ == GEOM_CYLINDER || type_ == GEOM_ELLIPSOID {
        if type_ == GEOM_MESH && m.mesh_polynum[m.geom_dataid[gi_idx] as usize] <= 0 { return 1e30; }
        // The C designated initializer zeroes the remaining scalar fields. The
        // typed default supplies an unused center callback and empty borrowed data.
        let mut obj_pt = CcdObj {
            pos: [x[0], x[1], x[2]], support: CcdSupport::Point,
            geom: 0, geom_type: 0, flex: 0, elem: 0, vert: 0,
            vertindex: std::cell::Cell::new(0), meshindex: std::cell::Cell::new(0),
            ..CcdObj::default()
        };
        let mut obj_geom = CcdObj::default();
        init_ccd_obj(&mut obj_geom, m, None, gi, 0.0);
        u::copy3(&mut obj_geom.pos, gpos);
        u::copy9(&mut obj_geom.mat, gmat);
        let mut x1 = [x[0], x[1], x[2]];
        let mut x2 = [gpos[0], gpos[1], gpos[2]];
        let gdist = gjk(&mut obj_pt, &mut obj_geom, &mut x1, &mut x2, distmax, m.opt.ccd_tolerance, m.opt.ccd_iterations as usize);
        if gdist > distmax { return 1e30; }
        if gdist > 0.0 {
            let mut diff = [x[0] - x2[0], x[1] - x2[1], x[2] - x2[2]];
            if u::normalize3(&mut diff) > 0.0 { u::copy3(n, &diff); return gdist; }
        }
        if type_ == GEOM_MESH {
            let meshid = m.geom_dataid[gi_idx] as usize;
            let vbase = &m.mesh_vert[3 * m.mesh_vertadr[meshid] as usize..];
            let polyadr = m.mesh_polyadr[meshid] as usize;
            let pn = m.mesh_polynum[meshid] as usize;
            dist = -1e30;
            let mut bestn = at3(&m.mesh_polynormal, 3 * polyadr);
            for i in 0..pn {
                let pnl = at3(&m.mesh_polynormal, 3 * (polyadr + i));
                let v0 = &vbase[3 * m.mesh_polyvert[m.mesh_polyvertadr[polyadr + i] as usize] as usize..];
                let c = pnl[0] * v0[0] as f64 + pnl[1] * v0[1] as f64 + pnl[2] * v0[2] as f64;
                let dd = u::dot3(pnl, &p) - c;
                if dd > dist { dist = dd; bestn = pnl; }
            }
            u::copy3(&mut nl, bestn);
        } else if type_ == GEOM_CYLINDER {
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            let pr = size[0] - r;
            let pz = size[1] - p[2].abs();
            if pr < pz {
                dist = -pr;
                if r > MINVAL { nl[0] = p[0] / r; nl[1] = p[1] / r; } else { nl[0] = 1.0; }
            } else { dist = -pz; nl[2] = if p[2] >= 0.0 { 1.0 } else { -1.0 }; }
        } else {
            let q = [p[0] / size[0], p[1] / size[1], p[2] / size[2]];
            let qnorm = u::norm3(&q);
            let smin = min(size[0], min(size[1], size[2]));
            dist = (qnorm - 1.0) * smin;
            nl[0] = p[0] / (size[0] * size[0]);
            nl[1] = p[1] / (size[1] * size[1]);
            nl[2] = p[2] / (size[2] * size[2]);
            if u::normalize3(&mut nl) == 0.0 { nl[2] = 1.0; }
        }
    } else { return 1e30; }
    u::mul_mat_vec3(n, gmat, &nl);
    dist
}

// featRadius is omitted: only the omitted IPC pair-gap machinery calls it.

pub fn geom_verts(m: &Model, gi: i32, gpos: &[f64; 3], gmat: &[f64; 9], out: &mut [f64]) -> usize {
    let gi = gi as usize;
    let type_ = m.geom_type[gi];
    if type_ == GEOM_SPHERE {
        for k in 0..3 { out[k] = gpos[k]; }
        return 1;
    }
    if type_ == GEOM_CAPSULE {
        let axis = [0.0, 0.0, m.geom_size[3 * gi + 1]];
        let mut wa = [0.0; 3];
        u::mul_mat_vec3(&mut wa, gmat, &axis);
        for k in 0..3 { out[k] = gpos[k] - wa[k]; out[3 + k] = gpos[k] + wa[k]; }
        return 2;
    }
    if type_ == GEOM_BOX {
        let size = at3(&m.geom_size, 3 * gi);
        let mut n = 0;
        for sx in (-1..=1).step_by(2) {
            for sy in (-1..=1).step_by(2) {
                for sz in (-1..=1).step_by(2) {
                    let loc = [sx as f64 * size[0], sy as f64 * size[1], sz as f64 * size[2]];
                    let mut wc = [0.0; 3];
                    u::mul_mat_vec3(&mut wc, gmat, &loc);
                    for k in 0..3 { out[3 * n + k] = gpos[k] + wc[k]; }
                    n += 1;
                }
            }
        }
        return n;
    }
    if type_ == GEOM_MESH {
        let mid = m.geom_dataid[gi] as usize;
        let nv = m.mesh_vertnum[mid] as usize;
        let vb = &m.mesh_vert[3 * m.mesh_vertadr[mid] as usize..];
        for i in 0..nv {
            let lv = [vb[3 * i] as f64, vb[3 * i + 1] as f64, vb[3 * i + 2] as f64];
            let mut wv = [0.0; 3];
            u::mul_mat_vec3(&mut wv, gmat, &lv);
            for k in 0..3 { out[3 * i + k] = gpos[k] + wv[k]; }
        }
        return nv;
    }
    0
}

pub fn geom_edges(m: &Model, gi: i32, gpos: &[f64; 3], gmat: &[f64; 9], out: &mut [f64]) -> usize {
    let type_ = m.geom_type[gi as usize];
    if type_ == GEOM_CAPSULE { return if geom_verts(m, gi, gpos, gmat, out) == 2 { 1 } else { 0 }; }
    let gi = gi as usize;
    if type_ == GEOM_BOX {
        let size = at3(&m.geom_size, 3 * gi);
        let mut n = 0;
        for axis in 0..3 {
            let a1 = (axis + 1) % 3;
            let a2 = (axis + 2) % 3;
            for s1 in (-1..=1).step_by(2) {
                for s2 in (-1..=1).step_by(2) {
                    let mut lo = [0.0; 3]; let mut hi = [0.0; 3];
                    let mut w1 = [0.0; 3]; let mut w2 = [0.0; 3];
                    lo[axis] = -size[axis]; hi[axis] = size[axis];
                    hi[a1] = s1 as f64 * size[a1]; lo[a1] = hi[a1];
                    hi[a2] = s2 as f64 * size[a2]; lo[a2] = hi[a2];
                    u::mul_mat_vec3(&mut w1, gmat, &lo);
                    u::mul_mat_vec3(&mut w2, gmat, &hi);
                    for k in 0..3 { out[6 * n + k] = gpos[k] + w1[k]; out[6 * n + 3 + k] = gpos[k] + w2[k]; }
                    n += 1;
                }
            }
        }
        return n;
    }
    if type_ == GEOM_MESH {
        let mid = m.geom_dataid[gi] as usize;
        let vb = &m.mesh_vert[3 * m.mesh_vertadr[mid] as usize..];
        let pa = m.mesh_polyadr[mid] as usize;
        let pn = m.mesh_polynum[mid] as usize;
        let mut n = 0;
        for p in 0..pn {
            let adr = m.mesh_polyvertadr[pa + p] as usize;
            let nvp = m.mesh_polyvertnum[pa + p] as usize;
            for j in 0..nvp {
                let a = m.mesh_polyvert[adr + j] as usize;
                let b = m.mesh_polyvert[adr + (j + 1) % nvp] as usize;
                if a >= b { continue; }
                let la = [vb[3 * a] as f64, vb[3 * a + 1] as f64, vb[3 * a + 2] as f64];
                let lb = [vb[3 * b] as f64, vb[3 * b + 1] as f64, vb[3 * b + 2] as f64];
                let mut wa = [0.0; 3]; let mut wb = [0.0; 3];
                u::mul_mat_vec3(&mut wa, gmat, &la);
                u::mul_mat_vec3(&mut wb, gmat, &lb);
                for k in 0..3 { out[6 * n + k] = gpos[k] + wa[k]; out[6 * n + 3 + k] = gpos[k] + wb[k]; }
                n += 1;
            }
        }
        return n;
    }
    0
}

pub fn standoff(band: f64, cap: f64) -> f64 { if band < cap { band } else { cap } }

// IPC is omitted: gapNormal, mjcFlexPair, mjc_pairGap, mjc_pairVerts, mjc_pairBand,
// conGapAdv, mjc_advance, bandOffset, sinkPush, addCand, bvhBox, candRun,
// candSegment, candUnits, candTask, mjc_candidates, mjc_ipcOwnsFlexFlex and
// mjc_ipcOwnsFlexGeom, plus their candidate structs. The static helpers
// maskFiltered and sameBody are omitted with their IPC callers.

pub fn geom_step_motion(m: &Model, d: &Data, g: i32, t: f64, dx: &mut [f64; 3], dw: &mut [f64; 3]) -> f64 {
    u::zero3(dx); u::zero3(dw);
    if g < 0 || m.body_weldid[m.geom_bodyid[g as usize] as usize] == 0 { return 0.0; }
    let g = g as usize;
    let bodyid = m.geom_bodyid[g] as usize;
    let mut cvel = [0.0; 6];
    let mut b = m.body_weldid[bodyid];
    while b > 0 {
        let bda = m.body_dofadr[b as usize] as usize;
        let dofnum = m.body_dofnum[b as usize] as usize;
        for j in 0..dofnum {
            let qv = d.qvel[bda + j];
            if qv != 0.0 {
                let cdof = &d.cdof[6 * (bda + j)..];
                for k in 0..6 { cvel[k] += cdof[k] * qv; }
            }
        }
        b = m.body_parentid[b as usize];
    }
    let mut gvel = [0.0; 6];
    transform_spatial(&mut gvel, &cvel, 0, at3(&d.geom_xpos, 3 * g), at3(&d.subtree_com, 3 * m.body_rootid[bodyid] as usize), None);
    if m.geom_type[g] != GEOM_SPHERE { inl::scl3(dw, at3(&gvel, 0), t); }
    inl::scl3(dx, at3(&gvel, 3), t);
    u::norm3(dx) + u::norm3(dw) * m.geom_rbound[g]
}

fn advance_geom_pose(obj: &mut CcdObj<'_>, dx: &[f64; 3], dw: &[f64; 3], step: f64) {
    inl::add_to_scl3(&mut obj.pos, dx, step);
    let wnorm = u::norm3(dw);
    if wnorm * step < MINVAL { return; }
    let mut axis = [0.0; 3]; let mut dq = [0.0; 4]; let mut dmat = [0.0; 9]; let mut mat = [0.0; 9];
    u::copy9(&mut mat, &obj.mat);
    inl::scl3(&mut axis, dw, 1.0 / wnorm);
    axis_angle2quat(&mut dq, &axis, wnorm * step);
    quat2mat(&mut dmat, &dq);
    u::mul_mat_mat3(&mut obj.mat, &dmat, &mat);
}

fn eval_geom_pair_dist<'a>(m: &Model, obj1: &mut CcdObj<'a>, obj2: &mut CcdObj<'a>, distmax: f64, x1: &mut [f64; 3], x2: &mut [f64; 3], normal: &mut [f64; 3]) -> f64 {
    let type1 = obj1.geom_type; let type2 = obj2.geom_type;
    let r1 = if type1 == GEOM_SPHERE || type1 == GEOM_CAPSULE { obj1.size[0] } else { 0.0 };
    let r2 = if type2 == GEOM_SPHERE || type2 == GEOM_CAPSULE { obj2.size[0] } else { 0.0 };
    let cutoff = distmax + r1 + r2;
    let has_plane = (type1 == GEOM_PLANE) as i32 - (type2 == GEOM_PLANE) as i32;
    if has_plane != 0 {
        let (plane, other) = if has_plane > 0 { (obj1, obj2) } else { (obj2, obj1) };
        let (x, xp) = if has_plane > 0 { (x2, x1) } else { (x1, x2) };
        let n = [plane.mat[2], plane.mat[5], plane.mat[8]];
        let mut s = [0.0; 3]; let dir = [-n[0], -n[1], -n[2]];
        other.support.call(&mut s, other, &dir);
        other.nsupp = 1;
        other.suppind[0] = other.vertindex.get();
        inl::copy3(at3_mut(&mut other.suppvert, 0), &s);
        let r = if has_plane > 0 { r2 } else { r1 };
        inl::add_scl3(x, &s, &n, -r);
        let dist = u::dot3(&n, x) - u::dot3(&n, &plane.pos);
        inl::add_scl3(xp, x, &n, -dist);
        inl::scl3(normal, &n, has_plane as f64);
        return dist;
    }
    normal[0] = 0.0; normal[1] = 0.0; normal[2] = 1.0;
    let dist = gjk(obj1, obj2, x1, x2, cutoff, m.opt.ccd_tolerance, m.opt.ccd_iterations as usize);
    if dist <= cutoff {
        let mut diff = [x2[0] - x1[0], x2[1] - x1[1], x2[2] - x1[2]];
        if u::normalize3(&mut diff) > 0.0 { u::copy3(normal, &diff); }
        inl::add_to_scl3(x1, normal, r1);
        inl::add_to_scl3(x2, normal, -r2);
    }
    dist - (r1 + r2)
}

fn contact_dist(m: &Model, g1: i32, g2: i32, margin: f64, t: f64, closing_dist: f64) -> f64 {
    let mut solref = [0.0; NREF]; let mut solimp = [0.0; NIMP]; let mut friction = [0.0; 5]; let mut adhesion = 0.0;
    let mut condim = 0; let mut ipair = -1;
    for i in 0..m.npair {
        if (m.pair_geom1[i] == g1 && m.pair_geom2[i] == g2) || (m.pair_geom1[i] == g2 && m.pair_geom2[i] == g1) { ipair = i as i32; break; }
    }
    if ipair >= 0 {
        u::copy(&mut solref, &m.pair_solref[NREF * ipair as usize..], NREF);
        u::copy(&mut solimp, &m.pair_solimp[NIMP * ipair as usize..], NIMP);
    } else { contact_param(m, &mut condim, &mut solref, &mut solimp, &mut friction, &mut adhesion, g1, g2, -1, -1); }
    let source = solref; assign_ref(m, &mut solref, &source);
    let source = solimp; assign_imp(m, &mut solimp, &source);
    if (solref[0] > 0.0) ^ (solref[1] > 0.0) { default_sol_ref_imp(Some(&mut solref), None); }
    let dt = m.opt.timestep;
    let imp = min(MAXIMP, max(MINIMP, solimp[1]));
    let mut delta = max(1e-4, solimp[2]);
    if dt > 0.0 {
        let mut k0; let mut b0;
        if solref[0] > 0.0 {
            let tau_ref = if !disabled(m, DSBL_REFSAFE) && is_metric(m) == 0 { max(2.0 * dt, solref[0]) } else { max(MINVAL, solref[0]) };
            let d_ratio = max(MINVAL, solref[1]);
            k0 = (dt * dt) / max(MINVAL, tau_ref * tau_ref * d_ratio * d_ratio);
            b0 = 2.0 * dt / tau_ref;
        } else { k0 = -solref[0] * dt * dt; b0 = -solref[1] * dt; }
        if is_metric(m) != 0 {
            if !disabled(m, DSBL_REFSAFE) && solref[0] > 0.0 && k0 > imp {
                let excess = k0 / imp;
                let fmax = MAXIMP * (1.0 - imp) / max(MINVAL, imp * (1.0 - MAXIMP));
                k0 = imp;
                b0 = min(b0 / excess.sqrt(), imp * max(0.0, fmax - 2.0));
            }
            if k0 > MINVAL { let rem = 1.0 - t - t * (b0 + k0); delta = (rem / k0) * closing_dist; }
        } else if k0 > MINVAL { let rem = 1.0 - t - b0; delta = (rem / k0) * closing_dist; }
    }
    margin - delta
}

fn pullback_geom_point(x_0: &mut [f64; 3], x_t: &[f64; 3], obj: &CcdObj<'_>, d: &Data) {
    let g = obj.geom as usize;
    let mut diff = [0.0; 3]; let mut local = [0.0; 3];
    inl::sub3(&mut diff, x_t, &obj.pos);
    u::mul_mat_tvec3(&mut local, &obj.mat, &diff);
    inl::mul_mat_vec3(x_0, at9(&d.geom_xmat, 9 * g), &local);
    u::add_to3(x_0, at3(&d.geom_xpos, 3 * g));
}

fn is_body_pair_excluded(m: &Model, b1: i32, b2: i32) -> bool {
    let sig = ((if b1 < b2 { b1 } else { b2 }) as u32).wrapping_shl(16).wrapping_add((if b1 > b2 { b1 } else { b2 }) as u32);
    for i in 0..m.nexclude { if m.exclude_signature[i] as u32 == sig { return true; } }
    false
}

fn find_explicit_geom_pair(m: &Model, g1: i32, g2: i32) -> i32 {
    for i in 0..m.npair { if (m.pair_geom1[i] == g1 && m.pair_geom2[i] == g2) || (m.pair_geom1[i] == g2 && m.pair_geom2[i] == g1) { return i as i32; } }
    -1
}

fn is_geom_pair_within_margin(m: &Model, d: &Data, g1: i32, g2: i32, margin: f64) -> bool {
    let type1 = m.geom_type[g1 as usize]; let type2 = m.geom_type[g2 as usize];
    if type1 == GEOM_PLANE && type2 == GEOM_PLANE { return false; }
    let mut obj1 = CcdObj::default(); let mut obj2 = CcdObj::default();
    init_ccd_obj(&mut obj1, m, Some(d), g1, 0.0);
    init_ccd_obj(&mut obj2, m, Some(d), g2, 0.0);
    if type1 == GEOM_SPHERE { obj1.support = CcdSupport::Point; }
    if type1 == GEOM_CAPSULE { obj1.support = CcdSupport::Line; }
    if type2 == GEOM_SPHERE { obj2.support = CcdSupport::Point; }
    if type2 == GEOM_CAPSULE { obj2.support = CcdSupport::Line; }
    let tol = margin + m.opt.ccd_tolerance;
    let mut x1 = [0.0; 3]; let mut x2 = [0.0; 3]; let mut normal = [0.0; 3];
    inl::copy3(&mut x1, &obj1.pos); inl::copy3(&mut x2, &obj2.pos);
    eval_geom_pair_dist(m, &mut obj1, &mut obj2, tol, &mut x1, &mut x2, &mut normal) <= tol
}

fn is_ray_occluded(m: &Model, d: &Data, g1: i32, g2: i32, a: &[f64; 3], b: &[f64; 3]) -> bool {
    let mut ab = [0.0; 3]; inl::sub3(&mut ab, b, a);
    let len2 = u::dot3(&ab, &ab);
    if len2 < MINVAL * MINVAL { return false; }
    let eps = 1e-5;
    let mut pnt = [0.0; 3]; let mut vec = [0.0; 3];
    inl::add_scl3(&mut pnt, a, &ab, eps); inl::scl3(&mut vec, &ab, 1.0 - 2.0 * eps);
    let mut mid = [0.0; 3]; u::add_scl3(&mut mid, a, &ab, 0.5);
    let half_len = 0.5 * len2.sqrt();
    let b1 = m.geom_bodyid[g1 as usize]; let b2 = m.geom_bodyid[g2 as usize];
    let weld1 = m.body_weldid[b1 as usize]; let weld2 = m.body_weldid[b2 as usize];
    let p1 = m.body_weldid[m.body_parentid[weld1 as usize] as usize];
    let fltparent = !disabled(m, DSBL_FILTERPARENT);
    let ct1 = m.geom_contype[g1 as usize]; let ca1 = m.geom_conaffinity[g1 as usize];
    for g3 in 0..m.ngeom {
        let type_ = m.geom_type[g3];
        if g3 as i32 == g1 || g3 as i32 == g2 || geom_supported(type_) == 0 { continue; }
        if m.geom_rbound[g3] > 0.0 {
            let mut diff = [0.0; 3]; inl::sub3(&mut diff, at3(&d.geom_xpos, 3 * g3), &mid);
            let max_r = m.geom_rbound[g3] + half_len;
            if u::dot3(&diff, &diff) > max_r * max_r { continue; }
        }
        let b3 = m.geom_bodyid[g3]; let weld3 = m.body_weldid[b3 as usize];
        if weld3 == weld1 || (weld3 == weld2 && weld2 != 0) { continue; }
        let p3 = m.body_weldid[m.body_parentid[weld3 as usize] as usize];
        let ct3 = m.geom_contype[g3]; let ca3 = m.geom_conaffinity[g3];
        let ipair = find_explicit_geom_pair(m, g1, g3 as i32);
        if ipair < 0 {
            if !((ct1 & ca3) != 0 || (ct3 & ca1) != 0) || is_body_pair_excluded(m, b1, b3) || (fltparent && weld1 != 0 && weld3 != 0 && (weld1 == p3 || weld3 == p1)) { continue; }
        }
        let hit;
        if type_ == GEOM_PLANE {
            let gpos = at3(&d.geom_xpos, 3 * g3); let gmat = at9(&d.geom_xmat, 9 * g3);
            let n = [gmat[2], gmat[5], gmat[8]]; let mut diff = [0.0; 3];
            inl::sub3(&mut diff, a, gpos);
            let da = u::dot3(&diff, &n); let db = da + u::dot3(&ab, &n);
            hit = da * db <= 0.0 && (da.abs() > eps || db.abs() > eps);
        } else if type_ == GEOM_MESH {
            let x = ray_mesh(m, d, g3 as i32, &pnt, &vec, None); hit = x >= 0.0 && x <= 1.0;
        } else {
            let x = ray_geom(at3(&d.geom_xpos, 3 * g3), Some(at9(&d.geom_xmat, 9 * g3)), at3(&m.geom_size, 3 * g3), &pnt, &vec, type_, None); hit = x >= 0.0 && x <= 1.0;
        }
        if hit {
            let margin3 = assign_margin(m, if ipair >= 0 { m.pair_margin[ipair as usize] } else { m.geom_margin[g1 as usize] + m.geom_margin[g3] });
            if !is_geom_pair_within_margin(m, d, g1, g3 as i32, margin3) { return true; }
        }
    }
    false
}

// Conservative advancement over the timestep; at most four output contacts.
pub fn advance_geom_pair(m: &Model, d: &Data, g1: i32, g2: i32, margin: f64, con: Option<&mut [PreContact]>, buffer: Option<&mut CcdBuffer>) -> usize {
    let type1 = if g1 >= 0 { m.geom_type[g1 as usize] } else { -1 };
    let type2 = if g2 >= 0 { m.geom_type[g2 as usize] } else { -1 };
    if geom_supported(type1) == 0 || geom_supported(type2) == 0 || (type1 == GEOM_PLANE && type2 == GEOM_PLANE) { return 0; }
    let mut dx1 = [0.0; 3]; let mut dw1 = [0.0; 3]; let mut dx2 = [0.0; 3]; let mut dw2 = [0.0; 3];
    let s1 = geom_step_motion(m, d, g1, m.opt.timestep, &mut dx1, &mut dw1);
    let s2 = geom_step_motion(m, d, g2, m.opt.timestep, &mut dx2, &mut dw2);
    let sweep = s1 + s2;
    if sweep < MINVAL { return 0; }
    let mut dx = [0.0; 3]; inl::sub3(&mut dx, &dx1, &dx2);
    let rot_sweep = (s1 - u::norm3(&dx1)) + (s2 - u::norm3(&dx2));
    let mut obj1 = CcdObj::default(); let mut obj2 = CcdObj::default();
    init_ccd_obj(&mut obj1, m, Some(d), g1, 0.0); init_ccd_obj(&mut obj2, m, Some(d), g2, 0.0);
    if type1 == GEOM_SPHERE { obj1.support = CcdSupport::Point; }
    if type1 == GEOM_CAPSULE { obj1.support = CcdSupport::Line; }
    if type2 == GEOM_SPHERE { obj2.support = CcdSupport::Point; }
    if type2 == GEOM_CAPSULE { obj2.support = CcdSupport::Line; }
    let r1 = if type1 == GEOM_SPHERE || type1 == GEOM_CAPSULE { obj1.size[0] } else { 0.0 };
    let r2 = if type2 == GEOM_SPHERE || type2 == GEOM_CAPSULE { obj2.size[0] } else { 0.0 };
    let distmax = sweep + margin;
    let mut x1 = [0.0; 3]; let mut x2 = [0.0; 3]; let mut normal = [0.0; 3];
    inl::copy3(&mut x1, &obj1.pos); inl::copy3(&mut x2, &obj2.pos);
    let dist0 = eval_geom_pair_dist(m, &mut obj1, &mut obj2, distmax, &mut x1, &mut x2, &mut normal);
    if dist0 <= margin || dist0 - margin > sweep { return 0; }
    let target = 0.02 * (dist0 - margin) + margin;
    let rel_tol = max(m.opt.ccd_tolerance, 0.5 * (target - margin));
    let mut t = 0.0; let mut dist = dist0;
    // The 16-step cap is the reference algorithm's cap, including its fallback below.
    for _it in 0..16 {
        let clearance = dist - target;
        if clearance <= rel_tol { break; }
        let dir_sweep = u::dot3(&normal, &dx) + rot_sweep;
        if dir_sweep <= MINVAL { return 0; }
        let step = clearance / dir_sweep;
        t += step;
        if t >= 1.0 { return 0; }
        let mut core1 = [0.0; 3]; let mut core2 = [0.0; 3]; let mut diff1 = [0.0; 3]; let mut diff2 = [0.0; 3]; let mut local1 = [0.0; 3]; let mut local2 = [0.0; 3];
        inl::add_scl3(&mut core1, &x1, &normal, -r1); inl::add_scl3(&mut core2, &x2, &normal, r2);
        inl::sub3(&mut diff1, &core1, &obj1.pos); inl::sub3(&mut diff2, &core2, &obj2.pos);
        u::mul_mat_tvec3(&mut local1, &obj1.mat, &diff1); u::mul_mat_tvec3(&mut local2, &obj2.mat, &diff2);
        advance_geom_pose(&mut obj1, &dx1, &dw1, step); advance_geom_pose(&mut obj2, &dx2, &dw2, step);
        let mut tmp1 = [0.0; 3]; let mut tmp2 = [0.0; 3];
        inl::mul_mat_vec3(&mut tmp1, &obj1.mat, &local1); u::add3(&mut x1, &obj1.pos, &tmp1);
        inl::mul_mat_vec3(&mut tmp2, &obj2.mat, &local2); u::add3(&mut x2, &obj2.pos, &tmp2);
        dist = eval_geom_pair_dist(m, &mut obj1, &mut obj2, distmax, &mut x1, &mut x2, &mut normal);
        if dist - margin > (1.0 - t) * sweep { return 0; }
    }
    let mut rel1 = [0.0; 3]; let mut rel2 = [0.0; 3]; let mut rot1 = [0.0; 3]; let mut rot2 = [0.0; 3]; let mut dp1 = [0.0; 3]; let mut dp2 = [0.0; 3]; let mut drel = [0.0; 3];
    inl::sub3(&mut rel1, &x1, &obj1.pos); inl::sub3(&mut rel2, &x2, &obj2.pos);
    inl::cross(&mut rot1, &dw1, &rel1); inl::cross(&mut rot2, &dw2, &rel2);
    u::add3(&mut dp1, &dx1, &rot1); u::add3(&mut dp2, &dx2, &rot2);
    inl::sub3(&mut drel, &dp1, &dp2);
    let vn = u::dot3(&normal, &drel);
    let closing_dist = max(vn, (dist0 - dist) / max(t, MINVAL));
    if dist - target > rel_tol {
        if vn <= MINVAL || dist - margin > (1.0 - t) * vn { return 0; }
        t += (dist - target) / vn;
    }
    if closing_dist <= 0.0 { return 0; }
    let mut ncon = 0;
    if let Some(con) = con {
        let mut nx = 1;
        let mut x1_all = [0.0; 12]; let mut x2_all = [0.0; 12]; let mut dist_all = [0.0; 4];
        inl::copy3(at3_mut(&mut x1_all, 0), &x1); inl::copy3(at3_mut(&mut x2_all, 0), &x2); dist_all[0] = dist;
        let has_plane = type1 == GEOM_PLANE || type2 == GEOM_PLANE;
        if let Some(buffer) = buffer {
            if has_plane || !disabled(m, DSBL_MULTICCD) {
                let pad = 2.0 * max(0.0, dist) + (1.0 - t) * closing_dist + m.opt.ccd_tolerance;
                obj1.margin = pad; obj2.margin = pad;
                inl::add_to_scl3(at3_mut(&mut x1_all, 0), &normal, 0.5 * pad);
                inl::add_to_scl3(at3_mut(&mut x2_all, 0), &normal, -0.5 * pad);
                nx = multicontact(&obj1, &obj2, &mut x1_all, &mut x2_all, Some(&mut dist_all), 4, m.nmeshdegmax, m.npolygonmax, buffer);
                for i in 0..nx {
                    inl::add_to_scl3(at3_mut(&mut x1_all, 3 * i), &normal, -0.5 * pad);
                    inl::add_to_scl3(at3_mut(&mut x2_all, 3 * i), &normal, 0.5 * pad);
                }
            }
        }
        let cdist = contact_dist(m, g1, g2, margin, t, closing_dist);
        // As in C, the slower body retains a lever-arm error from tangential travel.
        let w1 = s1 / sweep; let w2 = s2 / sweep;
        let g_fast = if s1 >= s2 { g1 } else { g2 }; let g_slow = if s1 >= s2 { g2 } else { g1 };
        for i in 0..nx {
            let x1_t = at3(&x1_all, 3 * i); let x2_t = at3(&x2_all, 3 * i);
            let mut x1_0 = [0.0; 3]; let mut x2_0 = [0.0; 3];
            pullback_geom_point(&mut x1_0, x1_t, &obj1, d); pullback_geom_point(&mut x2_0, x2_t, &obj2, d);
            if is_ray_occluded(m, d, g1, g2, &x1_0, x1_t) || is_ray_occluded(m, d, g2, g1, &x2_0, x2_t) || is_ray_occluded(m, d, g_fast, g_slow, x1_t, x2_t) { continue; }
            con[ncon].pos[0] = w1 * x1_0[0] + w2 * x2_0[0];
            con[ncon].pos[1] = w1 * x1_0[1] + w2 * x2_0[1];
            con[ncon].pos[2] = w1 * x1_0[2] + w2 * x2_0[2];
            con[ncon].dist = cdist;
            u::copy3(&mut con[ncon].normal, &normal); u::zero3(&mut con[ncon].tangent);
            ncon += 1;
        }
    } else {
        let g_fast = if s1 >= s2 { g1 } else { g2 }; let g_slow = if s1 >= s2 { g2 } else { g1 };
        let mut x1_0 = [0.0; 3]; let mut x2_0 = [0.0; 3];
        pullback_geom_point(&mut x1_0, &x1, &obj1, d); pullback_geom_point(&mut x2_0, &x2, &obj2, d);
        if !is_ray_occluded(m, d, g1, g2, &x1_0, &x1) && !is_ray_occluded(m, d, g2, g1, &x2_0, &x2) && !is_ray_occluded(m, d, g_fast, g_slow, &x1, &x2) { ncon = 1; }
    }
    ncon
}
