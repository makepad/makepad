//! Model constants: bounds, fixed quantities, reference configurations and statistics.
//!
//! Translated from `src/engine/engine_setconst.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::core_smooth::{com_pos, factor_m, flex_skip, kinematics, make_m, solve_m, tendon, transmission};
use crate::core_util::{actuator_armature, eff_flex_possible, jac_body_com, jac_dif_pair, local2global, tendon_has_damping, tendon_has_stiffness};
use crate::data::Data;
use crate::inline::{at3, at3_mut, at4, at4_mut, at6, at9, at9_mut, at10};
use crate::memory::clear_efc;
use crate::metric::eff_chol_set_const;
use crate::model::Model;
use crate::sleep::update_sleep;
use crate::types::*;
use crate::util_blas as blas;
use crate::util_errmem::error;
use crate::util_misc::{self, max, min};
use crate::util_sparse::sparse2dense;
use crate::util_spatial as spatial;

// engine_setconst.h: groups in MJSETCONST_ARRAYS order.
pub const SCGROUP_FIXED: i32 = 1 << 0;
pub const SCGROUP_KIN0: i32 = 1 << 1;
pub const SCGROUP_INERT0: i32 = 1 << 2;
pub const SCGROUP_SPRING: i32 = 1 << 3;
pub const SCGROUP_ALL: i32 = SCGROUP_FIXED | SCGROUP_KIN0 | SCGROUP_INERT0 | SCGROUP_SPRING;

// Compute dof_M0 via composite rigid body algorithm.
fn set_m0(m: &mut Model, d: &mut Data) {
    let mut buf = [0.0; 6];
    let last_body = m.nbody - 1;
    let nv = m.nv;
    blas::copy(&mut d.crb, &d.cinert, 10 * m.nbody);
    for i in (1..=last_body).rev() {
        if m.body_parentid[i] > 0 {
            let source = *at10(&d.crb, 10 * i);
            blas::add_to(&mut d.crb[10 * m.body_parentid[i] as usize..], &source, 10);
        }
    }
    for i in 0..nv {
        spatial::mul_inert_vec(&mut buf, at10(&d.crb, 10 * m.dof_bodyid[i] as usize), at6(&d.cdof, 6 * i));
        let armature = m.dof_armature[i] + actuator_armature(m, OBJ_JOINT, m.dof_jntid[i] as usize);
        m.dof_m0[i] = armature + blas::dot(&d.cdof[6 * i..], &buf, 6);
    }
}

fn get_wrap_body_tree_id(m: &Model, wrap_index: usize) -> i32 {
    let mut bodyid = -1;
    let objid = m.wrap_objid[wrap_index];
    match m.wrap_type[wrap_index] {
        WRAP_JOINT => bodyid = m.jnt_bodyid[objid as usize],
        WRAP_SITE => bodyid = m.site_bodyid[objid as usize],
        WRAP_SPHERE | WRAP_CYLINDER => bodyid = m.geom_bodyid[objid as usize],
        _ => {}
    }
    if bodyid != -1 { m.body_treeid[bodyid as usize] } else { -1 }
}

const K_SAME_FRAME_EPS: f64 = 1e-6;

fn is_null_vec3(v: &[f64; 3]) -> bool {
    v[0].abs() < K_SAME_FRAME_EPS && v[1].abs() < K_SAME_FRAME_EPS && v[2].abs() < K_SAME_FRAME_EPS
}

fn is_null_quat(q: &[f64; 4]) -> bool {
    let plus = (q[0] - 1.0).abs() < K_SAME_FRAME_EPS && q[1].abs() < K_SAME_FRAME_EPS && q[2].abs() < K_SAME_FRAME_EPS && q[3].abs() < K_SAME_FRAME_EPS;
    let minus = (q[0] + 1.0).abs() < K_SAME_FRAME_EPS && q[1].abs() < K_SAME_FRAME_EPS && q[2].abs() < K_SAME_FRAME_EPS && q[3].abs() < K_SAME_FRAME_EPS;
    plus || minus
}

fn is_same_quat(q1: &[f64; 4], q2: &[f64; 4]) -> bool {
    let plus = (q1[0] - q2[0]).abs() < K_SAME_FRAME_EPS && (q1[1] - q2[1]).abs() < K_SAME_FRAME_EPS && (q1[2] - q2[2]).abs() < K_SAME_FRAME_EPS && (q1[3] - q2[3]).abs() < K_SAME_FRAME_EPS;
    let minus = (q1[0] + q2[0]).abs() < K_SAME_FRAME_EPS && (q1[1] + q2[1]).abs() < K_SAME_FRAME_EPS && (q1[2] + q2[2]).abs() < K_SAME_FRAME_EPS && (q1[3] + q2[3]).abs() < K_SAME_FRAME_EPS;
    plus || minus
}

fn is_same_vec3(v1: &[f64; 3], v2: &[f64; 3]) -> bool {
    (v1[0] - v2[0]).abs() < K_SAME_FRAME_EPS && (v1[1] - v2[1]).abs() < K_SAME_FRAME_EPS && (v1[2] - v2[2]).abs() < K_SAME_FRAME_EPS
}

fn set_sameframe(m: &mut Model) {
    for i in 1..m.nbody {
        let ipos = at3(&m.body_ipos, 3 * i);
        let iquat = at4(&m.body_iquat, 4 * i);
        m.body_sameframe[i] = if is_null_vec3(ipos) && is_null_quat(iquat) { SAMEFRAME_BODY }
            else if is_null_quat(iquat) { SAMEFRAME_BODYROT } else { SAMEFRAME_NONE } as u8;
    }
    for i in 0..m.ngeom {
        let b = m.geom_bodyid[i] as usize;
        let gpos = at3(&m.geom_pos, 3 * i);
        let gquat = at4(&m.geom_quat, 4 * i);
        let ipos = at3(&m.body_ipos, 3 * b);
        let iquat = at4(&m.body_iquat, 4 * b);
        m.geom_sameframe[i] = if is_null_vec3(gpos) && is_null_quat(gquat) { SAMEFRAME_BODY }
            else if is_null_quat(gquat) { SAMEFRAME_BODYROT }
            else if is_same_vec3(gpos, ipos) && is_same_quat(gquat, iquat) { SAMEFRAME_INERTIA }
            else if is_same_quat(gquat, iquat) { SAMEFRAME_INERTIAROT } else { SAMEFRAME_NONE } as u8;
    }
    for i in 0..m.nsite {
        let b = m.site_bodyid[i] as usize;
        let spos = at3(&m.site_pos, 3 * i);
        let squat = at4(&m.site_quat, 4 * i);
        let ipos = at3(&m.body_ipos, 3 * b);
        let iquat = at4(&m.body_iquat, 4 * b);
        m.site_sameframe[i] = if is_null_vec3(spos) && is_null_quat(squat) { SAMEFRAME_BODY }
            else if is_null_quat(squat) { SAMEFRAME_BODYROT }
            else if is_same_vec3(spos, ipos) && is_same_quat(squat, iquat) { SAMEFRAME_INERTIA }
            else if is_same_quat(squat, iquat) { SAMEFRAME_INERTIAROT } else { SAMEFRAME_NONE } as u8;
    }
}

const K_BOUND_EPS: f64 = 1e-14;

fn geom_bound(m: &Model, g: usize, aamm: &mut [f64; 6], rbound: &mut f64) -> i32 {
    let size = at3(&m.geom_size, 3 * g);
    match m.geom_type[g] {
        GEOM_MESH | GEOM_SDF => return 0,
        GEOM_HFIELD => {
            let hsize = at4(&m.hfield_size, 4 * m.geom_dataid[g] as usize);
            aamm[0] = -hsize[0]; aamm[1] = -hsize[1]; aamm[2] = -hsize[3];
            aamm[3] = hsize[0]; aamm[4] = hsize[1]; aamm[5] = hsize[2];
            *rbound = (hsize[0] * hsize[0] + hsize[1] * hsize[1] + max(hsize[2] * hsize[2], hsize[3] * hsize[3])).sqrt();
            return 1;
        }
        GEOM_PLANE => {
            aamm[0] = -MAXVAL; aamm[1] = -MAXVAL; aamm[2] = -MAXVAL;
            aamm[3] = MAXVAL; aamm[4] = MAXVAL; aamm[5] = 0.0;
            *rbound = 0.0;
            return 1;
        }
        GEOM_SPHERE => { aamm[3] = size[0]; aamm[4] = size[0]; aamm[5] = size[0]; *rbound = size[0]; }
        GEOM_CAPSULE => { aamm[3] = size[0]; aamm[4] = size[0]; aamm[5] = size[0] + size[1]; *rbound = size[0] + size[1]; }
        GEOM_CYLINDER => { aamm[3] = size[0]; aamm[4] = size[0]; aamm[5] = size[1]; *rbound = (size[0] * size[0] + size[1] * size[1]).sqrt(); }
        GEOM_ELLIPSOID => { blas::copy3(at3_mut(aamm, 3), size); *rbound = max(max(size[0], size[1]), size[2]); }
        GEOM_BOX => { blas::copy3(at3_mut(aamm, 3), size); *rbound = (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt(); }
        _ => { blas::copy3(at3_mut(aamm, 3), size); *rbound = 0.0; }
    }
    aamm[0] = -aamm[3]; aamm[1] = -aamm[4]; aamm[2] = -aamm[5];
    1
}

fn set_bound(m: &mut Model, d: &mut Data) {
    for i in 0..m.ngeom {
        if m.geom_type[i] == GEOM_HFIELD {
            let hsize = at4(&m.hfield_size, 4 * m.geom_dataid[i] as usize);
            let size = at3_mut(&mut m.geom_size, 3 * i);
            size[0] = hsize[0]; size[1] = hsize[1]; size[2] = 0.25 * hsize[2] + 0.5 * hsize[3];
        }
        let mut aamm = [0.0; 6];
        let mut rbound = 0.0;
        if geom_bound(m, i, &mut aamm, &mut rbound) != 0 {
            for k in 0..3 {
                m.geom_aabb[6 * i + k] = (aamm[k + 3] + aamm[k]) / 2.0;
                m.geom_aabb[6 * i + k + 3] = (aamm[k + 3] - aamm[k]) / 2.0;
            }
            m.geom_rbound[i] = rbound;
        }
    }
    for i in 0..m.nbody {
        let mut contype = 0;
        let mut conaffinity = 0;
        let mut margin = 0.0;
        for g in m.body_geomadr[i]..m.body_geomadr[i] + m.body_geomnum[i] {
            let g = g as usize;
            contype |= m.geom_contype[g]; conaffinity |= m.geom_conaffinity[g];
            margin = max(margin, m.geom_margin[g] + m.geom_gap[g]);
        }
        m.body_contype[i] = contype; m.body_conaffinity[i] = conaffinity; m.body_margin[i] = margin;
    }
    let mut nbvhmax = 0;
    for i in 0..m.nbody { nbvhmax = nbvhmax.max(m.body_bvhnum[i]); }
    let mut node = d.scratch.num(6 * nbvhmax.max(1) as usize);
    let mut inbvh = d.scratch.int(m.ngeom.max(1));
    util_misc::zero_int(&mut inbvh, m.ngeom);
    for i in 0..m.nbody {
        let adr = m.body_bvhadr[i];
        let num = m.body_bvhnum[i];
        if adr < 0 { continue; }
        let adr = adr as usize;
        let ipos = at3(&m.body_ipos, 3 * i);
        let iquat = at4(&m.body_iquat, 4 * i);
        let mut qinv = [0.0; 4];
        spatial::neg_quat(&mut qinv, iquat);
        for j in (0..num as usize).rev() {
            let g = m.bvh_nodeid[adr + j];
            if g >= 0 {
                let g = g as usize;
                inbvh[g] = 1;
                let aabb = at6(&m.geom_aabb, 6 * g);
                let gpos = at3(&m.geom_pos, 3 * g);
                let gquat = at4(&m.geom_quat, 4 * g);
                node[6 * j..6 * j + 3].fill(MAXVAL);
                node[6 * j + 3..6 * j + 6].fill(-MAXVAL);
                for v in 0..8 {
                    let mut vert = [aabb[0] + if v & 1 != 0 { aabb[3] } else { -aabb[3] },
                        aabb[1] + if v & 2 != 0 { aabb[4] } else { -aabb[4] },
                        aabb[2] + if v & 4 != 0 { aabb[5] } else { -aabb[5] }];
                    let mut box_ = [0.0; 3];
                    spatial::rot_vec_quat(&mut box_, &vert, gquat);
                    box_[0] += gpos[0] - ipos[0]; box_[1] += gpos[1] - ipos[1]; box_[2] += gpos[2] - ipos[2];
                    spatial::rot_vec_quat(&mut vert, &box_, &qinv);
                    for k in 0..3 { node[6 * j + k] = min(node[6 * j + k], vert[k]); node[6 * j + k + 3] = max(node[6 * j + k + 3], vert[k]); }
                }
            } else {
                let child1 = *at6(&node, 6 * m.bvh_child[2 * (adr + j)] as usize);
                let child2 = *at6(&node, 6 * m.bvh_child[2 * (adr + j) + 1] as usize);
                for k in 0..3 { node[6 * j + k] = min(child1[k], child2[k]); node[6 * j + k + 3] = max(child1[k + 3], child2[k + 3]); }
            }
        }
        for j in 0..num as usize {
            let aamm = at6(&node, 6 * j);
            for k in 0..3 {
                let mut lo = aamm[k]; let mut hi = aamm[k + 3];
                if (lo - hi).abs() < K_BOUND_EPS { lo -= K_BOUND_EPS; hi += K_BOUND_EPS; }
                m.bvh_aabb[6 * (adr + j) + k] = (hi + lo) / 2.0;
                m.bvh_aabb[6 * (adr + j) + k + 3] = (hi - lo) / 2.0;
            }
        }
    }
    for i in 0..m.ngeom {
        let b = m.geom_bodyid[i] as usize;
        if m.body_bvhadr[b] >= 0 && inbvh[i] == 0 && (m.geom_contype[i] != 0 || m.geom_conaffinity[i] != 0) {
            d.scratch.put_num(node); d.scratch.put_int(inbvh);
            error(&format!("geom {} collides but was not compiled to collide, recompile the model", i));
        }
    }
    d.scratch.put_num(node); d.scratch.put_int(inbvh);
}

// 15-point Gauss-Kronrod quadrature, l = x^3 / (1-x)^2.
fn added_mass_kappa(dx: f64, dy: f64, dz: f64) -> f64 {
    // Unused K15 abscissae kronrod_x are omitted, as in the C.
    const KRONROD_W: [f64; 15] = [0.01146766, 0.03154605, 0.05239501, 0.07032663, 0.08450236,
        0.09517529, 0.10221647, 0.10474107, 0.10221647, 0.09517529,
        0.08450236, 0.07032663, 0.05239501, 0.03154605, 0.01146766];
    const KRONROD_L: [f64; 15] = [7.865151709349917e-08, 1.7347976913907274e-05, 0.0003548008144506193,
        0.002846636252924549, 0.014094260903596077, 0.053063261727396636,
        0.17041978741317773, 0.5, 1.4036301548686991,
        3.9353484827022642, 11.644841677041734, 39.53187807410903,
        177.5711362220801, 1429.4772912937397, 54087.416549217705];
    const KRONROD_D: [f64; 15] = [5.538677720489877e-05, 0.002080868285293228, 0.016514126520723166,
        0.07261900344370877, 0.23985243401862602, 0.6868318249020725,
        1.8551129519182894, 5.0, 14.060031152313941,
        43.28941239611009, 156.58546376397112, 747.9826085305024,
        5827.4042950027115, 116754.0197944512, 25482945.327264845];
    let invdx2 = 1.0 / (dx * dx);
    let invdy2 = 1.0 / (dy * dy);
    let invdz2 = 1.0 / (dz * dz);
    let scale = (dx * dx * dx * dy * dz).powf(0.4);
    let mut kappa = 0.0;
    for i in 0..15 {
        let lambda = scale * KRONROD_L[i];
        let denom = (1.0 + lambda * invdx2) * ((1.0 + lambda * invdx2) * (1.0 + lambda * invdy2) * (1.0 + lambda * invdz2)).sqrt();
        kappa += scale * KRONROD_D[i] / denom * KRONROD_W[i];
    }
    kappa * invdx2
}

fn set_fluid(m: &mut Model) {
    for i in 0..m.ngeom {
        let fluid = &mut m.geom_fluid[NFLUID * i..];
        if fluid[0] <= 0.0 { continue; }
        let mut semiaxes = [0.0; 3];
        util_misc::geom_semi_axes(&mut semiaxes, at3(&m.geom_size, 3 * i), m.geom_type[i]);
        let dx = semiaxes[0]; let dy = semiaxes[1]; let dz = semiaxes[2];
        let volume = 4.0 / 3.0 * PI * dx * dy * dz;
        let kx = added_mass_kappa(dx, dy, dz);
        let ky = added_mass_kappa(dy, dz, dx);
        let kz = added_mass_kappa(dz, dx, dy);
        let yz = dy * dy - dz * dz; let zx = dz * dz - dx * dx; let xy = dx * dx - dy * dy;
        let ixfac = yz * yz * (kz - ky).abs() / max(K_BOUND_EPS, (2.0 * yz + (dy * dy + dz * dz) * (ky - kz)).abs());
        let iyfac = zx * zx * (kx - kz).abs() / max(K_BOUND_EPS, (2.0 * zx + (dz * dz + dx * dx) * (kz - kx)).abs());
        let izfac = xy * xy * (ky - kx).abs() / max(K_BOUND_EPS, (2.0 * xy + (dx * dx + dy * dy) * (kx - ky)).abs());
        fluid[6] = volume * kx / max(K_BOUND_EPS, 2.0 - kx);
        fluid[7] = volume * ky / max(K_BOUND_EPS, 2.0 - ky);
        fluid[8] = volume * kz / max(K_BOUND_EPS, 2.0 - kz);
        fluid[9] = volume * ixfac / 5.0; fluid[10] = volume * iyfac / 5.0; fluid[11] = volume * izfac / 5.0;
    }
}

// Fixed quantities do not depend on qpos0.
fn set_fixed(m: &mut Model, d: &mut Data) {
    for i in 0..m.nbody { m.body_subtreemass[i] = m.body_mass[i]; }
    for i in (1..m.nbody).rev() {
        m.body_subtreemass[m.body_parentid[i] as usize] += m.body_subtreemass[i];
    }
    let mut ngravcomp = 0;
    for i in 0..m.nbody { ngravcomp += usize::from(m.body_gravcomp[i] > 0.0); }
    m.ngravcomp = ngravcomp;
    m.flg_gravcomp = u8::from(ngravcomp > 0);
    let mut flg_surfacevel = 0;
    for i in 0..m.ngeom {
        let sv = at6(&m.geom_surfacevel, 6 * i);
        if sv[0] != 0.0 || sv[1] != 0.0 || sv[2] != 0.0 || sv[3] != 0.0 || sv[4] != 0.0 || sv[5] != 0.0 {
            flg_surfacevel = 1; break;
        }
    }
    m.flg_surfacevel = flg_surfacevel;
    let mut flg_adhesion = 0;
    for i in 0..m.ngeom {
        if m.geom_adhesion[i] != 0.0 { flg_adhesion = 1; break; }
    }
    let mut i = 0;
    while i < m.npair && flg_adhesion == 0 {
        if m.pair_adhesion[i] != 0.0 { flg_adhesion = 1; }
        i += 1;
    }
    m.flg_adhesion = flg_adhesion;
    util_misc::fill_int(&mut m.jnt_actuatorid, -1, m.njnt);
    util_misc::fill_int(&mut m.tendon_actuatorid, -1, m.ntendon);
    for i in 0..m.nactuator {
        if m.actuator_damping[i] == 0.0 && util_misc::is_zero(&m.actuator_dampingpoly[NPOLY * i..], NPOLY) != 0 && m.actuator_armature[i] == 0.0 { continue; }
        if m.actuator_trntype[i] == TRN_JOINT || m.actuator_trntype[i] == TRN_JOINTINPARENT {
            let jntid = m.actuator_trnid[2 * i] as usize;
            if m.jnt_actuatorid[jntid] == -1 { m.jnt_actuatorid[jntid] = i as i32; }
            else { m.jnt_actuatorid[jntid] = -2; }
        } else if m.actuator_trntype[i] == TRN_TENDON {
            let tenid = m.actuator_trnid[2 * i] as usize;
            if m.tendon_actuatorid[tenid] == -1 { m.tendon_actuatorid[tenid] = i as i32; }
            else { m.tendon_actuatorid[tenid] = -2; }
        }
    }
    for i in 0..m.nbody {
        let weldid = m.body_weldid[i] as usize;
        if m.body_dofnum[weldid] != 0 { m.body_treeid[i] = m.dof_treeid[m.body_dofadr[weldid] as usize]; }
        else { m.body_treeid[i] = -1; }
    }
    util_misc::zero_int(&mut m.tree_bodynum, m.ntree);
    let mut tree_current = -1;
    for i in 1..m.nbody {
        let treeid = m.body_treeid[i];
        if treeid != -1 {
            if treeid > tree_current { tree_current += 1; m.tree_bodyadr[tree_current as usize] = i as i32; }
            m.tree_bodynum[tree_current as usize] += 1;
        }
    }
    util_misc::zero_int(&mut m.tree_dofnum, m.ntree);
    tree_current = -1;
    for i in 0..m.nv {
        if m.dof_treeid[i] > tree_current { tree_current += 1; m.tree_dofadr[tree_current as usize] = i as i32; }
        m.tree_dofnum[tree_current as usize] += 1;
    }
    let mut tree_marker = d.scratch.int(m.ntree);
    for i in 0..m.ntendon {
        util_misc::zero_int(&mut tree_marker, m.ntree);
        m.tendon_treenum[i] = 0;
        m.tendon_treeid[2 * i] = -1; m.tendon_treeid[2 * i + 1] = -1;
        for j in m.tendon_adr[i]..m.tendon_adr[i] + m.tendon_num[i] {
            let wrap_treeid = get_wrap_body_tree_id(m, j as usize);
            if wrap_treeid != -1 && tree_marker[wrap_treeid as usize] == 0 {
                tree_marker[wrap_treeid as usize] = 1;
                if m.tendon_treenum[i] == 0 { m.tendon_treeid[2 * i] = wrap_treeid; }
                else if m.tendon_treenum[i] == 1 { m.tendon_treeid[2 * i + 1] = wrap_treeid; }
                m.tendon_treenum[i] += 1;
            }
        }
    }
    for i in 0..m.ntree {
        let policy = m.tree_sleep_policy[i];
        if policy == SLEEP_AUTO_NEVER || policy == SLEEP_AUTO_ALLOWED { m.tree_sleep_policy[i] = SLEEP_AUTO; }
    }
    for i in 0..m.nactuator {
        let mut bodyid = -1; let mut bodyid2 = -1;
        let tid = m.actuator_trnid[2 * i]; let tid2 = m.actuator_trnid[2 * i + 1];
        match m.actuator_trntype[i] {
            TRN_JOINT | TRN_JOINTINPARENT => bodyid = m.jnt_bodyid[tid as usize],
            TRN_SO3 => {
                bodyid = if tid2 >= 0 { m.site_bodyid[tid as usize] } else { m.jnt_bodyid[tid as usize] };
                bodyid2 = if tid2 >= 0 { m.site_bodyid[tid2 as usize] } else { -1 };
            }
            TRN_SITE | TRN_SLIDERCRANK => {
                bodyid = m.site_bodyid[tid as usize];
                bodyid2 = if tid2 >= 0 { m.site_bodyid[tid2 as usize] } else { -1 };
            }
            TRN_BODY => bodyid = tid,
            TRN_TENDON => {
                for j in m.tendon_adr[tid as usize]..m.tendon_adr[tid as usize] + m.tendon_num[tid as usize] {
                    let treeid = get_wrap_body_tree_id(m, j as usize);
                    if treeid != -1 && m.tree_sleep_policy[treeid as usize] == SLEEP_AUTO { m.tree_sleep_policy[treeid as usize] = SLEEP_AUTO_NEVER; }
                }
                continue;
            }
            TRN_UNDEFINED => continue,
            _ => {}
        }
        if bodyid != -1 {
            let treeid = m.body_treeid[bodyid as usize];
            if treeid != -1 && m.tree_sleep_policy[treeid as usize] == SLEEP_AUTO { m.tree_sleep_policy[treeid as usize] = SLEEP_AUTO_NEVER; }
        }
        if bodyid2 != -1 {
            let treeid = m.body_treeid[bodyid2 as usize];
            if treeid != -1 && m.tree_sleep_policy[treeid as usize] == SLEEP_AUTO { m.tree_sleep_policy[treeid as usize] = SLEEP_AUTO_NEVER; }
        }
    }
    for i in 0..m.ntendon {
        let treenum = m.tendon_treenum[i];
        if treenum < 2 { continue; }
        if treenum == 2 && tendon_has_stiffness(m, i) == 0 && tendon_has_damping(m, i) == 0 { continue; }
        util_misc::zero_int(&mut tree_marker, m.ntree);
        for j in m.tendon_adr[i]..m.tendon_adr[i] + m.tendon_num[i] {
            let treeid = get_wrap_body_tree_id(m, j as usize);
            if treeid != -1 && tree_marker[treeid as usize] == 0 {
                tree_marker[treeid as usize] = 1;
                let policy = m.tree_sleep_policy[treeid as usize];
                if policy == SLEEP_AUTO { m.tree_sleep_policy[treeid as usize] = SLEEP_AUTO_NEVER; }
                else if policy == SLEEP_ALLOWED || policy == SLEEP_INIT {
                    d.scratch.put_int(tree_marker);
                    if treenum > 2 { error(&format!("tree {} connected to tendon {} which spans more than 2 trees, sleeping not allowed", treeid, i)); }
                    else { error(&format!("tree {} connected to tendon {} with non-zero stiffness or damping, sleeping not allowed", treeid, i)); }
                }
            }
        }
    }
    for i in 0..m.nflex {
        if m.flex_edgeequality[i] != 0 || eff_flex_possible(m, i) != 0 { continue; }
        if m.flex_interp[i] != 0 {
            let nodenum = m.flex_nodenum[i] as usize;
            let bodyid = &m.flex_nodebodyid[m.flex_nodeadr[i] as usize..];
            for j in 0..nodenum {
                let treeid = m.body_treeid[bodyid[j] as usize];
                if treeid != -1 && m.tree_sleep_policy[treeid as usize] == SLEEP_AUTO { m.tree_sleep_policy[treeid as usize] = SLEEP_AUTO_NEVER; }
            }
        } else {
            let vertnum = m.flex_vertnum[i] as usize;
            let bodyid = &m.flex_vertbodyid[m.flex_vertadr[i] as usize..];
            for j in 0..vertnum {
                let treeid = m.body_treeid[bodyid[j] as usize];
                if treeid != -1 && m.tree_sleep_policy[treeid as usize] == SLEEP_AUTO { m.tree_sleep_policy[treeid as usize] = SLEEP_AUTO_NEVER; }
            }
            if m.flex_dim[i] == 1 && m.flex_bendingadr[i] >= 0 {
                let b = &m.flex_bending[m.flex_bendingadr[i] as usize..];
                let nhinge = b[0] as i32;
                for _e in 0..=nhinge {
                    // Typed closure retains the C continuation without unreachable-code warnings.
                    let jnt = (|| -> i32 { crate::unported!("mj_rodChainTwistJnt") })();
                    if jnt >= 0 {
                        let treeid = m.dof_treeid[m.jnt_dofadr[jnt as usize] as usize];
                        if treeid != -1 && m.tree_sleep_policy[treeid as usize] == SLEEP_AUTO { m.tree_sleep_policy[treeid as usize] = SLEEP_AUTO_NEVER; }
                    }
                }
            }
        }
    }
    for i in 0..m.ntree {
        if m.tree_sleep_policy[i] == SLEEP_AUTO { m.tree_sleep_policy[i] = SLEEP_AUTO_ALLOWED; }
    }
    d.scratch.put_int(tree_marker);
}

fn make_tendon_sparse(m: &mut Model) {
    let ntendon = m.ntendon;
    let rownnz = &mut m.ten_j_rownnz;
    let rowadr = &mut m.ten_j_rowadr;
    let colind = &mut m.ten_j_colind;
    if ntendon == 0 { return; }
    util_misc::zero_int(rownnz, ntendon);
    util_misc::zero_int(rowadr, ntendon);
    for i in 0..ntendon {
        rowadr[i] = if i > 0 { rowadr[i - 1] + rownnz[i - 1] } else { 0 };
        let adr = m.tendon_adr[i] as usize;
        let num = m.tendon_num[i];
        if m.tendon_type[i] == TENDON_FIXED {
            for j in 0..num as usize { colind[rowadr[i] as usize + j] = m.jnt_dofadr[m.wrap_objid[adr + j] as usize]; }
            rownnz[i] = num;
        } else {
            let mut nnz = 0;
            for j in 0..num as usize {
                let type_ = m.wrap_type[adr + j];
                let mut bodyid = -1;
                if type_ == WRAP_SITE { bodyid = m.site_bodyid[m.wrap_objid[adr + j] as usize]; }
                else if type_ == WRAP_SPHERE || type_ == WRAP_CYLINDER { bodyid = m.geom_bodyid[m.wrap_objid[adr + j] as usize]; }
                if bodyid > 0 {
                    let mut bid = bodyid;
                    while bid > 0 {
                        let bdofadr = m.body_dofadr[bid as usize];
                        let bdofnum = m.body_dofnum[bid as usize];
                        for k in 0..bdofnum {
                            let dof = bdofadr + k;
                            let mut found = false;
                            for l in 0..nnz {
                                if colind[rowadr[i] as usize + l] == dof { found = true; break; }
                            }
                            if !found { colind[rowadr[i] as usize + nnz] = dof; nnz += 1; }
                        }
                        bid = m.body_parentid[bid as usize];
                    }
                }
            }
            rownnz[i] = nnz as i32;
        }
        let nnz = rownnz[i];
        for j in 0..nnz - 1 {
            for k in j + 1..nnz {
                let j = (rowadr[i] + j) as usize;
                let k = (rowadr[i] + k) as usize;
                if colind[k] < colind[j] { let tmp = colind[j]; colind[j] = colind[k]; colind[k] = tmp; }
            }
        }
    }
}

fn make_flex_sparse(m: &mut Model, d: &mut Data) {
    let nv = m.nv;
    if m.nflex == 0 { return; }
    let mut chain = d.scratch.int(nv);
    let mut dummy_pos = d.scratch.num(3);
    blas::zero(&mut dummy_pos, 3);
    util_misc::zero_int(&mut m.flexedge_j_rowadr, m.nflexedge);
    util_misc::zero_int(&mut m.flexedge_j_rownnz, m.nflexedge);
    util_misc::zero_int(&mut m.flexedge_j_colind, m.n_jfe);
    let mut total_nnz = 0;
    for f in 0..m.nflex {
        let ebase = m.flex_edgeadr[f] as usize;
        if m.flex_rigid[f] != 0 || m.flex_interp[f] != 0 {
            for e in 0..m.flex_edgenum[f] as usize { m.flexedge_j_rowadr[ebase + e] = total_nnz as i32; }
            continue;
        }
        let skipjacobian = m.flex_edgeequality[f] == 0 && m.flex_edgedamping[f] == 0.0 && m.flex_edgestiffness[f] == 0.0 && m.flex_damping[f] == 0.0;
        let vbase = m.flex_vertadr[f] as usize;
        for e in 0..m.flex_edgenum[f] as usize {
            m.flexedge_j_rowadr[ebase + e] = total_nnz as i32;
            if skipjacobian { continue; }
            let v1 = m.flex_edge[2 * (ebase + e)] as usize;
            let v2 = m.flex_edge[2 * (ebase + e) + 1] as usize;
            let b1 = m.flex_vertbodyid[vbase + v1] as usize;
            let b2 = m.flex_vertbodyid[vbase + v2] as usize;
            // NV in C is distinct from nv; use nv_pair.
            let nv_pair = jac_dif_pair(m, d, &mut chain, b1, b2, at3(&dummy_pos, 0), at3(&dummy_pos, 0), None, None, None, None, None, None, 1, 0);
            m.flexedge_j_rownnz[ebase + e] = nv_pair as i32;
            util_misc::copy_int(&mut m.flexedge_j_colind[total_nnz..], &chain, nv_pair);
            total_nnz += nv_pair;
        }
    }
    d.scratch.put_int(chain); d.scratch.put_num(dummy_pos);
}

fn kinematics0(m: &mut Model, d: &mut Data) {
    blas::copy(&mut d.qpos, &m.qpos0, m.nq);
    kinematics(m, d); com_pos(m, d);
    tendon(m, d); flex_skip(m, d, 0); transmission(m, d);
}

fn set_kinematics0(m: &mut Model, d: &mut Data) {
    blas::copy(&mut m.flexedge_length0, &d.flexedge_length, m.nflexedge);
    for f in 0..m.nflex {
        if m.flex_interp[f] != 0 { blas::zero(&mut m.flexedge_length0[m.flex_edgeadr[f] as usize..], m.flex_edgenum[f] as usize); }
    }
    blas::copy(&mut m.tendon_length0, &d.ten_length, m.ntendon);
    blas::copy(&mut m.actuator_length0, &d.actuator_length, m.nout);
    for i in 0..m.neq {
        if m.eq_objtype[i] != OBJ_BODY { continue; }
        let id1 = m.eq_obj1id[i] as usize; let id2 = m.eq_obj2id[i] as usize;
        let data = &mut m.eq_data[NEQDATA * i..];
        if m.eq_type[i] == EQ_CONNECT {
            let mut pos = [0.0; 3];
            local2global(d, Some(&mut pos), None, Some(at3(data, 0)), None, id1, 0);
            blas::sub_from3(&mut pos, at3(&d.xpos, 3 * id2));
            blas::mul_mat_tvec3(at3_mut(data, 3), at9(&d.xmat, 9 * id2), &pos);
        } else if m.eq_type[i] == EQ_WELD && data[11] != 0.0 {
            let mut pos = [0.0; 3]; let mut quat = [0.0; 4];
            local2global(d, Some(&mut pos), None, Some(at3(data, 0)), None, id2, 0);
            blas::sub_from3(&mut pos, at3(&d.xpos, 3 * id1));
            blas::mul_mat_tvec3(at3_mut(data, 3), at9(&d.xmat, 9 * id1), &pos);
            spatial::neg_quat(&mut quat, at4(&d.xquat, 4 * id1));
            spatial::mul_quat(at4_mut(data, 6), &quat, at4(&d.xquat, 4 * id2));
        }
    }
    for i in 0..m.ncam {
        let id = m.cam_bodyid[i] as usize; let id1 = m.cam_targetbodyid[i];
        let mut pos = [0.0; 3]; let mut mat = [0.0; 9];
        local2global(d, Some(&mut pos), Some(&mut mat), Some(at3(&m.cam_pos, 3 * i)), Some(at4(&m.cam_quat, 4 * i)), id, 0);
        blas::sub3(at3_mut(&mut m.cam_pos0, 3 * i), &pos, at3(&d.xpos, 3 * id));
        blas::sub3(at3_mut(&mut m.cam_poscom0, 3 * i), &pos, at3(&d.subtree_com, if id1 >= 0 { 3 * id1 as usize } else { 3 * id }));
        blas::copy9(at9_mut(&mut m.cam_mat0, 9 * i), &mat);
    }
    for i in 0..m.nlight {
        let id = m.light_bodyid[i] as usize; let id1 = m.light_targetbodyid[i];
        let mut pos = [0.0; 3]; let mut dir = [0.0; 3];
        local2global(d, Some(&mut pos), None, Some(at3(&m.light_pos, 3 * i)), None, id, 0);
        spatial::rot_vec_quat(&mut dir, at3(&m.light_dir, 3 * i), at4(&d.xquat, 4 * id));
        blas::normalize3(&mut dir);
        blas::sub3(at3_mut(&mut m.light_pos0, 3 * i), &pos, at3(&d.xpos, 3 * id));
        blas::sub3(at3_mut(&mut m.light_poscom0, 3 * i), &pos, at3(&d.subtree_com, if id1 >= 0 { 3 * id1 as usize } else { 3 * id }));
        blas::copy3(at3_mut(&mut m.light_dir0, 3 * i), &dir);
    }
}

fn set_inertia0(m: &mut Model, d: &mut Data) {
    let nv = m.nv;
    let mut a = [0.0; 36];
    let mut jac = d.scratch.num(6 * nv);
    let mut tmp = d.scratch.num(6 * nv);
    let mut moment = d.scratch.num(nv);
    set_m0(m, d);
    make_m(m, d); factor_m(m, d);
    m.body_invweight0[0] = 0.0; m.body_invweight0[1] = 0.0;
    for i in 1..m.nbody {
        if m.body_dofnum[m.body_weldid[i] as usize] == 0 {
            m.body_invweight0[2 * i] = 0.0; m.body_invweight0[2 * i + 1] = 0.0;
        } else if m.body_simple[i] == 2 {
            let mass = m.body_mass[i];
            if mass == 0.0 { error(&format!("moving body {} has 0 mass", i)); }
            m.body_invweight0[2 * i] = 1.0 / max(MINVAL, mass);
            m.body_invweight0[2 * i + 1] = 0.0;
        } else {
            if nv != 0 {
                let (jacp, jacr) = jac.split_at_mut(3 * nv);
                jac_body_com(m, d, Some(jacp), Some(jacr), i);
                solve_m(m, d, &mut tmp, Some(&jac), 6);
                blas::mul_mat_mat_t(&mut a, &jac, &tmp, 6, nv, 6);
            }
            m.body_invweight0[2 * i] = (a[0] + a[7] + a[14]) / 3.0;
            m.body_invweight0[2 * i + 1] = (a[21] + a[28] + a[35]) / 3.0;
        }
    }
    for i in 0..m.njnt {
        if m.body_simple[m.jnt_bodyid[i] as usize] == 2 {
            let id = m.jnt_dofadr[i] as usize;
            let bi = m.jnt_bodyid[i] as usize;
            let mass = m.body_mass[bi];
            if mass == 0.0 { error(&format!("moving body {} has 0 mass", bi)); }
            m.dof_invweight0[id] = 1.0 / max(MINVAL, mass);
        } else {
            let id = m.jnt_dofadr[i] as usize;
            let dnum = if m.jnt_type[i] == JNT_FREE { 6 } else if m.jnt_type[i] == JNT_BALL { 3 } else { 1 };
            if nv != 0 {
                blas::zero(&mut jac, dnum * nv);
                for j in 0..dnum { jac[j * (nv + 1) + id] = 1.0; }
                solve_m(m, d, &mut tmp, Some(&jac), dnum);
                blas::mul_mat_mat_t(&mut a, &jac, &tmp, dnum, nv, dnum);
            }
            if dnum == 6 {
                m.dof_invweight0[id] = (a[0] + a[7] + a[14]) / 3.0;
                m.dof_invweight0[id + 1] = m.dof_invweight0[id]; m.dof_invweight0[id + 2] = m.dof_invweight0[id];
                m.dof_invweight0[id + 3] = (a[21] + a[28] + a[35]) / 3.0;
                m.dof_invweight0[id + 4] = m.dof_invweight0[id + 3]; m.dof_invweight0[id + 5] = m.dof_invweight0[id + 3];
            } else if dnum == 3 {
                m.dof_invweight0[id] = (a[0] + a[4] + a[8]) / 3.0;
                m.dof_invweight0[id + 1] = m.dof_invweight0[id]; m.dof_invweight0[id + 2] = m.dof_invweight0[id];
            } else { m.dof_invweight0[id] = a[0]; }
        }
    }
    blas::zero(&mut m.flexedge_invweight0, m.nflexedge);
    if nv != 0 {
        for f in 0..m.nflex {
            if m.flex_interp[f] != 0 { continue; }
            for i in m.flex_edgeadr[f]..m.flex_edgeadr[f] + m.flex_edgenum[f] {
                let i = i as usize;
                let b1 = m.flex_vertbodyid[(m.flex_vertadr[f] + m.flex_edge[2 * i]) as usize] as usize;
                let b2 = m.flex_vertbodyid[(m.flex_vertadr[f] + m.flex_edge[2 * i + 1]) as usize] as usize;
                if m.flexedge_rigid[i] != 0 { m.flexedge_invweight0[i] = 0.0; }
                else if m.body_simple[b1] == 2 && m.body_simple[b2] == 2 {
                    m.flexedge_invweight0[i] = (1.0 / m.body_mass[b1] + 1.0 / m.body_mass[b2]) / 2.0;
                } else {
                    blas::zero(&mut tmp, nv);
                    let end = m.flexedge_j_rowadr[i] + m.flexedge_j_rownnz[i];
                    for j in m.flexedge_j_rowadr[i]..end { tmp[m.flexedge_j_colind[j as usize] as usize] = d.flexedge_j[j as usize]; }
                    let (input, output) = tmp.split_at_mut(nv);
                    solve_m(m, d, output, Some(input), 1);
                    m.flexedge_invweight0[i] = blas::dot(input, output, nv);
                }
            }
        }
        for i in 0..m.ntendon {
            sparse2dense(&mut tmp, &d.ten_j, 1, nv, &m.ten_j_rownnz[i..], &m.ten_j_rowadr[i..], &m.ten_j_colind);
            let (input, output) = tmp.split_at_mut(nv);
            solve_m(m, d, output, Some(input), 1);
            m.tendon_invweight0[i] = blas::dot(input, output, nv);
        }
        for i in 0..m.nout {
            sparse2dense(&mut moment, &d.actuator_moment, 1, nv, &d.moment_rownnz[i..], &d.moment_rowadr[i..], &d.moment_colind);
            solve_m(m, d, &mut tmp, Some(&moment), 1);
            m.actuator_acc0[i] = blas::norm(&tmp, nv);
        }
    } else {
        blas::zero(&mut m.tendon_invweight0, m.ntendon);
        blas::zero(&mut m.actuator_acc0, m.nout);
    }
    d.scratch.put_num(jac); d.scratch.put_num(tmp); d.scratch.put_num(moment);
}

fn update_box(xmin: &mut [f64; 3], xmax: &mut [f64; 3], pos: &[f64; 3], radius: f64) {
    for i in 0..3 { xmin[i] = min(xmin[i], pos[i] - radius); xmax[i] = max(xmax[i], pos[i] + radius); }
}

fn body_size(m: &Model, d: &Data, body: &mut [f64]) {
    blas::zero(body, m.nbody);
    for i in 0..m.njnt {
        let mut id = m.jnt_bodyid[i] as usize;
        body[id] = max(body[id], blas::dist3(at3(&d.xipos, 3 * id), at3(&d.xanchor, 3 * i)));
        id = m.body_parentid[id] as usize;
        body[id] = max(body[id], blas::dist3(at3(&d.xipos, 3 * id), at3(&d.xanchor, 3 * i)));
    }
    body[0] = 0.0;
    for i in 1..m.nbody {
        for id in m.body_geomadr[i]..m.body_geomadr[i] + m.body_geomnum[i] {
            let id = id as usize;
            if m.geom_rbound[id] > 0.0 { body[i] = max(body[i], m.geom_rbound[id] + blas::dist3(at3(&d.xipos, 3 * i), at3(&d.geom_xpos, 3 * id))); }
        }
    }
    for f in 0..m.nflex {
        if m.flex_interp[f] != 0 {
            for v1 in m.flex_nodeadr[f]..m.flex_nodeadr[f] + m.flex_nodenum[f] {
                for v2 in m.flex_nodeadr[f]..m.flex_nodeadr[f] + m.flex_nodenum[f] {
                    let edge = blas::dist3(at3(&d.xpos, 3 * m.flex_nodebodyid[v1 as usize] as usize), at3(&d.xpos, 3 * m.flex_nodebodyid[v2 as usize] as usize));
                    let b = m.flex_nodebodyid[v1 as usize] as usize;
                    body[b] = max(body[b], edge);
                }
            }
            continue;
        }
        for e in m.flex_edgeadr[f]..m.flex_edgeadr[f] + m.flex_edgenum[f] {
            let e = e as usize;
            let b1 = m.flex_vertbodyid[(m.flex_vertadr[f] + m.flex_edge[2 * e]) as usize] as usize;
            let b2 = m.flex_vertbodyid[(m.flex_vertadr[f] + m.flex_edge[2 * e + 1]) as usize] as usize;
            body[b1] = max(body[b1], m.flexedge_length0[e]); body[b2] = max(body[b2], m.flexedge_length0[e]);
        }
    }
    for i in 1..m.nbody { body[i] = max(body[i], 1e-5); }
}

fn set_dof_length(m: &mut Model, d: &mut Data) {
    let mut body = d.scratch.num(m.nbody);
    body_size(m, d, &mut body);
    for i in 0..m.nv {
        m.dof_length[i] = 1.0;
        let jnt = m.dof_jntid[i] as usize;
        let type_ = m.jnt_type[jnt];
        let offset = i as i32 - m.jnt_dofadr[jnt];
        if type_ == JNT_BALL || type_ == JNT_HINGE || (type_ == JNT_FREE && offset >= 3) { m.dof_length[i] = body[m.dof_bodyid[i] as usize]; }
    }
    d.scratch.put_num(body);
}

fn set_spring(m: &mut Model, d: &mut Data) {
    blas::copy(&mut d.qpos, &m.qpos_spring, m.nq);
    kinematics(m, d); com_pos(m, d); tendon(m, d); transmission(m, d);
}

fn set_efm0_factor(m: &mut Model, _d: &mut Data) {
    let nbd = m.nefm0dof;
    if nbd == 0 { return; }
    crate::unported!("setEfm0Factor (active metric)");
}

// Constants set once by compilation, assuming computations in qpos0.
fn set_stat(m: &mut Model, d: &mut Data) {
    let mut xmin = [1e10; 3]; let mut xmax = [-1e10; 3];
    for i in 1..m.nbody {
        update_box(&mut xmin, &mut xmax, at3(&d.xpos, 3 * i), 0.0);
        update_box(&mut xmin, &mut xmax, at3(&d.xipos, 3 * i), 0.0);
    }
    for i in 0..m.njnt { update_box(&mut xmin, &mut xmax, at3(&d.xanchor, 3 * i), 0.0); }
    for i in 0..m.nsite { update_box(&mut xmin, &mut xmax, at3(&d.site_xpos, 3 * i), 0.0); }
    for i in 0..m.ngeom {
        let mut rbound = 0.0;
        if m.geom_rbound[i] > 0.0 { rbound = m.geom_rbound[i]; }
        else if m.geom_type[i] == GEOM_PLANE {
            if m.geom_size[3 * i] != 0.0 || m.geom_size[3 * i + 1] != 0.0 { rbound = max(m.geom_size[3 * i], m.geom_size[3 * i + 1]) * 0.1; }
            else { rbound = 0.01; }
        } else if m.geom_type[i] == GEOM_HFIELD {
            let j = m.geom_dataid[i] as usize;
            rbound = max(m.hfield_size[4 * j], max(m.hfield_size[4 * j + 1], max(m.hfield_size[4 * j + 2], m.hfield_size[4 * j + 3]))) * 0.1;
        }
        update_box(&mut xmin, &mut xmax, at3(&d.geom_xpos, 3 * i), rbound);
    }
    blas::add3(&mut m.stat.center, &xmin, &xmax);
    let center = m.stat.center;
    blas::scl3(&mut m.stat.center, &center, 0.5);
    if xmax[0] > xmin[0] { m.stat.extent = max(1e-5, max(xmax[0] - xmin[0], max(xmax[1] - xmin[1], xmax[2] - xmin[2]))); }
    if m.nbody > 1 {
        let mut body = d.scratch.num(m.nbody);
        body_size(m, d, &mut body);
        m.stat.meansize = 0.0;
        for i in 1..m.nbody { m.stat.meansize += body[i] / (m.nbody - 1) as f64; }
        d.scratch.put_num(body);
    }
    m.stat.extent = max(m.stat.extent, 2.0 * m.stat.meansize);
    if m.nbody > 1 {
        m.stat.meanmass = 0.0;
        for i in 1..m.nbody { m.stat.meanmass += m.body_mass[i]; }
        m.stat.meanmass /= (m.nbody - 1) as f64;
    }
    if m.nv != 0 {
        m.stat.meaninertia = 0.0;
        for i in 0..m.nv { m.stat.meaninertia += d.m[(m.m_rowadr[i] + m.m_rownnz[i] - 1) as usize]; }
        m.stat.meaninertia /= m.nv as f64;
    }
}

fn set_damp_ratio(m: &mut Model, d: &mut Data) {
    let nv = m.nv;
    let mut tmp = d.scratch.num(nv); let mut moment = d.scratch.num(nv);
    for i in 0..m.nactuator {
        let biasprm = &m.actuator_biasprm[NBIAS * i..];
        let gainprm = &m.actuator_gainprm[NGAIN * i..];
        let is_pid = m.actuator_gaintype[i] == GAIN_PID;
        if !is_pid && gainprm[0] != -biasprm[1] { continue; }
        if biasprm[2] <= 0.0 { continue; }
        let mut mass = 0.0;
        if nv != 0 {
            let outadr = m.actuator_outadr[i] as usize;
            let outnum = m.actuator_outnum[i] as usize;
            let mut w = 0.0;
            for k in 0..outnum {
                let j = outadr + k;
                sparse2dense(&mut moment, &d.actuator_moment, 1, nv, &d.moment_rownnz[j..], &d.moment_rowadr[j..], &d.moment_colind);
                solve_m(m, d, &mut tmp, Some(&moment), 1);
                w += blas::dot(&moment, &tmp, nv);
            }
            w /= outnum as f64;
            if w > MINVAL { mass = 1.0 / w; }
        }
        let kp = if is_pid { -biasprm[1] } else { gainprm[0] };
        let damping = biasprm[2] * 2.0 * (kp * mass).sqrt();
        m.actuator_biasprm[NBIAS * i + 2] = -damping;
    }
    d.scratch.put_num(tmp); d.scratch.put_num(moment);
}

fn set_spring_length(m: &mut Model, d: &mut Data) {
    for i in 0..m.ntendon {
        if m.tendon_lengthspring[2 * i] == -1.0 && m.tendon_lengthspring[2 * i + 1] == -1.0 {
            m.tendon_lengthspring[2 * i] = d.ten_length[i]; m.tendon_lengthspring[2 * i + 1] = d.ten_length[i];
        }
    }
}

pub fn set_const_groups(m: &mut Model, d: &mut Data, groups: i32) {
    if groups & SCGROUP_FIXED != 0 {
        m.npolygonmax = 0;
        for i in 0..m.nmeshpoly { m.npolygonmax = m.npolygonmax.max(m.mesh_polyvertnum[i] as usize); }
        m.nmeshdegmax = 0;
        for i in 0..m.nmeshvert { m.nmeshdegmax = m.nmeshdegmax.max(m.mesh_polymapnum[i] as usize); }
        set_sameframe(m);
        for i in 1..m.nbody {
            if m.body_simple[i] > 0 && m.body_sameframe[i] != SAMEFRAME_BODY as u8 { error(&format!("body {} is compiled as simple but sameframe no longer holds, use body/simple='false'", i)); }
        }
        set_bound(m, d); set_fluid(m); set_fixed(m, d);
        make_tendon_sparse(m); make_flex_sparse(m, d);
    }
    let enableflags = m.opt.enableflags;
    m.opt.enableflags &= !ENBL_SLEEP;
    if groups & (SCGROUP_KIN0 | SCGROUP_INERT0 | SCGROUP_SPRING) != 0 { d.ncon = 0; clear_efc(d); }
    let mocap = m.nmocap != 0 && groups & (SCGROUP_KIN0 | SCGROUP_INERT0 | SCGROUP_SPRING) != 0;
    let mut mocap_saved = None;
    if mocap {
        let mut mocap_pos = d.scratch.num(3 * m.nmocap);
        let mut mocap_quat = d.scratch.num(4 * m.nmocap);
        blas::copy(&mut mocap_pos, &d.mocap_pos, 3 * m.nmocap);
        blas::copy(&mut mocap_quat, &d.mocap_quat, 4 * m.nmocap);
        for i in 0..m.nbody {
            let id = m.body_mocapid[i];
            if id >= 0 {
                blas::copy3(at3_mut(&mut d.mocap_pos, 3 * id as usize), at3(&m.body_pos, 3 * i));
                blas::copy4(at4_mut(&mut d.mocap_quat, 4 * id as usize), at4(&m.body_quat, 4 * i));
            }
        }
        mocap_saved = Some((mocap_pos, mocap_quat));
    }
    if groups & (SCGROUP_KIN0 | SCGROUP_INERT0) != 0 {
        kinematics0(m, d);
        if groups & SCGROUP_KIN0 != 0 { set_kinematics0(m, d); set_dof_length(m, d); }
        if groups & SCGROUP_INERT0 != 0 { set_inertia0(m, d); }
    }
    if groups & SCGROUP_SPRING != 0 { set_spring(m, d); }
    if let Some((mocap_pos, mocap_quat)) = mocap_saved {
        blas::copy(&mut d.mocap_pos, &mocap_pos, 3 * m.nmocap);
        blas::copy(&mut d.mocap_quat, &mocap_quat, 4 * m.nmocap);
        d.scratch.put_num(mocap_pos); d.scratch.put_num(mocap_quat);
    }
    if groups & SCGROUP_SPRING != 0 { set_efm0_factor(m, d); eff_chol_set_const(m, d); }
    m.opt.enableflags = enableflags;
    if groups & (SCGROUP_KIN0 | SCGROUP_INERT0 | SCGROUP_SPRING) != 0 {
        util_misc::fill_int(&mut d.tree_asleep, -(1 + MINAWAKE as i32), m.ntree);
        update_sleep(m, d);
    }
}

// C's static setConst is distinct from mj_setConst; retain the distinction with _impl.
fn set_const_impl(m: &mut Model, d: &mut Data, compiled: i32) {
    set_const_groups(m, d, SCGROUP_FIXED | SCGROUP_KIN0 | SCGROUP_INERT0);
    if compiled != 0 { set_damp_ratio(m, d); set_stat(m, d); }
    set_const_groups(m, d, SCGROUP_SPRING);
    if compiled != 0 { set_spring_length(m, d); }
}

pub fn set_const(m: &mut Model, d: &mut Data) { set_const_impl(m, d, 0); }

pub fn set_compiled_const(m: &mut Model, d: &mut Data) { set_const_impl(m, d, 1); }

// Evaluate actuator length, advance special dynamics.
fn eval_act(m: &Model, d: &mut Data, index: usize, side: i32, opt: &crate::model::LROpt) -> f64 {
    let nv = m.nv;
    let out = m.actuator_outadr[index] as usize;
    let scl = (-m.opt.timestep / max(0.01, opt.timeconst)).exp();
    // mju_scl aliases its input; each element is read before it is written.
    for i in 0..nv { d.qvel[i] = d.qvel[i] * scl; }
    crate::forward::step1(m, d);
    let mut moment = d.scratch.num(nv);
    sparse2dense(&mut moment, &d.actuator_moment, 1, nv,
        &d.moment_rownnz[out..], &d.moment_rowadr[out..], &d.moment_colind);
    let mut qfrc_applied = std::mem::take(&mut d.qfrc_applied);
    solve_m(m, d, &mut qfrc_applied, Some(&moment), 1);
    d.qfrc_applied = qfrc_applied;
    let nrm = blas::norm(&d.qfrc_applied, nv);
    blas::scl(&mut d.qfrc_applied, &moment, (2*side-1) as f64*opt.accel/max(MINVAL, nrm), nv);
    let nrm = blas::norm(&d.qfrc_applied, nv);
    if opt.maxforce > 0.0 && nrm > opt.maxforce {
        let scl = opt.maxforce/max(MINVAL, nrm);
        for i in 0..nv { d.qfrc_applied[i] = d.qfrc_applied[i] * scl; }
    }
    crate::forward::step2(m, d);
    d.scratch.put_num(moment);
    d.actuator_length[out]
}

// Set length range for specified actuator, return 1 if ok, 0 if error.
pub fn set_length_range(m: &mut Model, d: &mut Data, index: i32,
    opt: &crate::model::LROpt, error: &mut [u8], error_sz: usize) -> i32 {
    if index < 0 || index as usize >= m.nactuator { panic!("invalid actuator index"); }
    let index = index as usize;
    let out = m.actuator_outadr[index] as usize;
    let ismuscle = m.actuator_gaintype[index] == GAIN_MUSCLE || m.actuator_biastype[index] == BIAS_MUSCLE;
    let isuser = m.actuator_gaintype[index] == GAIN_USER || m.actuator_biastype[index] == BIAS_USER;
    if opt.mode == LRMODE_NONE || (opt.mode == LRMODE_MUSCLE && !ismuscle)
        || (opt.mode == LRMODE_MUSCLEUSER && !ismuscle && !isuser) { return 1; }
    if opt.useexisting != 0 && m.actuator_lengthrange[2*out] < m.actuator_lengthrange[2*out+1] { return 1; }
    let threadid = m.actuator_trnid[2*index] as usize;
    if opt.uselimit != 0 {
        if m.actuator_trntype[index] == TRN_JOINT || m.actuator_trntype[index] == TRN_JOINTINPARENT {
            if m.jnt_limited[threadid] != 0 {
                m.actuator_lengthrange[2*out] = m.jnt_range[2*threadid];
                m.actuator_lengthrange[2*out+1] = m.jnt_range[2*threadid+1];
                return 1;
            }
        }
        if m.actuator_trntype[index] == TRN_TENDON {
            if m.tendon_limited[threadid] != 0 {
                m.actuator_lengthrange[2*out] = m.tendon_range[2*threadid];
                m.actuator_lengthrange[2*out+1] = m.tendon_range[2*threadid+1];
                return 1;
            }
        }
    }
    // snprintf's bounded, NUL-terminated output. Leave bytes beyond the terminator alone.
    let mut write_error = |message: String| {
        if error_sz != 0 {
            let n = message.len().min(error_sz - 1);
            error[..n].copy_from_slice(&message.as_bytes()[..n]);
            error[n] = 0;
        }
    };
    let mut lmin = [0.0; 2];
    let mut lmax = [0.0; 2];
    for side in 0..2 {
        crate::io::reset_data(m, d);
        let mut updated = false;
        while d.time < opt.inttotal {
            let len = eval_act(m, d, index, side as i32, opt);
            if d.time == 0.0 {
                write_error(format!("Unstable lengthrange simulation in actuator {}", index));
                return 0;
            }
            if d.time > opt.inttotal-opt.interval {
                if len < lmin[side] || !updated { lmin[side] = len; }
                if len > lmax[side] || !updated { lmax[side] = len; }
                updated = true;
            }
        }
        m.actuator_lengthrange[2*out+side] = if side == 0 { lmin[side] } else { lmax[side] };
    }
    let dif = m.actuator_lengthrange[2*out+1] - m.actuator_lengthrange[2*out];
    if dif <= 0.0 {
        write_error(format!("Invalid lengthrange ({}, {}) in actuator {}",
            m.actuator_lengthrange[2*out], m.actuator_lengthrange[2*out+1], index));
        return 0;
    }
    if lmax[0]-lmin[0] > opt.tolrange*dif {
        write_error(format!("Lengthrange computation did not converge in actuator {}:\n  eval ({}, {})\n  range ({}, {})",
            index, lmin[0], lmax[0], m.actuator_lengthrange[2*out], m.actuator_lengthrange[2*out+1]));
        return 0;
    }
    if lmax[1]-lmin[1] > opt.tolrange*dif {
        write_error(format!("Lengthrange computation did not converge in actuator {}:\n  eval ({}, {})\n range ({}, {})",
            index, lmin[1], lmax[1], m.actuator_lengthrange[2*out], m.actuator_lengthrange[2*out+1]));
        return 0;
    }
    1
}
