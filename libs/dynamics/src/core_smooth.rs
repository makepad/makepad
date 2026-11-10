//! Smooth kinematics, inertia, transmission and recursive Newton–Euler dynamics.
//!
//! Translated from `src/engine/engine_core_smooth.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust; with sleep on, a mocap body is static while it holds still.

use crate::core_constraint::mul_jac_tvec;
use crate::core_util::*;
use crate::data::Data;
use crate::inline::{self, at10, at10_mut, at3, at3_mut, at4, at4_mut, at6, at6_mut, at9, at9_mut};
use crate::model::{disabled, enabled, Model};
use crate::sleep::{sleep_state, update_sleep, wake};
use crate::types::*;
use crate::util_blas as blas;
use crate::util_errmem::warning;
use crate::util_misc::{self, max, min};
use crate::util_sparse as sparse;
use crate::util_spatial as spatial;

// Forward kinematics part 1: bodies.
pub fn kinematics1(m: &Model, d: &mut Data) {
    let nbody = m.nbody;
    blas::zero3(at3_mut(&mut d.xpos, 0));
    blas::unit4(at4_mut(&mut d.xquat, 0));
    blas::zero3(at3_mut(&mut d.xipos, 0));
    blas::unit4(at4_mut(&mut d.xiquat, 0));
    blas::zero(&mut d.xmat, 9);
    blas::zero(&mut d.ximat, 9);
    d.xmat[0] = 1.0;
    d.xmat[4] = 1.0;
    d.xmat[8] = 1.0;
    d.ximat[0] = 1.0;
    d.ximat[4] = 1.0;
    d.ximat[8] = 1.0;
    let sleep_filter = enabled(m, ENBL_SLEEP);
    for i in 1..nbody {
        // A mocap body is awake while it moves and static while it holds still (its pose is checked below), and
        // so are the bodies it carries: a still one wakes nothing it touches.
        let mocap = sleep_filter && m.body_mocapid[i] >= 0;
        if sleep_filter && !mocap && m.body_treeid[i] < 0 && m.body_mocapid[m.body_rootid[i] as usize] >= 0 {
            d.body_awake[i] = d.body_awake[m.body_rootid[i] as usize];
        }
        if sleep_filter && !mocap && d.body_awake[i] == S_STATIC {
            continue;
        }
        let mut xpos = [0.0; 3];
        let mut xquat = [0.0; 4];
        let jntadr = m.body_jntadr[i] as usize;
        let jntnum = m.body_jntnum[i] as usize;
        if jntnum == 1 && m.jnt_type[jntadr] == JNT_FREE {
            let qadr = m.jnt_qposadr[jntadr] as usize;
            inline::copy3(&mut xpos, at3(&d.qpos, qadr));
            inline::copy4(&mut xquat, at4(&d.qpos, qadr + 3));
            blas::normalize4(&mut xquat);
            inline::copy3(at3_mut(&mut d.xanchor, 3 * jntadr), &xpos);
            inline::copy3(
                at3_mut(&mut d.xaxis, 3 * jntadr),
                at3(&m.jnt_axis, 3 * jntadr),
            );
        } else {
            let pid = m.body_parentid[i] as usize;
            let mut quat = [0.0; 4];
            let (bodypos, bodyquat) = if m.body_mocapid[i] >= 0 {
                let mocap = m.body_mocapid[i] as usize;
                inline::copy4(&mut quat, at4(&d.mocap_quat, 4 * mocap));
                blas::normalize4(&mut quat);
                (at3(&d.mocap_pos, 3 * mocap), &quat)
            } else {
                (at3(&m.body_pos, 3 * i), at4(&m.body_quat, 4 * i))
            };
            if pid != 0 {
                inline::mul_mat_vec3(&mut xpos, at9(&d.xmat, 9 * pid), bodypos);
                inline::add_to3(&mut xpos, at3(&d.xpos, 3 * pid));
                inline::mul_quat(&mut xquat, at4(&d.xquat, 4 * pid), bodyquat);
            } else {
                inline::copy3(&mut xpos, bodypos);
                inline::copy4(&mut xquat, bodyquat);
            }
            let mut xanchor = [0.0; 3];
            let mut xaxis = [0.0; 3];
            for j in 0..jntnum {
                let jid = jntadr + j;
                let qadr = m.jnt_qposadr[jid] as usize;
                let jtype = m.jnt_type[jid];
                inline::rot_vec_quat(&mut xaxis, at3(&m.jnt_axis, 3 * jid), &xquat);
                inline::rot_vec_quat(&mut xanchor, at3(&m.jnt_pos, 3 * jid), &xquat);
                inline::add_to3(&mut xanchor, &xpos);
                match jtype {
                    JNT_SLIDE => {
                        inline::add_to_scl3(&mut xpos, &xaxis, d.qpos[qadr] - m.qpos0[qadr])
                    }
                    JNT_BALL | JNT_HINGE => {
                        let mut qloc = [0.0; 4];
                        if jtype == JNT_BALL {
                            inline::copy4(&mut qloc, at4(&d.qpos, qadr));
                            blas::normalize4(&mut qloc);
                        } else {
                            inline::axis_angle2quat(
                                &mut qloc,
                                at3(&m.jnt_axis, 3 * jid),
                                d.qpos[qadr] - m.qpos0[qadr],
                            );
                        }
                        let old = xquat;
                        spatial::mul_quat(&mut xquat, &old, &qloc);
                        let mut vec = [0.0; 3];
                        inline::rot_vec_quat(&mut vec, at3(&m.jnt_pos, 3 * jid), &xquat);
                        inline::sub3(&mut xpos, &xanchor, &vec);
                    }
                    _ => panic!("unknown joint type {}", jtype),
                }
                inline::copy3(at3_mut(&mut d.xanchor, 3 * jid), &xanchor);
                inline::copy3(at3_mut(&mut d.xaxis, 3 * jid), &xaxis);
            }
        }
        blas::normalize4(&mut xquat);
        if mocap {
            let still = xpos == *at3(&d.xpos, 3 * i) && xquat == *at4(&d.xquat, 4 * i);
            d.body_awake[i] = if still { S_STATIC } else { S_AWAKE };
        }
        if sleep_filter && jntnum != 0 && d.body_awake[i] == S_ASLEEP {
            let pos = at3(&d.xpos, 3 * i);
            let xq = at4(&d.xquat, 4 * i);
            let matches = xpos[0] == pos[0]
                && xpos[1] == pos[1]
                && xpos[2] == pos[2]
                && xquat[0] == xq[0]
                && xquat[1] == xq[1]
                && xquat[2] == xq[2]
                && xquat[3] == xq[3];
            if matches {
                continue;
            } else {
                d.tree_awake[m.body_treeid[i] as usize] = 1;
            }
        }
        inline::copy4(at4_mut(&mut d.xquat, 4 * i), &xquat);
        inline::copy3(at3_mut(&mut d.xpos, 3 * i), &xpos);
        inline::quat2mat(at9_mut(&mut d.xmat, 9 * i), &xquat);
    }
}

// Forward kinematics part 2: body inertias, geoms and sites.
pub fn kinematics2(m: &Model, d: &mut Data) {
    let sleep_enabled = enabled(m, ENBL_SLEEP);
    let sleep_filter = sleep_enabled && (d.nbody_awake as usize) < m.nbody;
    let nbody = if sleep_filter {
        d.nbody_awake as usize
    } else {
        m.nbody
    };
    for b in 1..nbody {
        let i = if sleep_filter {
            d.body_awake_ind[b] as usize
        } else {
            b
        };
        if sleep_enabled && d.body_awake[i] == S_STATIC {
            continue;
        }
        let mut pos = *at3(&d.xipos, 3 * i);
        let mut quat = *at4(&d.xiquat, 4 * i);
        let mut mat = *at9(&d.ximat, 9 * i);
        local2global_quat(
            d,
            Some(&mut pos),
            Some(&mut quat),
            Some(&mut mat),
            Some(at3(&m.body_ipos, 3 * i)),
            Some(at4(&m.body_iquat, 4 * i)),
            i,
            m.body_sameframe[i],
        );
        inline::copy3(at3_mut(&mut d.xipos, 3 * i), &pos);
        inline::copy4(at4_mut(&mut d.xiquat, 4 * i), &quat);
        inline::copy9(at9_mut(&mut d.ximat, 9 * i), &mat);
    }
    for b in 0..nbody {
        let i = if sleep_filter {
            d.body_awake_ind[b] as usize
        } else {
            b
        };
        if sleep_enabled && d.body_awake[i] == S_STATIC {
            continue;
        }
        let start = m.body_geomadr[i];
        let end = start + m.body_geomnum[i];
        for g in start..end {
            let g = g as usize;
            let mut pos = *at3(&d.geom_xpos, 3 * g);
            let mut quat = *at4(&d.geom_xquat, 4 * g);
            let mut mat = *at9(&d.geom_xmat, 9 * g);
            local2global_quat(
                d,
                Some(&mut pos),
                Some(&mut quat),
                Some(&mut mat),
                Some(at3(&m.geom_pos, 3 * g)),
                Some(at4(&m.geom_quat, 4 * g)),
                m.geom_bodyid[g] as usize,
                m.geom_sameframe[g],
            );
            inline::copy3(at3_mut(&mut d.geom_xpos, 3 * g), &pos);
            inline::copy4(at4_mut(&mut d.geom_xquat, 4 * g), &quat);
            inline::copy9(at9_mut(&mut d.geom_xmat, 9 * g), &mat);
        }
    }
    for i in 0..m.nsite {
        let bodyid = m.site_bodyid[i] as usize;
        if sleep_enabled && d.body_awake[bodyid] != S_AWAKE {
            continue;
        }
        let mut pos = *at3(&d.site_xpos, 3 * i);
        let mut quat = *at4(&d.site_xquat, 4 * i);
        let mut mat = *at9(&d.site_xmat, 9 * i);
        local2global_quat(
            d,
            Some(&mut pos),
            Some(&mut quat),
            Some(&mut mat),
            Some(at3(&m.site_pos, 3 * i)),
            Some(at4(&m.site_quat, 4 * i)),
            bodyid,
            m.site_sameframe[i],
        );
        inline::copy3(at3_mut(&mut d.site_xpos, 3 * i), &pos);
        inline::copy4(at4_mut(&mut d.site_xquat, 4 * i), &quat);
        inline::copy9(at9_mut(&mut d.site_xmat, 9 * i), &mat);
    }
}

pub fn kinematics(m: &Model, d: &mut Data) {
    kinematics1(m, d);
    if wake(m, d) != 0 {
        update_sleep(m, d);
    }
    kinematics2(m, d);
}

// Map inertias and motion dofs to global frame centered at subtree-CoM.
pub fn com_pos(m: &Model, d: &mut Data) {
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nbody_awake as usize) < m.nbody;
    let nbody = if sleep_filter {
        d.nbody_awake as usize
    } else {
        m.nbody
    };
    let nparent = if sleep_filter {
        d.nparent_awake as usize
    } else {
        m.nbody
    };
    for b in 0..nbody {
        let i = if sleep_filter {
            d.body_awake_ind[b] as usize
        } else {
            b
        };
        inline::scl3(
            at3_mut(&mut d.subtree_com, 3 * i),
            at3(&d.xipos, 3 * i),
            m.body_mass[i],
        );
    }
    for b in (0..nparent).rev() {
        let i = if sleep_filter {
            d.parent_awake_ind[b] as usize
        } else {
            b
        };
        if i == 0 {
            continue;
        }
        let parent = m.body_parentid[i] as usize;
        if sleep_filter && d.body_awake[i] == S_ASLEEP {
            let mut child_moment = [0.0; 3];
            inline::scl3(
                &mut child_moment,
                at3(&d.subtree_com, 3 * i),
                m.body_subtreemass[i],
            );
            inline::add_to3(at3_mut(&mut d.subtree_com, 3 * parent), &child_moment);
        } else {
            let child = *at3(&d.subtree_com, 3 * i);
            inline::add_to3(at3_mut(&mut d.subtree_com, 3 * parent), &child);
        }
    }
    for b in 0..nbody {
        let i = if sleep_filter {
            d.body_awake_ind[b] as usize
        } else {
            b
        };
        if m.body_subtreemass[i] < MINVAL {
            inline::copy3(at3_mut(&mut d.subtree_com, 3 * i), at3(&d.xipos, 3 * i));
        } else {
            let old = *at3(&d.subtree_com, 3 * i);
            blas::scl3(
                at3_mut(&mut d.subtree_com, 3 * i),
                &old,
                1.0 / m.body_subtreemass[i],
            );
        }
    }
    blas::zero(&mut d.cinert, 10);
    for b in 1..nbody {
        let i = if sleep_filter {
            d.body_awake_ind[b] as usize
        } else {
            b
        };
        let mut offset = [0.0; 3];
        inline::sub3(
            &mut offset,
            at3(&d.xipos, 3 * i),
            at3(&d.subtree_com, 3 * m.body_rootid[i] as usize),
        );
        spatial::inert_com(
            at10_mut(&mut d.cinert, 10 * i),
            at3(&m.body_inertia, 3 * i),
            at9(&d.ximat, 9 * i),
            &offset,
            m.body_mass[i],
        );
    }
    for b in 1..nbody {
        let i = if sleep_filter {
            d.body_awake_ind[b] as usize
        } else {
            b
        };
        let jntnum = m.body_jntnum[i];
        if jntnum == 0 {
            continue;
        }
        let start = m.body_jntadr[i];
        let end = start + jntnum;
        for j in start..end {
            let j = j as usize;
            let da = 6 * m.jnt_dofadr[j] as usize;
            let mut offset = [0.0; 3];
            inline::sub3(
                &mut offset,
                at3(&d.subtree_com, 3 * m.body_rootid[i] as usize),
                at3(&d.xanchor, 3 * j),
            );
            let mut skip = 0;
            match m.jnt_type[j] {
                JNT_FREE | JNT_BALL => {
                    if m.jnt_type[j] == JNT_FREE {
                        blas::zero(&mut d.cdof[da..], 18);
                        d.cdof[da + 3] = 1.0;
                        d.cdof[da + 10] = 1.0;
                        d.cdof[da + 17] = 1.0;
                        skip = 18;
                    }
                    for k in 0..3 {
                        let axis = [
                            d.xmat[9 * i + k],
                            d.xmat[9 * i + k + 3],
                            d.xmat[9 * i + k + 6],
                        ];
                        spatial::dof_com(
                            at6_mut(&mut d.cdof, da + skip + 6 * k),
                            &axis,
                            Some(&offset),
                        );
                    }
                }
                JNT_SLIDE => spatial::dof_com(at6_mut(&mut d.cdof, da), at3(&d.xaxis, 3 * j), None),
                JNT_HINGE => spatial::dof_com(
                    at6_mut(&mut d.cdof, da),
                    at3(&d.xaxis, 3 * j),
                    Some(&offset),
                ),
                _ => {}
            }
        }
    }
}

pub fn camlight(m: &Model, d: &mut Data) {
    let ncam = m.ncam;
    let nlight = m.nlight;
    let sleep_filter = enabled(m, ENBL_SLEEP);
    for i in 0..ncam {
        let id = m.cam_bodyid[i] as usize;
        let id1 = m.cam_targetbodyid[i];
        if sleep_filter && d.body_awake[id] != S_AWAKE {
            if id1 < 0 || d.body_awake[id1 as usize] != S_AWAKE {
                continue;
            }
        }
        let mut pos = *at3(&d.cam_xpos, 3 * i);
        let mut mat = *at9(&d.cam_xmat, 9 * i);
        local2global(
            d,
            Some(&mut pos),
            Some(&mut mat),
            Some(at3(&m.cam_pos, 3 * i)),
            Some(at4(&m.cam_quat, 4 * i)),
            id,
            0,
        );
        inline::copy3(at3_mut(&mut d.cam_xpos, 3 * i), &pos);
        inline::copy9(at9_mut(&mut d.cam_xmat, 9 * i), &mat);
        match m.cam_mode[i] {
            CAMLIGHT_FIXED => {}
            CAMLIGHT_TRACK | CAMLIGHT_TRACKCOM => {
                inline::copy9(at9_mut(&mut d.cam_xmat, 9 * i), at9(&m.cam_mat0, 9 * i));
                if m.cam_mode[i] == CAMLIGHT_TRACK {
                    inline::add3(
                        at3_mut(&mut d.cam_xpos, 3 * i),
                        at3(&d.xpos, 3 * id),
                        at3(&m.cam_pos0, 3 * i),
                    );
                } else {
                    inline::add3(
                        at3_mut(&mut d.cam_xpos, 3 * i),
                        at3(&d.subtree_com, 3 * id),
                        at3(&m.cam_poscom0, 3 * i),
                    );
                }
            }
            CAMLIGHT_TARGETBODY | CAMLIGHT_TARGETBODYCOM => {
                if id1 >= 0 {
                    let pos = if m.cam_mode[i] == CAMLIGHT_TARGETBODY {
                        *at3(&d.xpos, 3 * id1 as usize)
                    } else {
                        *at3(&d.subtree_com, 3 * id1 as usize)
                    };
                    let mut mat_t = [0.0; 9];
                    inline::sub3(at3_mut(&mut mat_t, 6), at3(&d.cam_xpos, 3 * i), &pos);
                    blas::normalize3(at3_mut(&mut mat_t, 6));
                    mat_t[3] = 0.0;
                    mat_t[4] = 0.0;
                    mat_t[5] = 1.0;
                    let y = *at3(&mat_t, 3);
                    let z = *at3(&mat_t, 6);
                    inline::cross(at3_mut(&mut mat_t, 0), &y, &z);
                    blas::normalize3(at3_mut(&mut mat_t, 0));
                    let z = *at3(&mat_t, 6);
                    let x = *at3(&mat_t, 0);
                    inline::cross(at3_mut(&mut mat_t, 3), &z, &x);
                    blas::normalize3(at3_mut(&mut mat_t, 3));
                    blas::transpose(&mut d.cam_xmat[9 * i..], &mat_t, 3, 3);
                }
            }
            _ => {}
        }
    }
    for i in 0..nlight {
        let id = m.light_bodyid[i] as usize;
        let id1 = m.light_targetbodyid[i];
        if sleep_filter && d.body_awake[id] != S_AWAKE {
            if id1 < 0 || d.body_awake[id1 as usize] != S_AWAKE {
                continue;
            }
        }
        let mut pos = *at3(&d.light_xpos, 3 * i);
        local2global(
            d,
            Some(&mut pos),
            None,
            Some(at3(&m.light_pos, 3 * i)),
            None,
            id,
            0,
        );
        inline::copy3(at3_mut(&mut d.light_xpos, 3 * i), &pos);
        inline::rot_vec_quat(
            at3_mut(&mut d.light_xdir, 3 * i),
            at3(&m.light_dir, 3 * i),
            at4(&d.xquat, 4 * id),
        );
        match m.light_mode[i] {
            CAMLIGHT_FIXED => {}
            CAMLIGHT_TRACK | CAMLIGHT_TRACKCOM => {
                inline::copy3(at3_mut(&mut d.light_xdir, 3 * i), at3(&m.light_dir0, 3 * i));
                if m.light_mode[i] == CAMLIGHT_TRACK {
                    inline::add3(
                        at3_mut(&mut d.light_xpos, 3 * i),
                        at3(&d.xpos, 3 * id),
                        at3(&m.light_pos0, 3 * i),
                    );
                } else {
                    inline::add3(
                        at3_mut(&mut d.light_xpos, 3 * i),
                        at3(&d.subtree_com, 3 * id),
                        at3(&m.light_poscom0, 3 * i),
                    );
                }
            }
            CAMLIGHT_TARGETBODY | CAMLIGHT_TARGETBODYCOM => {
                if id1 >= 0 {
                    let lookat = if m.light_mode[i] == CAMLIGHT_TARGETBODY {
                        *at3(&d.xpos, 3 * id1 as usize)
                    } else {
                        *at3(&d.subtree_com, 3 * id1 as usize)
                    };
                    inline::sub3(
                        at3_mut(&mut d.light_xdir, 3 * i),
                        &lookat,
                        at3(&d.light_xpos, 3 * i),
                    );
                }
            }
            _ => {}
        }
        blas::normalize3(at3_mut(&mut d.light_xdir, 3 * i));
    }
}

// Leaf aabbs must be updated before this call.
pub fn update_dynamic_bvh(m: &Model, d: &mut Data, bvhadr: usize, bvhnum: usize) {
    let mut modified = d.scratch.int(bvhnum);
    util_misc::zero_int(&mut modified, bvhnum);
    for i in 0..bvhnum {
        if m.bvh_nodeid[bvhadr + i] >= 0 {
            modified[i] = 1;
        }
    }
    for i in (0..bvhnum).rev() {
        if m.bvh_nodeid[bvhadr + i] < 0 {
            let child1 = m.bvh_child[2 * (bvhadr + i)] as usize;
            let child2 = m.bvh_child[2 * (bvhadr + i) + 1] as usize;
            if modified[child1] != 0 || modified[child2] != 0 {
                let aabb1 = *at6(&d.bvh_aabb_dyn, 6 * (bvhadr - m.nbvhstatic + child1));
                let aabb2 = *at6(&d.bvh_aabb_dyn, 6 * (bvhadr - m.nbvhstatic + child2));
                let mut xmin = [0.0; 3];
                let mut xmax = [0.0; 3];
                for k in 0..3 {
                    xmin[k] = min(aabb1[k] - aabb1[k + 3], aabb2[k] - aabb2[k + 3]);
                    xmax[k] = max(aabb1[k] + aabb1[k + 3], aabb2[k] + aabb2[k + 3]);
                }
                let aabb = at6_mut(&mut d.bvh_aabb_dyn, 6 * (bvhadr - m.nbvhstatic + i));
                for k in 0..3 {
                    aabb[k] = 0.5 * (xmax[k] + xmin[k]);
                    aabb[k + 3] = 0.5 * (xmax[k] - xmin[k]);
                }
                modified[i] = 1;
            }
        }
    }
    d.scratch.put_int(modified);
}

// Compute flex-related quantities, optionally skipping rigid flexes.
pub fn flex_skip(m: &Model, d: &mut Data, skiprigid: i32) {
    let nv = m.nv;
    let rowadr = &m.flexedge_j_rowadr;
    if m.nflex == 0 { return; }
    d.flex_hessian_valid[..m.nflex].fill(0);
    for f in 0..m.nflex {
        let vstart = m.flex_vertadr[f] as usize;
        let vend = (m.flex_vertadr[f] + m.flex_vertnum[f]) as usize;
        let nstart = m.flex_nodeadr[f];
        let nend = m.flex_nodeadr[f] + m.flex_nodenum[f];
        if m.flex_interp[f] == 0 {
            for i in vstart..vend {
                if m.flex_centered[f] != 0 || (m.flex_vert[3 * i] == 0.0 && m.flex_vert[3 * i + 1] == 0.0 && m.flex_vert[3 * i + 2] == 0.0) {
                    inline::copy3(at3_mut(&mut d.flexvert_xpos, 3 * i), at3(&d.xpos, 3 * m.flex_vertbodyid[i] as usize));
                } else {
                    inline::mul_mat_vec3(at3_mut(&mut d.flexvert_xpos, 3 * i), at9(&d.xmat, 9 * m.flex_vertbodyid[i] as usize), at3(&m.flex_vert, 3 * i));
                    inline::add_to3(at3_mut(&mut d.flexvert_xpos, 3 * i), at3(&d.xpos, 3 * m.flex_vertbodyid[i] as usize));
                }
            }
        } else {
            let nodenum = nend - nstart;
            let order = if m.flex_interp[f] < 0 { -m.flex_interp[f] } else { m.flex_interp[f] };
            let cn = &m.flex_cellnum[3 * f..];
            if nodenum != (cn[0] * order + 1) * (cn[1] * order + 1) * (cn[2] * order + 1) { crate::util_errmem::error("flex_interp_order mismatch"); }
            let nodexpos = d.scratch.num(3 * nodenum as usize);
            // Typed closures keep C's continuation without unreachable-code warnings.
            (|| -> () { crate::unported!("mju_flexGatherState") })();
            (|| -> () { crate::unported!("mj_flexInterpVert") })();
            d.scratch.put_num(nodexpos);
        }
    }
    for f in 0..m.nflex {
        let dim = m.flex_dim[f] as usize;
        let box_ = [0.0; 2];
        let pad = if (|| -> i32 { crate::unported!("mj_rodBoxSize") })() != 0 { max(m.flex_radius[f], blas::norm(&box_, 2)) } else { m.flex_radius[f] };
        let elemnum = m.flex_elemnum[f] as usize;
        for e in 0..elemnum {
            let edata = &m.flex_elem[m.flex_elemdataadr[f] as usize + e * (dim + 1)..];
            let vert = &d.flexvert_xpos[3 * m.flex_vertadr[f] as usize..];
            let mut xmin = [0.0; 3]; let mut xmax = [0.0; 3];
            inline::copy3(&mut xmin, at3(vert, 3 * edata[0] as usize));
            inline::copy3(&mut xmax, at3(vert, 3 * edata[0] as usize));
            for i in 1..=dim {
                for j in 0..3 {
                    let value = vert[3 * edata[i] as usize + j];
                    xmin[j] = min(xmin[j], value); xmax[j] = max(xmax[j], value);
                }
            }
            let base = m.flex_elemadr[f] as usize + e;
            d.flexelem_aabb[6 * base] = 0.5 * (xmax[0] + xmin[0]);
            d.flexelem_aabb[6 * base + 1] = 0.5 * (xmax[1] + xmin[1]);
            d.flexelem_aabb[6 * base + 2] = 0.5 * (xmax[2] + xmin[2]);
            d.flexelem_aabb[6 * base + 3] = 0.5 * (xmax[0] - xmin[0]) + pad;
            d.flexelem_aabb[6 * base + 4] = 0.5 * (xmax[1] - xmin[1]) + pad;
            d.flexelem_aabb[6 * base + 5] = 0.5 * (xmax[2] - xmin[2]) + pad;
        }
    }
    if !disabled(m, DSBL_MIDPHASE) {
        for f in 0..m.nflex {
            if m.flex_bvhadr[f] >= 0 {
                let flex_bvhadr = m.flex_bvhadr[f] as usize;
                let flex_bvhnum = m.flex_bvhnum[f] as usize;
                for i in flex_bvhadr..flex_bvhadr + flex_bvhnum {
                    if m.bvh_nodeid[i] >= 0 {
                        inline::copy6(at6_mut(&mut d.bvh_aabb_dyn, 6 * (i - m.nbvhstatic)), at6(&d.flexelem_aabb, 6 * (m.flex_elemadr[f] + m.bvh_nodeid[i]) as usize));
                    }
                }
                update_dynamic_bvh(m, d, flex_bvhadr, flex_bvhnum);
            }
        }
    }
    let mut jac1 = d.scratch.num(3 * nv);
    let mut jac2 = d.scratch.num(3 * nv);
    let mut jacdif = d.scratch.num(3 * nv);
    let mut chain = d.scratch.int(nv);
    blas::zero(&mut d.flexedge_j, m.n_jfe);
    for f in 0..m.nflex {
        if (skiprigid != 0 && m.flex_rigid[f] != 0) || m.flex_interp[f] != 0 { continue; }
        let skipjacobian = m.flex_edgeequality[f] != 1 && m.flex_edgedamping[f] == 0.0 && m.flex_edgestiffness[f] == 0.0 && m.flex_damping[f] == 0.0;
        let vbase = m.flex_vertadr[f] as usize;
        let ebase = m.flex_edgeadr[f] as usize;
        let edgenum = m.flex_edgenum[f] as usize;
        for e in 0..edgenum {
            let v1 = m.flex_edge[2 * (ebase + e)] as usize;
            let v2 = m.flex_edge[2 * (ebase + e) + 1] as usize;
            let b1 = m.flex_vertbodyid[vbase + v1] as usize;
            let b2 = m.flex_vertbodyid[vbase + v2] as usize;
            let pos1 = at3(&d.flexvert_xpos, 3 * (vbase + v1));
            let pos2 = at3(&d.flexvert_xpos, 3 * (vbase + v2));
            let mut vec = [0.0; 3];
            inline::sub3(&mut vec, pos2, pos1);
            d.flexedge_length[ebase + e] = blas::normalize3(&mut vec);
            if skipjacobian { continue; }
            // NV in C is distinct from nv; use nv_pair.
            let nv_pair = jac_dif_pair(m, d, &mut chain, b1, b2, pos1, pos2, Some(&mut jac1), Some(&mut jac2), Some(&mut jacdif), None, None, None, 1, 0);
            if nv_pair == 0 { continue; }
            if nv_pair > m.flexedge_j_rownnz[ebase + e] as usize { crate::util_errmem::error("flexedge_J_rownnz too small"); }
            blas::mul_mat_tvec(&mut d.flexedge_j[rowadr[ebase + e] as usize..], &jacdif, &vec, 3, nv_pair);
        }
    }
    d.scratch.put_num(jac1); d.scratch.put_num(jac2); d.scratch.put_num(jacdif); d.scratch.put_int(chain);
}

// mj_flex remains deferred with the flex feature.
pub fn flex(m: &Model, _d: &mut Data) { if m.nflex != 0 { crate::unported!("mj_flex"); } }

pub fn tendon(m: &Model, d: &mut Data) {
    let nv = m.nv;
    let nten = m.ntendon;
    let rownnz = &m.ten_j_rownnz;
    let rowadr = &m.ten_j_rowadr;
    let colind = &m.ten_j_colind;
    if nten == 0 {
        return;
    }
    let mut jac1 = d.scratch.num(3 * nv);
    let mut jac2 = d.scratch.num(3 * nv);
    let mut jacdif = d.scratch.num(3 * nv);
    let mut tmp = d.scratch.num(nv);
    let mut chain = d.scratch.int(nv);
    blas::zero(&mut d.ten_length, nten);
    blas::zero(&mut d.ten_j, m.n_jten);
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.ntree_awake as usize) < m.ntree;
    let mut wrapcount = 0;
    for i in 0..nten {
        if sleep_filter && sleep_state(m, d, OBJ_TENDON, i as i32) == S_ASLEEP {
            continue;
        }
        let adr = m.tendon_adr[i] as usize;
        d.ten_wrapadr[i] = wrapcount as i32;
        d.ten_wrapnum[i] = 0;
        let tendon_num = m.tendon_num[i];
        if m.tendon_type[i] == TENDON_FIXED {
            for j in 0..tendon_num as usize {
                let k = m.wrap_objid[adr + j] as usize;
                d.ten_length[i] += m.wrap_prm[adr + j] * d.qpos[m.jnt_qposadr[k] as usize];
                let coef = 1.0;
                let dofadr = m.jnt_dofadr[k];
                sparse::combine_sparse_inc(
                    &mut d.ten_j[rowadr[i] as usize..],
                    &[coef],
                    m.nv,
                    1.0,
                    m.wrap_prm[adr + j],
                    rownnz[i] as usize,
                    1,
                    &colind[rowadr[i] as usize..],
                    &[dofadr],
                );
            }
            continue;
        }
        let mut divisor = 1.0;
        let mut j = 0;
        while j < tendon_num - 1 {
            let jidx = adr + j as usize;
            let type0 = m.wrap_type[jidx];
            let type1 = m.wrap_type[jidx + 1];
            let id0 = m.wrap_objid[jidx];
            let mut id1 = m.wrap_objid[jidx + 1];
            if type0 == WRAP_PULLEY || type1 == WRAP_PULLEY {
                if type0 == WRAP_PULLEY {
                    divisor = m.wrap_prm[jidx];
                    blas::zero3(at3_mut(&mut d.wrap_xpos, wrapcount * 3));
                    d.wrap_obj[wrapcount] = -2;
                    d.ten_wrapnum[i] += 1;
                    wrapcount += 1;
                }
                j += 1;
                continue;
            }
            let mut wlen = -1.0;
            let mut wrapid = -1;
            let mut wpnt = [0.0; 12];
            inline::copy3(at3_mut(&mut wpnt, 0), at3(&d.site_xpos, 3 * id0 as usize));
            let mut wbody = [0; 4];
            wbody[0] = m.site_bodyid[id0 as usize];
            let wraptype;
            if type1 == WRAP_SPHERE || type1 == WRAP_CYLINDER {
                wraptype = type1;
                wrapid = id1;
                // C reassigns type1 here, but never reads it again.
                id1 = m.wrap_objid[jidx + 2];
                let sideid = util_misc::round(m.wrap_prm[jidx + 1]);
                if sideid < -1 || sideid >= m.nsite as i32 {
                    panic!("invalid sideid {} in wrap_prm", sideid);
                }
                wlen = util_misc::wrap(
                    at6_mut(&mut wpnt, 3),
                    at3(&d.site_xpos, 3 * id0 as usize),
                    at3(&d.site_xpos, 3 * id1 as usize),
                    at3(&d.geom_xpos, 3 * wrapid as usize),
                    at9(&d.geom_xmat, 9 * wrapid as usize),
                    m.geom_size[3 * wrapid as usize],
                    wraptype,
                    if sideid >= 0 {
                        Some(at3(&d.site_xpos, 3 * sideid as usize))
                    } else {
                        None
                    },
                );
            } else {
                wraptype = WRAP_NONE;
            }
            if wlen < 0.0 {
                inline::copy3(at3_mut(&mut wpnt, 3), at3(&d.site_xpos, 3 * id1 as usize));
                wbody[1] = m.site_bodyid[id1 as usize];
                d.ten_length[i] += blas::dist3(at3(&wpnt, 0), at3(&wpnt, 3)) / divisor;
            } else {
                inline::copy3(at3_mut(&mut wpnt, 9), at3(&d.site_xpos, 3 * id1 as usize));
                wbody[1] = m.geom_bodyid[wrapid as usize];
                wbody[2] = m.geom_bodyid[wrapid as usize];
                wbody[3] = m.site_bodyid[id1 as usize];
                d.ten_length[i] += (blas::dist3(at3(&wpnt, 0), at3(&wpnt, 3))
                    + wlen
                    + blas::dist3(at3(&wpnt, 6), at3(&wpnt, 9)))
                    / divisor;
            }
            for k in 0..if wlen < 0.0 { 1 } else { 3 } {
                if wbody[k] != wbody[k + 1] {
                    let mut dif = [0.0; 3];
                    inline::sub3(&mut dif, at3(&wpnt, 3 * k + 3), at3(&wpnt, 3 * k));
                    blas::normalize3(&mut dif);
                    // NV and nv are distinct in C.
                    let nv_pair = jac_dif_pair(
                        m,
                        d,
                        &mut chain,
                        wbody[k] as usize,
                        wbody[k + 1] as usize,
                        at3(&wpnt, 3 * k),
                        at3(&wpnt, 3 * k + 3),
                        Some(&mut jac1),
                        Some(&mut jac2),
                        Some(&mut jacdif),
                        None,
                        None,
                        None,
                        1,
                        0,
                    );
                    if nv_pair == 0 {
                        continue;
                    }
                    blas::mul_mat_tvec(&mut tmp, &jacdif, &dif, 3, nv_pair);
                    sparse::combine_sparse_inc(
                        &mut d.ten_j[rowadr[i] as usize..],
                        &tmp,
                        nv,
                        1.0,
                        1.0 / divisor,
                        rownnz[i] as usize,
                        nv_pair,
                        &colind[rowadr[i] as usize..],
                        &chain,
                    );
                }
            }
            if wlen < 0.0 {
                inline::copy3(at3_mut(&mut d.wrap_xpos, wrapcount * 3), at3(&wpnt, 0));
            } else {
                inline::copy9(at9_mut(&mut d.wrap_xpos, wrapcount * 3), at9(&wpnt, 0));
            }
            d.wrap_obj[wrapcount] = -1;
            if wlen >= 0.0 {
                d.wrap_obj[wrapcount + 1] = wrapid;
                d.wrap_obj[wrapcount + 2] = wrapid;
            }
            d.ten_wrapnum[i] += if wlen < 0.0 { 1 } else { 3 };
            wrapcount += if wlen < 0.0 { 1 } else { 3 };
            j += if wraptype != WRAP_NONE { 2 } else { 1 };
            if j == tendon_num - 1 || m.wrap_type[adr + j as usize + 1] == WRAP_PULLEY {
                inline::copy3(
                    at3_mut(&mut d.wrap_xpos, wrapcount * 3),
                    at3(&d.site_xpos, 3 * id1 as usize),
                );
                d.wrap_obj[wrapcount] = -1;
                d.ten_wrapnum[i] += 1;
                wrapcount += 1;
            }
        }
    }
    d.scratch.put_num(jac1);
    d.scratch.put_num(jac2);
    d.scratch.put_num(jacdif);
    d.scratch.put_num(tmp);
    d.scratch.put_int(chain);
}

pub fn tendon_dot(m: &Model, d: &mut Data, id: i32, vec: &[f64]) -> f64 {
    let nv = m.nv;
    let mut res = 0.0;
    if id < 0 || id >= m.ntendon as i32 {
        return 0.0;
    }
    let id = id as usize;
    if m.tendon_type[id] == TENDON_FIXED {
        return 0.0;
    }
    let issparse = is_sparse(m) != 0;
    let mut chain = if issparse {
        d.scratch.int(nv)
    } else {
        Vec::new()
    };
    let mut jac1 = d.scratch.num(3 * nv);
    let mut jac2 = d.scratch.num(3 * nv);
    let mut jacdif = d.scratch.num(3 * nv);
    let mut tmp = d.scratch.num(nv);
    let mut divisor = 1.0;
    let mut j = 0;
    let adr = m.tendon_adr[id] as usize;
    let num = m.tendon_num[id];
    while j < num - 1 {
        let jidx = adr + j as usize;
        let type0 = m.wrap_type[jidx];
        let type1 = m.wrap_type[jidx + 1];
        let id0 = m.wrap_objid[jidx] as usize;
        let id1 = m.wrap_objid[jidx + 1] as usize;
        if type0 == WRAP_PULLEY || type1 == WRAP_PULLEY {
            if type0 == WRAP_PULLEY {
                divisor = m.wrap_prm[jidx];
            }
            j += 1;
            continue;
        }
        let mut wpnt = [0.0; 6];
        inline::copy3(at3_mut(&mut wpnt, 0), at3(&d.site_xpos, 3 * id0));
        let mut vel = [0.0; 6];
        object_velocity(m, d, OBJ_SITE, id0, &mut vel, 0);
        let mut wvel = [vel[3], vel[4], vel[5], 0.0, 0.0, 0.0];
        let mut wbody = [0; 2];
        wbody[0] = m.site_bodyid[id0];
        let wraptype;
        if type1 == WRAP_SPHERE || type1 == WRAP_CYLINDER {
            panic!("geom wrapping not supported");
        } else {
            wraptype = WRAP_NONE;
        }
        wbody[1] = m.site_bodyid[id1];
        inline::copy3(at3_mut(&mut wpnt, 3), at3(&d.site_xpos, 3 * id1));
        object_velocity(m, d, OBJ_SITE, id1, &mut vel, 0);
        inline::copy3(at3_mut(&mut wvel, 3), at3(&vel, 3));
        if wbody[0] != wbody[1] {
            let mut dpnt = [0.0; 3];
            blas::sub3(&mut dpnt, at3(&wpnt, 3), at3(&wpnt, 0));
            let norm = blas::normalize3(&mut dpnt);
            let mut dvel = [0.0; 3];
            blas::sub3(&mut dvel, at3(&wvel, 3), at3(&wvel, 0));
            let dot = blas::dot3(&dpnt, &dvel);
            blas::add_to_scl3(&mut dvel, &dpnt, -dot);
            let old = dvel;
            blas::scl3(
                &mut dvel,
                &old,
                if norm > MINVAL { 1.0 / norm } else { 0.0 },
            );
            if issparse {
                // NV and nv are distinct in C.
                let nv_pair = merge_chain(m, &mut chain, wbody[0] as usize, wbody[1] as usize, 0);
                if nv_pair != 0 {
                    jac_dot_sparse(
                        m,
                        d,
                        Some(&mut jac1),
                        None,
                        at3(&wpnt, 0),
                        wbody[0] as usize,
                        nv_pair,
                        &chain,
                    );
                    jac_dot_sparse(
                        m,
                        d,
                        Some(&mut jac2),
                        None,
                        at3(&wpnt, 3),
                        wbody[1] as usize,
                        nv_pair,
                        &chain,
                    );
                    blas::sub(&mut jacdif, &jac2, &jac1, 3 * nv_pair);
                    blas::mul_mat_tvec(&mut tmp, &jacdif, &dpnt, 3, nv_pair);
                    for k in 0..nv_pair {
                        res += (tmp[k] / divisor) * vec[chain[k] as usize];
                    }
                    jac_sparse(
                        m,
                        d,
                        Some(&mut jac1),
                        None,
                        at3(&wpnt, 0),
                        wbody[0] as usize,
                        nv_pair,
                        &chain,
                        0,
                    );
                    jac_sparse(
                        m,
                        d,
                        Some(&mut jac2),
                        None,
                        at3(&wpnt, 3),
                        wbody[1] as usize,
                        nv_pair,
                        &chain,
                        0,
                    );
                    blas::sub(&mut jacdif, &jac2, &jac1, 3 * nv_pair);
                    blas::mul_mat_tvec(&mut tmp, &jacdif, &dvel, 3, nv_pair);
                    for k in 0..nv_pair {
                        res += (tmp[k] / divisor) * vec[chain[k] as usize];
                    }
                }
            } else {
                jac_dot(
                    m,
                    d,
                    Some(&mut jac1),
                    None,
                    at3(&wpnt, 0),
                    wbody[0] as usize,
                );
                jac_dot(
                    m,
                    d,
                    Some(&mut jac2),
                    None,
                    at3(&wpnt, 3),
                    wbody[1] as usize,
                );
                blas::sub(&mut jacdif, &jac2, &jac1, 3 * nv);
                blas::mul_mat_tvec(&mut tmp, &jacdif, &dpnt, 3, nv);
                res += blas::dot(&tmp, vec, nv) / divisor;
                jac(
                    m,
                    d,
                    Some(&mut jac1),
                    None,
                    at3(&wpnt, 0),
                    wbody[0] as usize,
                );
                jac(
                    m,
                    d,
                    Some(&mut jac2),
                    None,
                    at3(&wpnt, 3),
                    wbody[1] as usize,
                );
                blas::sub(&mut jacdif, &jac2, &jac1, 3 * nv);
                blas::mul_mat_tvec(&mut tmp, &jacdif, &dvel, 3, nv);
                res += blas::dot(&tmp, vec, nv) / divisor;
            }
        }
        j += if wraptype != WRAP_NONE { 2 } else { 1 };
    }
    if issparse {
        d.scratch.put_int(chain);
    }
    d.scratch.put_num(jac1);
    d.scratch.put_num(jac2);
    d.scratch.put_num(jacdif);
    d.scratch.put_num(tmp);
    res
}

pub fn transmission(m: &Model, d: &mut Data) {
    let nv = m.nv;
    let nactuator = m.nactuator;
    if nactuator == 0 {
        return;
    }
    let mut jac = d.scratch.num(3 * nv);
    let mut jac_a = d.scratch.num(3 * nv);
    let mut jac_s = d.scratch.num(3 * nv);
    let issparse = is_sparse(m);
    let mut efc_force = None;
    let mut moment_exclude = None;
    let mut jacdifp = None;
    let mut jac1p = None;
    let mut jac2p = None;
    let mut chain = None;
    let mut jacref = None;
    let mut moment_row = None;
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nv_awake as usize) < nv;
    for i in 0..nactuator {
        let out = m.actuator_outadr[i] as usize;
        d.moment_rowadr[out] = if out == 0 {
            0
        } else {
            d.moment_rowadr[out - 1] + d.moment_rownnz[out - 1]
        };
        let adr = d.moment_rowadr[out] as usize;
        if sleep_filter && sleep_state(m, d, OBJ_ACTUATOR, i as i32) == S_ASLEEP {
            for k in 0..m.actuator_outnum[i] as usize {
                d.moment_rowadr[out + k] = if out + k == 0 {
                    0
                } else {
                    d.moment_rowadr[out + k - 1] + d.moment_rownnz[out + k - 1]
                };
                d.moment_rownnz[out + k] = 0;
            }
            continue;
        }
        let id = m.actuator_trnid[2 * i] as usize;
        let gear = &m.actuator_gear[6 * out..];
        match m.actuator_trntype[i] {
            TRN_JOINT | TRN_JOINTINPARENT => {
                if m.jnt_type[id] == JNT_SLIDE || m.jnt_type[id] == JNT_HINGE {
                    d.moment_rownnz[out] = 1;
                    d.moment_colind[adr] = m.jnt_dofadr[id];
                    d.actuator_length[out] = d.qpos[m.jnt_qposadr[id] as usize] * gear[0];
                    d.actuator_moment[adr] = gear[0];
                } else if m.jnt_type[id] == JNT_BALL {
                    let mut axis = [0.0; 3];
                    let mut quat = [0.0; 4];
                    inline::copy4(&mut quat, at4(&d.qpos, m.jnt_qposadr[id] as usize));
                    blas::normalize4(&mut quat);
                    inline::quat2vel(&mut axis, &quat, 1.0);
                    let mut gear_axis = [0.0; 3];
                    if m.actuator_trntype[i] == TRN_JOINT {
                        inline::copy3(&mut gear_axis, at3(gear, 0));
                    } else {
                        let old = quat;
                        spatial::neg_quat(&mut quat, &old);
                        inline::rot_vec_quat(&mut gear_axis, at3(gear, 0), &quat);
                    }
                    d.actuator_length[out] = blas::dot3(&axis, &gear_axis);
                    let jnt_dofadr = m.jnt_dofadr[id];
                    for j in 0..3 {
                        d.moment_colind[adr + j] = jnt_dofadr + j as i32;
                    }
                    d.moment_rownnz[out] = 3;
                    inline::copy3(at3_mut(&mut d.actuator_moment, adr), &gear_axis);
                } else {
                    d.actuator_length[out] = 0.0;
                    let mut gear_axis = [0.0; 3];
                    if m.actuator_trntype[i] == TRN_JOINT {
                        inline::copy3(&mut gear_axis, at3(gear, 3));
                    } else {
                        let mut quat = [0.0; 4];
                        inline::copy4(&mut quat, at4(&d.qpos, m.jnt_qposadr[id] as usize + 3));
                        blas::normalize4(&mut quat);
                        let old = quat;
                        spatial::neg_quat(&mut quat, &old);
                        inline::rot_vec_quat(&mut gear_axis, at3(gear, 3), &quat);
                    }
                    let jnt_dofadr = m.jnt_dofadr[id];
                    for j in 0..6 {
                        d.moment_colind[adr + j] = jnt_dofadr + j as i32;
                    }
                    d.moment_rownnz[out] = 6;
                    inline::copy3(at3_mut(&mut d.actuator_moment, adr), at3(gear, 0));
                    inline::copy3(at3_mut(&mut d.actuator_moment, adr + 3), &gear_axis);
                }
            }
            TRN_SLIDERCRANK => {
                let idslider = m.actuator_trnid[2 * i + 1] as usize;
                let rod = m.actuator_cranklength[i];
                let axis = [
                    d.site_xmat[9 * idslider + 2],
                    d.site_xmat[9 * idslider + 5],
                    d.site_xmat[9 * idslider + 8],
                ];
                let mut vec = [0.0; 3];
                blas::sub3(
                    &mut vec,
                    at3(&d.site_xpos, 3 * id),
                    at3(&d.site_xpos, 3 * idslider),
                );
                let av = blas::dot3(&vec, &axis);
                let det = av * av + rod * rod - blas::dot3(&vec, &vec);
                let sdet;
                let mut ok = true;
                if det <= 0.0 {
                    ok = false;
                    sdet = 0.0;
                    d.actuator_length[out] = av;
                } else {
                    sdet = det.sqrt();
                    d.actuator_length[out] = av - sdet;
                }
                let mut dlda = [0.0; 3];
                let mut dldv = [0.0; 3];
                if ok {
                    blas::scl3(&mut dldv, &axis, 1.0 - av / sdet);
                    blas::scl3(&mut dlda, &vec, 1.0 / sdet);
                    inline::add_to3(&mut dldv, &dlda);
                    blas::scl3(&mut dlda, &vec, 1.0 - av / sdet);
                } else {
                    inline::copy3(&mut dlda, &vec);
                    inline::copy3(&mut dldv, &axis);
                }
                let point = *at3(&d.site_xpos, 3 * idslider);
                jac_point_axis(
                    m,
                    d,
                    Some(&mut jac_s),
                    Some(&mut jac_a),
                    &point,
                    &axis,
                    m.site_bodyid[idslider] as usize,
                );
                jac_site(m, d, Some(&mut jac), None, id);
                blas::sub_from(&mut jac, &jac_s, 3 * nv);
                // C allocates another row here; reuse the owned scratch row after clearing it.
                let row = moment_row.get_or_insert_with(|| d.scratch.num(nv));
                blas::zero(row, nv);
                for j in 0..nv {
                    for k in 0..3 {
                        row[j] += dlda[k] * jac_a[k * nv + j] + dldv[k] * jac[k * nv + j];
                    }
                }
                d.actuator_length[out] *= gear[0];
                let mut nnz = 0;
                for j in 0..nv {
                    if row[j] != 0.0 {
                        d.actuator_moment[adr + nnz] = row[j] * gear[0];
                        d.moment_colind[adr + nnz] = j as i32;
                        nnz += 1;
                    }
                }
                d.moment_rownnz[out] = nnz as i32;
            }
            TRN_TENDON => {
                d.actuator_length[out] = d.ten_length[id] * gear[0];
                let ten_j_rownnz = m.ten_j_rownnz[id] as usize;
                let ten_j_rowadr = m.ten_j_rowadr[id] as usize;
                d.moment_rownnz[out] = ten_j_rownnz as i32;
                util_misc::copy_int(
                    &mut d.moment_colind[adr..],
                    &m.ten_j_colind[ten_j_rowadr..],
                    ten_j_rownnz,
                );
                blas::scl(
                    &mut d.actuator_moment[adr..],
                    &d.ten_j[ten_j_rowadr..],
                    gear[0],
                    ten_j_rownnz,
                );
            }
            TRN_SO3 => {
                if m.actuator_trnid[2 * i + 1] == -1 {
                    let mut axis = [0.0; 3];
                    let mut quat = [0.0; 4];
                    inline::copy4(&mut quat, at4(&d.qpos, m.jnt_qposadr[id] as usize));
                    blas::normalize4(&mut quat);
                    inline::quat2vel(&mut axis, &quat, 1.0);
                    for k in 0..3 {
                        let outk = out + k;
                        d.moment_rowadr[outk] = if k == 0 {
                            adr as i32
                        } else {
                            d.moment_rowadr[outk - 1] + d.moment_rownnz[outk - 1]
                        };
                        d.actuator_length[outk] = axis[k];
                        d.moment_rownnz[outk] = 1;
                        d.moment_colind[d.moment_rowadr[outk] as usize] =
                            m.jnt_dofadr[id] + k as i32;
                        d.actuator_moment[d.moment_rowadr[outk] as usize] = 1.0;
                    }
                } else {
                    let refid = m.actuator_trnid[2 * i + 1] as usize;
                    let jacref = jacref.get_or_insert_with(|| d.scratch.num(3 * nv));
                    let row = moment_row.get_or_insert_with(|| d.scratch.num(nv));
                    let mut vec = [0.0; 3];
                    inline::sub_quat(
                        &mut vec,
                        at4(&d.site_xquat, 4 * id),
                        at4(&d.site_xquat, 4 * refid),
                    );
                    jac_site(m, d, None, Some(&mut jac_s), id);
                    jac_site(m, d, None, Some(jacref), refid);
                    blas::sub_from(&mut jac_s, jacref, 3 * nv);
                    let b0 = m.body_weldid[m.site_bodyid[id] as usize] as usize;
                    let b1 = m.body_weldid[m.site_bodyid[refid] as usize] as usize;
                    let mut dofadr0 = m.body_dofadr[b0] + m.body_dofnum[b0] - 1;
                    let mut dofadr1 = m.body_dofadr[b1] + m.body_dofnum[b1] - 1;
                    let mut dofadr_common = -1;
                    if dofadr0 >= 0 && dofadr1 >= 0 {
                        while dofadr0 != dofadr1 {
                            if dofadr0 < dofadr1 {
                                dofadr1 = m.dof_parentid[dofadr1 as usize];
                            } else {
                                dofadr0 = m.dof_parentid[dofadr0 as usize];
                            }
                            if dofadr0 == -1 || dofadr1 == -1 {
                                break;
                            }
                        }
                        if dofadr0 == dofadr1 {
                            dofadr_common = dofadr0;
                        }
                    }
                    let mut da = dofadr_common;
                    while da >= 0 {
                        jac_s[da as usize] = 0.0;
                        jac_s[nv + da as usize] = 0.0;
                        jac_s[2 * nv + da as usize] = 0.0;
                        da = m.dof_parentid[da as usize];
                    }
                    for k in 0..3 {
                        let outk = out + k;
                        d.moment_rowadr[outk] = if k == 0 {
                            adr as i32
                        } else {
                            d.moment_rowadr[outk - 1] + d.moment_rownnz[outk - 1]
                        };
                        d.actuator_length[outk] = vec[k];
                        let wrench = [
                            d.site_xmat[9 * id + k],
                            d.site_xmat[9 * id + k + 3],
                            d.site_xmat[9 * id + k + 6],
                        ];
                        blas::mul_mat_tvec(row, &jac_s, &wrench, 3, nv);
                        let mut nnz = 0;
                        for j in 0..nv {
                            if row[j] != 0.0 {
                                d.actuator_moment[d.moment_rowadr[outk] as usize + nnz] = row[j];
                                d.moment_colind[d.moment_rowadr[outk] as usize + nnz] = j as i32;
                                nnz += 1;
                            }
                        }
                        d.moment_rownnz[outk] = nnz as i32;
                    }
                }
            }
            TRN_SITE => {
                jac_site(m, d, Some(&mut jac), Some(&mut jac_s), id);
                d.actuator_length[out] = 0.0;
                let row = moment_row.get_or_insert_with(|| d.scratch.num(nv));
                if m.actuator_trnid[2 * i + 1] == -1 {
                    let mut wrench = [0.0; 6];
                    inline::mul_mat_vec3(
                        at3_mut(&mut wrench, 0),
                        at9(&d.site_xmat, 9 * id),
                        at3(gear, 0),
                    );
                    inline::mul_mat_vec3(
                        at3_mut(&mut wrench, 3),
                        at9(&d.site_xmat, 9 * id),
                        at3(gear, 3),
                    );
                    blas::mul_mat_tvec(row, &jac, &wrench, 3, nv);
                    blas::mul_mat_tvec(&mut jac, &jac_s, &wrench[3..], 3, nv);
                    blas::add_to(row, &jac, nv);
                } else {
                    let refid = m.actuator_trnid[2 * i + 1] as usize;
                    let jacref = jacref.get_or_insert_with(|| d.scratch.num(3 * nv));
                    let b0 = m.body_weldid[m.site_bodyid[id] as usize] as usize;
                    let b1 = m.body_weldid[m.site_bodyid[refid] as usize] as usize;
                    let mut dofadr0 = m.body_dofadr[b0] + m.body_dofnum[b0] - 1;
                    let mut dofadr1 = m.body_dofadr[b1] + m.body_dofnum[b1] - 1;
                    let mut dofadr_common = -1;
                    if dofadr0 >= 0 && dofadr1 >= 0 {
                        while dofadr0 != dofadr1 {
                            if dofadr0 < dofadr1 {
                                dofadr1 = m.dof_parentid[dofadr1 as usize];
                            } else {
                                dofadr0 = m.dof_parentid[dofadr0 as usize];
                            }
                            if dofadr0 == -1 || dofadr1 == -1 {
                                break;
                            }
                        }
                        if dofadr0 == dofadr1 {
                            dofadr_common = dofadr0;
                        }
                    }
                    blas::zero(row, nv);
                    if util_misc::is_zero(gear, 3) == 0 {
                        let mut vec = [0.0; 3];
                        blas::sub3(
                            &mut vec,
                            at3(&d.site_xpos, 3 * id),
                            at3(&d.site_xpos, 3 * refid),
                        );
                        let old = vec;
                        blas::mul_mat_tvec3(&mut vec, at9(&d.site_xmat, 9 * refid), &old);
                        d.actuator_length[out] += blas::dot3(&vec, at3(gear, 0));
                        jac_site(m, d, Some(jacref), None, refid);
                        blas::sub_from(&mut jac, jacref, 3 * nv);
                        let mut da = dofadr_common;
                        while da >= 0 {
                            jac[da as usize] = 0.0;
                            jac[nv + da as usize] = 0.0;
                            jac[2 * nv + da as usize] = 0.0;
                            da = m.dof_parentid[da as usize];
                        }
                        let mut wrench = [0.0; 6];
                        inline::mul_mat_vec3(
                            at3_mut(&mut wrench, 0),
                            at9(&d.site_xmat, 9 * refid),
                            at3(gear, 0),
                        );
                        blas::mul_mat_tvec(row, &jac, &wrench, 3, nv);
                    }
                    if util_misc::is_zero(&gear[3..], 3) == 0 {
                        let mut vec = [0.0; 3];
                        inline::sub_quat(
                            &mut vec,
                            at4(&d.site_xquat, 4 * id),
                            at4(&d.site_xquat, 4 * refid),
                        );
                        d.actuator_length[out] += blas::dot3(&vec, at3(gear, 3));
                        jac_site(m, d, None, Some(jacref), refid);
                        blas::sub_from(&mut jac_s, jacref, 3 * nv);
                        let mut da = dofadr_common;
                        while da >= 0 {
                            jac_s[da as usize] = 0.0;
                            jac_s[nv + da as usize] = 0.0;
                            jac_s[2 * nv + da as usize] = 0.0;
                            da = m.dof_parentid[da as usize];
                        }
                        let mut wrench = [0.0; 6];
                        inline::mul_mat_vec3(
                            at3_mut(&mut wrench, 0),
                            at9(&d.site_xmat, 9 * refid),
                            at3(gear, 3),
                        );
                        blas::mul_mat_tvec(&mut jac, &jac_s, &wrench, 3, nv);
                        blas::add_to(row, &jac, nv);
                    }
                }
                let mut nnz = 0;
                for j in 0..nv {
                    if row[j] != 0.0 {
                        d.actuator_moment[adr + nnz] = row[j];
                        d.moment_colind[adr + nnz] = j as i32;
                        nnz += 1;
                    }
                }
                d.moment_rownnz[out] = nnz as i32;
            }
            TRN_BODY => {
                d.actuator_length[out] = 0.0;
                let row = moment_row.get_or_insert_with(|| d.scratch.num(nv));
                blas::zero(row, nv);
                if efc_force.is_none() {
                    efc_force = Some(d.scratch.num(d.nefc as usize));
                    moment_exclude = Some(d.scratch.num(nv));
                    jacdifp = Some(d.scratch.num(3 * nv));
                    jac1p = Some(d.scratch.num(3 * nv));
                    jac2p = Some(d.scratch.num(3 * nv));
                    chain = if issparse != 0 {
                        Some(d.scratch.int(nv))
                    } else {
                        None
                    };
                }
                let force = efc_force.as_mut().unwrap();
                let exclude = moment_exclude.as_mut().unwrap();
                let difp = jacdifp.as_mut().unwrap();
                let j1p = jac1p.as_mut().unwrap();
                let j2p = jac2p.as_mut().unwrap();
                let chain = chain.as_deref_mut().unwrap_or(&mut []);
                blas::zero(force, d.nefc as usize);
                blas::zero(exclude, nv);
                let mut counter = 0;
                for j in 0..d.ncon as usize {
                    let con = &d.contact[j];
                    let g1 = con.geom[0];
                    let g2 = con.geom[1];
                    if g1 < 0 || g2 < 0 {
                        continue;
                    }
                    let b1 = m.geom_bodyid[g1 as usize] as usize;
                    let b2 = m.geom_bodyid[g2 as usize] as usize;
                    if b1 != id && b2 != id {
                        continue;
                    }
                    if con.exclude == 0 {
                        if con.efc_address < 0 {
                            continue;
                        }
                        counter += 1;
                        if con.dim == 1 || m.opt.cone == CONE_ELLIPTIC {
                            force[con.efc_address as usize] = 1.0;
                        } else {
                            let npyramid = con.dim - 1;
                            for k in 0..2 * npyramid {
                                force[(con.efc_address + k) as usize] = 0.5 / npyramid as f64;
                            }
                        }
                    } else if con.exclude == 1 {
                        counter += 1;
                        // NV and nv are distinct in C.
                        let nv_pair = jac_dif_pair(
                            m,
                            d,
                            chain,
                            b1,
                            b2,
                            &con.pos,
                            &con.pos,
                            Some(j1p),
                            Some(j2p),
                            Some(difp),
                            None,
                            None,
                            None,
                            issparse,
                            0,
                        );
                        blas::mul_mat_mat(&mut jac, &con.frame, difp, 1, 3, nv_pair);
                        if issparse != 0 {
                            for k in 0..nv_pair {
                                exclude[chain[k] as usize] += jac[k];
                            }
                        } else {
                            blas::add_to(exclude, &jac, nv);
                        }
                    }
                }
                if counter != 0 {
                    mul_jac_tvec(m, d, row, force);
                    blas::add_to(row, exclude, nv);
                    let scale = -1.0 / counter as f64;
                    for k in 0..nv {
                        row[k] = row[k] * scale;
                    }
                }
                let mut nnz = 0;
                for j in 0..nv {
                    if row[j] != 0.0 {
                        d.actuator_moment[adr + nnz] = row[j];
                        d.moment_colind[adr + nnz] = j as i32;
                        nnz += 1;
                    }
                }
                d.moment_rownnz[out] = nnz as i32;
            }
            _ => panic!("unknown transmission type {}", m.actuator_trntype[i]),
        }
    }
    d.scratch.put_num(jac);
    d.scratch.put_num(jac_a);
    d.scratch.put_num(jac_s);
    for scratch in [
        efc_force,
        moment_exclude,
        jacdifp,
        jac1p,
        jac2p,
        jacref,
        moment_row,
    ]
    .into_iter()
    .flatten()
    {
        d.scratch.put_num(scratch);
    }
    if let Some(chain) = chain {
        d.scratch.put_int(chain);
    }
}

pub fn tendon_armature(m: &Model, d: &mut Data) {
    let nv = m.nv;
    let ntendon = m.ntendon;
    let m_rownnz = &m.m_rownnz;
    let m_rowadr = &m.m_rowadr;
    let m_colind = &m.m_colind;
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nv_awake as usize) < nv;
    for k in 0..ntendon {
        if sleep_filter && sleep_state(m, d, OBJ_TENDON, k as i32) == S_ASLEEP {
            continue;
        }
        let armature = m.tendon_armature[k] + actuator_armature(m, OBJ_TENDON, k);
        if armature == 0.0 {
            continue;
        }
        let j_rowadr = m.ten_j_rowadr[k] as usize;
        let j_rownnz = m.ten_j_rownnz[k] as usize;
        let j_colind = &m.ten_j_colind[j_rowadr..];
        let ten_j = &d.ten_j[j_rowadr..];
        for j in 0..j_rownnz {
            let ten_j_i = ten_j[j];
            if ten_j_i == 0.0 {
                continue;
            }
            let i = j_colind[j] as usize;
            if sleep_filter && sleep_state(m, d, OBJ_DOF, i as i32) == S_ASLEEP {
                continue;
            }
            let m_adr = m_rowadr[i] as usize;
            sparse::add_to_scl_sparse_inc(
                &mut d.m[m_adr..],
                ten_j,
                m_rownnz[i] as usize,
                &m_colind[m_adr..],
                j_rownnz,
                j_colind,
                armature * ten_j_i,
            );
        }
    }
}

pub fn crb(m: &Model, d: &mut Data) {
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nv_awake as usize) < m.nv;
    let nbody = if sleep_filter {
        d.nbody_awake as usize
    } else {
        m.nbody
    };
    let nparent = if sleep_filter {
        d.nparent_awake as usize
    } else {
        m.nbody
    };
    let nv = if sleep_filter {
        d.nv_awake as usize
    } else {
        m.nv
    };
    if !sleep_filter {
        blas::copy(&mut d.crb, &d.cinert, 10 * nbody);
    } else {
        blas::copy_rows(&mut d.crb, &d.cinert, &d.body_awake_ind, nbody, 10);
    }
    for b in (0..nparent).rev() {
        let i = if sleep_filter {
            d.parent_awake_ind[b] as usize
        } else {
            b
        };
        if m.body_parentid[i] > 0 {
            let child = *at10(&d.crb, 10 * i);
            blas::add_to(&mut d.crb[10 * m.body_parentid[i] as usize..], &child, 10);
        }
    }
    if !sleep_filter {
        blas::zero(&mut d.m, m.n_c);
    } else {
        sparse::zero_sparse(&mut d.m, &m.m_rownnz, &m.m_rowadr, &d.dof_awake_ind, nv);
    }
    for v in 0..nv {
        let i = if sleep_filter {
            d.dof_awake_ind[v] as usize
        } else {
            v
        };
        let adr = m.m_rowadr[i] as usize;
        if m.dof_simplenum[i] != 0 {
            d.m[adr] = m.dof_m0[i];
            continue;
        }
        let mut madr_ij = adr as i32 + m.m_rownnz[i] - 1;
        d.m[madr_ij as usize] =
            m.dof_armature[i] + actuator_armature(m, OBJ_JOINT, m.dof_jntid[i] as usize);
        let mut buf = [0.0; 6];
        spatial::mul_inert_vec(
            &mut buf,
            at10(&d.crb, 10 * m.dof_bodyid[i] as usize),
            at6(&d.cdof, 6 * i),
        );
        let mut j = i as i32;
        while j >= 0 {
            d.m[madr_ij as usize] += inline::dot6(at6(&d.cdof, 6 * j as usize), &buf);
            madr_ij -= 1;
            j = m.dof_parentid[j as usize];
        }
    }
}

pub fn make_m(m: &Model, d: &mut Data) {
    crb(m, d);
    tendon_armature(m, d);
}

pub fn factor_m(m: &Model, d: &mut Data) {
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nv_awake as usize) < m.nv;
    let index;
    let nv;
    if !sleep_filter {
        index = None;
        nv = m.nv;
        blas::copy(&mut d.q_ld, &d.m, m.n_c);
    } else {
        index = Some(d.dof_awake_ind.as_slice());
        nv = d.nv_awake as usize;
        sparse::copy_sparse(
            &mut d.q_ld,
            &d.m,
            &m.m_rownnz,
            &m.m_rowadr,
            &d.dof_awake_ind,
            nv,
        );
    }
    let clamped = factor_i(
        &mut d.q_ld,
        Some(&mut d.q_ldiag_inv),
        nv,
        &m.m_rownnz,
        &m.m_rowadr,
        &m.m_colind,
        index,
    );
    if clamped >= 0 {
        warning(d, WARN_INERTIA, clamped);
    }
}

// Sparse L'*D*L factorization. Return the first clamped dof, or -1.
pub fn factor_i(
    mat: &mut [f64],
    mut diaginv: Option<&mut [f64]>,
    nv: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    index: Option<&[i32]>,
) -> i32 {
    let mut clamped = -1;
    for j in (0..nv).rev() {
        let k = if let Some(index) = index {
            index[j] as usize
        } else {
            j
        };
        let start = rowadr[k] as usize;
        let diag = (rownnz[k] - 1) as usize;
        let end = start + diag;
        if mat[end] < MINVAL {
            mat[end] = MINVAL;
            if clamped < 0 {
                clamped = k as i32;
            }
        }
        let inv_d = 1.0 / mat[end];
        if let Some(diaginv) = diaginv.as_deref_mut() {
            diaginv[k] = inv_d;
        }
        // C's addToScl reads row k while writing an earlier, disjoint row.
        let (earlier, row) = mat.split_at_mut(start);
        let row = &mut row[..diag];
        for (offset, &i) in colind[start..end].iter().enumerate().rev() {
            let i = i as usize;
            let scl = -row[offset] * inv_d;
            let adr = rowadr[i] as usize;
            let len = rownnz[i] as usize;
            blas::add_to_scl(&mut earlier[adr..adr + len], row, scl, len);
        }
        for value in row {
            *value = *value * inv_d;
        }
    }
    clamped
}

// x is the exclusive output (C restrict); retain row, vector-offset, column order.
pub fn solve_ld(
    x: &mut [f64],
    q_ld: &[f64],
    q_ldiag_inv: &[f64],
    nv: usize,
    n: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    index: Option<&[i32]>,
) {
    for k in (0..nv).rev() {
        let i = if let Some(index) = index {
            index[k] as usize
        } else {
            k
        };
        if rownnz[i] == 1 {
            continue;
        }
        if n == 1 {
            let x_i = x[i];
            if x_i != 0.0 {
                let start = rowadr[i] as usize;
                let end = start + rownnz[i] as usize - 1;
                for adr in start..end {
                    x[colind[adr] as usize] -= q_ld[adr] * x_i;
                }
            }
        } else {
            let start = rowadr[i] as usize;
            let end = start + rownnz[i] as usize - 1;
            let mut offset = 0;
            while offset < n * nv {
                let x_i = x[i + offset];
                if x_i != 0.0 {
                    for adr in start..end {
                        x[offset + colind[adr] as usize] -= q_ld[adr] * x_i;
                    }
                }
                offset += nv;
            }
        }
    }
    for k in 0..nv {
        let i = if let Some(index) = index {
            index[k] as usize
        } else {
            k
        };
        let inv_d_i = q_ldiag_inv[i];
        if n == 1 {
            x[i] *= inv_d_i;
        } else {
            let mut offset = 0;
            while offset < n * nv {
                x[i + offset] *= inv_d_i;
                offset += nv;
            }
        }
    }
    for k in 0..nv {
        let i = if let Some(index) = index {
            index[k] as usize
        } else {
            k
        };
        if rownnz[i] == 1 {
            continue;
        }
        let d = rownnz[i] - 1;
        if d > 0 {
            let adr = rowadr[i] as usize;
            if n == 1 {
                x[i] -= sparse::dot_sparse(&q_ld[adr..], x, d as usize, &colind[adr..]);
            } else {
                let mut offset = 0;
                while offset < n * nv {
                    x[i + offset] -=
                        sparse::dot_sparse(&q_ld[adr..], &x[offset..], d as usize, &colind[adr..]);
                    offset += nv;
                }
            }
        }
    }
}

// None for y means C's in-place x == y case.
pub fn solve_m(m: &Model, d: &mut Data, x: &mut [f64], y: Option<&[f64]>, n: usize) {
    if let Some(y) = y {
        blas::copy(x, y, n * m.nv);
    }
    solve_ld(
        x,
        &d.q_ld,
        &d.q_ldiag_inv,
        m.nv,
        n,
        &m.m_rownnz,
        &m.m_rowadr,
        &m.m_colind,
        None,
    );
}

pub fn solve_m2(m: &Model, d: &mut Data, x: &mut [f64], y: &[f64], sqrt_inv_d: &[f64], n: usize) {
    solve_m2_impl(
        x,
        y,
        sqrt_inv_d,
        &d.q_ld,
        m.nv,
        n,
        &m.m_rownnz,
        &m.m_rowadr,
        &m.m_colind,
        &m.dof_simplenum,
    );
}

pub fn solve_m2_impl(
    x: &mut [f64],
    y: &[f64],
    sqrt_inv_d: &[f64],
    factor: &[f64],
    nv: usize,
    n: usize,
    rownnz: &[i32],
    rowadr: &[i32],
    colind: &[i32],
    diagnum: &[i32],
) {
    let q_ld = factor;
    blas::copy(x, y, n * nv);
    for i in (1..nv).rev() {
        if diagnum[i] != 0 {
            continue;
        }
        let start = rowadr[i] as usize;
        let end = start + rownnz[i] as usize - 1;
        let mut offset = 0;
        while offset < n * nv {
            let x_i = x[i + offset];
            if x_i != 0.0 {
                for adr in start..end {
                    x[offset + colind[adr] as usize] -= q_ld[adr] * x_i;
                }
            }
            offset += nv;
        }
    }
    for i in 0..nv {
        let inv_d_i = sqrt_inv_d[i];
        let mut offset = 0;
        while offset < n * nv {
            x[i + offset] *= inv_d_i;
            offset += nv;
        }
    }
}

pub fn com_vel(m: &Model, d: &mut Data) {
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nbody_awake as usize) < m.nbody;
    let nbody = if sleep_filter {
        d.nbody_awake as usize
    } else {
        m.nbody
    };
    blas::zero(&mut d.cvel, 6);
    for b in 1..nbody {
        let i = if sleep_filter {
            d.body_awake_ind[b] as usize
        } else {
            b
        };
        let mut cvel = [0.0; 6];
        inline::copy6(&mut cvel, at6(&d.cvel, 6 * m.body_parentid[i] as usize));
        let dofnum = m.body_dofnum[i] as usize;
        // An empty C span need not have a valid address; no elements are accessed.
        let bda = if dofnum == 0 {
            0
        } else {
            m.body_dofadr[i] as usize
        };
        let mut cdofdot = [0.0; 36];
        let mut j = 0;
        while j < dofnum {
            let mut tmp = [0.0; 6];
            match m.jnt_type[m.dof_jntid[bda + j] as usize] {
                JNT_FREE | JNT_BALL => {
                    if m.jnt_type[m.dof_jntid[bda + j] as usize] == JNT_FREE {
                        blas::zero(&mut cdofdot, 18);
                        spatial::mul_dof_vec(&mut tmp, &d.cdof[6 * bda..], &d.qvel[bda..], 3);
                        blas::add_to(&mut cvel, &tmp, 6);
                        j += 3;
                    }
                    inline::cross_motion(
                        at6_mut(&mut cdofdot, 6 * j),
                        &cvel,
                        at6(&d.cdof, 6 * (bda + j)),
                    );
                    inline::cross_motion(
                        at6_mut(&mut cdofdot, 6 * (j + 1)),
                        &cvel,
                        at6(&d.cdof, 6 * (bda + j + 1)),
                    );
                    inline::cross_motion(
                        at6_mut(&mut cdofdot, 6 * (j + 2)),
                        &cvel,
                        at6(&d.cdof, 6 * (bda + j + 2)),
                    );
                    spatial::mul_dof_vec(&mut tmp, &d.cdof[6 * (bda + j)..], &d.qvel[bda + j..], 3);
                    blas::add_to(&mut cvel, &tmp, 6);
                    j += 2;
                }
                _ => {
                    inline::cross_motion(
                        at6_mut(&mut cdofdot, 6 * j),
                        &cvel,
                        at6(&d.cdof, 6 * (bda + j)),
                    );
                    spatial::mul_dof_vec(&mut tmp, &d.cdof[6 * (bda + j)..], &d.qvel[bda + j..], 1);
                    blas::add_to(&mut cvel, &tmp, 6);
                }
            }
            j += 1;
        }
        inline::copy6(at6_mut(&mut d.cvel, 6 * i), &cvel);
        blas::copy(&mut d.cdof_dot[6 * bda..], &cdofdot, 6 * dofnum);
    }
}

pub fn subtree_vel(m: &Model, d: &mut Data) {
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nbody_awake as usize) < m.nbody;
    let nbody = if sleep_filter {
        d.nbody_awake as usize
    } else {
        m.nbody
    };
    let nparent = if sleep_filter {
        d.nparent_awake as usize
    } else {
        m.nbody
    };
    let mut body_vel = d.scratch.num(6 * m.nbody);
    for b in 0..nbody {
        let i = if sleep_filter {
            d.body_awake_ind[b] as usize
        } else {
            b
        };
        object_velocity(m, d, OBJ_BODY, i, at6_mut(&mut body_vel, 6 * i), 0);
        blas::scl3(
            at3_mut(&mut d.subtree_linvel, 3 * i),
            at3(&body_vel, 6 * i + 3),
            m.body_mass[i],
        );
        let mut dv = [0.0; 3];
        blas::mul_mat_tvec3(&mut dv, at9(&d.ximat, 9 * i), at3(&body_vel, 6 * i));
        dv[0] *= m.body_inertia[3 * i];
        dv[1] *= m.body_inertia[3 * i + 1];
        dv[2] *= m.body_inertia[3 * i + 2];
        inline::mul_mat_vec3(
            at3_mut(&mut d.subtree_angmom, 3 * i),
            at9(&d.ximat, 9 * i),
            &dv,
        );
    }
    for b in (0..nbody).rev() {
        let i = if sleep_filter {
            d.body_awake_ind[b] as usize
        } else {
            b
        };
        if i != 0 {
            let child = *at3(&d.subtree_linvel, 3 * i);
            inline::add_to3(
                at3_mut(&mut d.subtree_linvel, 3 * m.body_parentid[i] as usize),
                &child,
            );
        }
        let old = *at3(&d.subtree_linvel, 3 * i);
        blas::scl3(
            at3_mut(&mut d.subtree_linvel, 3 * i),
            &old,
            1.0 / max(MINVAL, m.body_subtreemass[i]),
        );
    }
    for b in (0..nparent).rev() {
        let i = if sleep_filter {
            d.parent_awake_ind[b] as usize
        } else {
            b
        };
        if i == 0 {
            continue;
        }
        let parent = m.body_parentid[i] as usize;
        let mut dx = [0.0; 3];
        let mut dv = [0.0; 3];
        let mut dp = [0.0; 3];
        let mut d_l = [0.0; 3];
        if sleep_filter && d.body_awake[i] == S_ASLEEP {
            blas::zero3(at3_mut(&mut d.subtree_linvel, 3 * i));
            blas::zero3(at3_mut(&mut d.subtree_angmom, 3 * i));
        } else {
            blas::sub3(&mut dx, at3(&d.xipos, 3 * i), at3(&d.subtree_com, 3 * i));
            blas::sub3(
                &mut dv,
                at3(&body_vel, 6 * i + 3),
                at3(&d.subtree_linvel, 3 * i),
            );
            blas::scl3(&mut dp, &dv, m.body_mass[i]);
            inline::cross(&mut d_l, &dx, &dp);
            inline::add_to3(at3_mut(&mut d.subtree_angmom, 3 * i), &d_l);
            let child = *at3(&d.subtree_angmom, 3 * i);
            inline::add_to3(at3_mut(&mut d.subtree_angmom, 3 * parent), &child);
        }
        blas::sub3(
            &mut dx,
            at3(&d.subtree_com, 3 * i),
            at3(&d.subtree_com, 3 * parent),
        );
        blas::sub3(
            &mut dv,
            at3(&d.subtree_linvel, 3 * i),
            at3(&d.subtree_linvel, 3 * parent),
        );
        let old = dv;
        blas::scl3(&mut dv, &old, m.body_subtreemass[i]);
        inline::cross(&mut d_l, &dx, &dv);
        inline::add_to3(at3_mut(&mut d.subtree_angmom, 3 * parent), &d_l);
    }
    d.scratch.put_num(body_vel);
    d.flg_subtreevel = 1;
}

/// Compute M(qpos)*qacc + C(qpos,qvel); flg_acc=0 removes the inertial term.
/// None selects d.qfrc_bias, preserving C's output-identity sleep filtering.
pub fn rne(m: &Model, d: &mut Data, flg_acc: i32, result: Option<&mut [f64]>) {
    let sleep_filter =
        result.is_none() && enabled(m, ENBL_SLEEP) && (d.nbody_awake as usize) < m.nbody;
    let nbody = if sleep_filter {
        d.nbody_awake as usize
    } else {
        m.nbody
    };
    let nv = if sleep_filter {
        d.nv_awake as usize
    } else {
        m.nv
    };
    let mut loc_cacc = d.scratch.num(m.nbody * 6);
    let mut loc_cfrc_body = d.scratch.num(m.nbody * 6);
    blas::zero(&mut loc_cacc, 6);
    if !disabled(m, DSBL_GRAVITY) {
        blas::scl3(at3_mut(&mut loc_cacc, 3), &m.opt.gravity, -1.0);
    }
    for b in 1..nbody {
        let i = if sleep_filter {
            d.body_awake_ind[b] as usize
        } else {
            b
        };
        let dofnum = m.body_dofnum[i] as usize;
        let bda = if dofnum == 0 {
            0
        } else {
            m.body_dofadr[i] as usize
        };
        let mut tmp = [0.0; 6];
        spatial::mul_dof_vec(&mut tmp, &d.cdof_dot[6 * bda..], &d.qvel[bda..], dofnum);
        let parent = *at6(&loc_cacc, 6 * m.body_parentid[i] as usize);
        blas::add(&mut loc_cacc[6 * i..], &parent, &tmp, 6);
        if flg_acc != 0 {
            spatial::mul_dof_vec(&mut tmp, &d.cdof[6 * bda..], &d.qacc[bda..], dofnum);
            blas::add_to(&mut loc_cacc[6 * i..], &tmp, 6);
        }
        spatial::mul_inert_vec(
            at6_mut(&mut loc_cfrc_body, 6 * i),
            at10(&d.cinert, 10 * i),
            at6(&loc_cacc, 6 * i),
        );
        spatial::mul_inert_vec(&mut tmp, at10(&d.cinert, 10 * i), at6(&d.cvel, 6 * i));
        let mut tmp1 = [0.0; 6];
        inline::cross_force(&mut tmp1, at6(&d.cvel, 6 * i), &tmp);
        blas::add_to(&mut loc_cfrc_body[6 * i..], &tmp1, 6);
    }
    blas::zero(&mut loc_cfrc_body, 6);
    for b in (1..nbody).rev() {
        let i = if sleep_filter {
            d.body_awake_ind[b] as usize
        } else {
            b
        };
        let j = m.body_parentid[i] as usize;
        if j != 0 {
            let child = *at6(&loc_cfrc_body, 6 * i);
            blas::add_to(&mut loc_cfrc_body[6 * j..], &child, 6);
        }
    }
    let result = result.unwrap_or(&mut d.qfrc_bias);
    for v in 0..nv {
        let i = if sleep_filter {
            d.dof_awake_ind[v] as usize
        } else {
            v
        };
        result[i] = inline::dot6(
            at6(&d.cdof, 6 * i),
            at6(&loc_cfrc_body, 6 * m.dof_bodyid[i] as usize),
        );
    }
    d.scratch.put_num(loc_cacc);
    d.scratch.put_num(loc_cfrc_body);
}

fn add_external(
    m: &Model,
    d: &mut Data,
    body: usize,
    point: &[f64; 3],
    cfrc: &[f64; 6],
    scale: f64,
) {
    if body == 0 {
        return;
    }
    let mut cfrc_com = [0.0; 6];
    spatial::transform_spatial(
        &mut cfrc_com,
        cfrc,
        1,
        at3(&d.subtree_com, 3 * m.body_rootid[body] as usize),
        point,
        None,
    );
    blas::add_to_scl(&mut d.cfrc_ext[6 * body..], &cfrc_com, scale, 6);
}

fn add_tendon(m: &Model, d: &mut Data, t: usize, frc: f64) {
    let adr = m.tendon_adr[t] as usize;
    let num = m.tendon_num[t] as usize;
    let mut p = d.ten_wrapadr[t] as usize;
    let pend = p + d.ten_wrapnum[t] as usize;
    let mut divisor = 1.0;
    let mut prevbody = -1;
    let mut prevpnt = [0.0; 3];
    for j in adr..adr + num {
        let r#type = m.wrap_type[j];
        let objid = m.wrap_objid[j];
        if r#type == WRAP_PULLEY {
            divisor = m.wrap_prm[j];
            prevbody = -1;
            p += 1;
            continue;
        }
        let npnt;
        let body;
        if r#type == WRAP_SITE {
            npnt = 1;
            body = m.site_bodyid[objid as usize];
        } else {
            npnt = if p < pend && d.wrap_obj[p] == objid {
                2
            } else {
                0
            };
            body = m.geom_bodyid[objid as usize];
        }
        for _k in 0..npnt {
            let pnt = *at3(&d.wrap_xpos, 3 * p);
            if prevbody >= 0 && body != prevbody {
                let mut cfrc = [0.0; 6];
                inline::sub3(at3_mut(&mut cfrc, 3), &pnt, &prevpnt);
                if blas::normalize3(at3_mut(&mut cfrc, 3)) >= MINVAL {
                    let old = *at3(&cfrc, 3);
                    blas::scl3(at3_mut(&mut cfrc, 3), &old, frc / divisor);
                    add_external(m, d, body as usize, &pnt, &cfrc, 1.0);
                    add_external(m, d, prevbody as usize, &prevpnt, &cfrc, -1.0);
                }
            }
            prevbody = body;
            prevpnt = pnt;
            p += 1;
        }
    }
}

pub fn rne_post_constraint(m: &Model, d: &mut Data) {
    let nbody = m.nbody;
    let ntendon = m.ntendon;
    let nactuator = m.nactuator;
    let mut cfrc = [0.0; 6];
    let mut lfrc = [0.0; 6];
    let mut ten_frc = Vec::new();
    let mut ten_nlimit = Vec::new();
    blas::zero(&mut d.cacc, 6);
    if !disabled(m, DSBL_GRAVITY) {
        blas::scl3(at3_mut(&mut d.cacc, 3), &m.opt.gravity, -1.0);
    }
    if ntendon != 0 {
        ten_frc = d.scratch.num(ntendon);
        ten_nlimit = d.scratch.int(ntendon);
        blas::zero(&mut ten_frc, ntendon);
        util_misc::zero_int(&mut ten_nlimit, ntendon);
    }
    blas::zero(&mut d.cfrc_ext, 6 * nbody);
    for i in 1..nbody {
        if util_misc::is_zero(&d.xfrc_applied[6 * i..], 6) == 0 {
            inline::copy3(at3_mut(&mut cfrc, 0), at3(&d.xfrc_applied, 6 * i + 3));
            inline::copy3(at3_mut(&mut cfrc, 3), at3(&d.xfrc_applied, 6 * i));
            let point = *at3(&d.xipos, 3 * i);
            add_external(m, d, i, &point, &cfrc, 1.0);
        }
    }
    for i in 0..d.ncon as usize {
        let con = d.contact[i];
        if con.efc_address < 0 {
            continue;
        }
        if con.geom[0] < 0 || con.geom[1] < 0 {
            continue;
        }
        contact_force(m, d, i as i32, &mut lfrc);
        blas::mul_mat_tvec3(at3_mut(&mut cfrc, 0), &con.frame, at3(&lfrc, 3));
        blas::mul_mat_tvec3(at3_mut(&mut cfrc, 3), &con.frame, at3(&lfrc, 0));
        add_external(
            m,
            d,
            m.geom_bodyid[con.geom[0] as usize] as usize,
            &con.pos,
            &cfrc,
            -1.0,
        );
        add_external(
            m,
            d,
            m.geom_bodyid[con.geom[1] as usize] as usize,
            &con.pos,
            &cfrc,
            1.0,
        );
    }
    let mut i = 0;
    let ne = d.ne as usize;
    while i < ne {
        if d.efc_type[i] != CNSTR_EQUALITY {
            panic!("row {} of efc is not an equality constraint", i);
        }
        let id = d.efc_id[i] as usize;
        let eq_data = &m.eq_data[NEQDATA * id..];
        let mut pos = [0.0; 3];
        match m.eq_type[id] {
            EQ_CONNECT | EQ_WELD => {
                let body_semantic = m.eq_objtype[id] == OBJ_BODY;
                let obj1 = m.eq_obj1id[id] as usize;
                let obj2 = m.eq_obj2id[id] as usize;
                inline::copy3(at3_mut(&mut cfrc, 3), at3(&d.efc_force, i));
                if m.eq_type[id] == EQ_CONNECT {
                    blas::zero3(at3_mut(&mut cfrc, 0));
                } else {
                    let mut q0 = [0.0; 4];
                    let mut q1 = [0.0; 4];
                    let mut quat = [0.0; 4];
                    if body_semantic {
                        spatial::mul_quat(&mut q0, at4(&d.xquat, 4 * obj1), at4(eq_data, 6));
                        inline::copy4(&mut q1, at4(&d.xquat, 4 * obj2));
                    } else {
                        spatial::mul_quat(
                            &mut q0,
                            at4(&d.xquat, 4 * m.site_bodyid[obj1] as usize),
                            at4(&m.site_quat, 4 * obj1),
                        );
                        spatial::mul_quat(
                            &mut q1,
                            at4(&d.xquat, 4 * m.site_bodyid[obj2] as usize),
                            at4(&m.site_quat, 4 * obj2),
                        );
                    }
                    let old = q0;
                    spatial::neg_quat(&mut q0, &old);
                    spatial::mul_quat_axis(&mut quat, &q1, at3(&d.efc_force, i + 3));
                    let old = quat;
                    spatial::mul_quat(&mut quat, &old, &q0);
                    blas::scl3(at3_mut(&mut cfrc, 0), at3(&quat, 1), 0.5 * eq_data[10]);
                }
                let k = if body_semantic {
                    obj1
                } else {
                    m.site_bodyid[obj1] as usize
                };
                if k != 0 {
                    let offset = if body_semantic {
                        at3(eq_data, 3 * usize::from(m.eq_type[id] == EQ_WELD))
                    } else {
                        at3(&m.site_pos, 3 * obj1)
                    };
                    local2global(d, Some(&mut pos), None, Some(offset), None, k, 0);
                    add_external(m, d, k, &pos, &cfrc, 1.0);
                }
                let k = if body_semantic {
                    obj2
                } else {
                    m.site_bodyid[obj2] as usize
                };
                if k != 0 {
                    let offset = if body_semantic {
                        at3(eq_data, 3 * usize::from(m.eq_type[id] == EQ_CONNECT))
                    } else {
                        at3(&m.site_pos, 3 * obj2)
                    };
                    local2global(d, Some(&mut pos), None, Some(offset), None, k, 0);
                    add_external(m, d, k, &pos, &cfrc, -1.0);
                }
                i += if m.eq_type[id] == EQ_WELD { 6 } else { 3 };
            }
            EQ_JOINT => {
                i += 1;
            }
            EQ_TENDON => {
                let obj1 = m.eq_obj1id[id] as usize;
                let obj2 = m.eq_obj2id[id];
                ten_frc[obj1] += d.efc_force[i];
                if obj2 >= 0 {
                    let obj2 = obj2 as usize;
                    let dif = d.ten_length[obj2] - m.tendon_length0[obj2];
                    let deriv = eq_data[1]
                        + 2.0 * eq_data[2] * dif
                        + 3.0 * eq_data[3] * dif * dif
                        + 4.0 * eq_data[4] * dif * dif * dif;
                    ten_frc[obj2] -= deriv * d.efc_force[i];
                }
                i += 1;
            }
            EQ_FLEX => {
                let k = m.eq_obj1id[id] as usize;
                let flex_edgeadr = m.flex_edgeadr[k];
                let flex_edgenum = m.flex_edgenum[k];
                for e in flex_edgeadr..flex_edgeadr + flex_edgenum {
                    if m.flexedge_rigid[e as usize] == 0 {
                        i += 1;
                    }
                }
            }
            EQ_FLEXSTRAIN => {
                while i < ne && d.efc_id[i] == id as i32 {
                    i += 1;
                }
            }
            _ => panic!("unknown constraint type type {}", m.eq_type[id]),
        }
    }
    if ntendon != 0 {
        let nf = d.nf as usize;
        let nl = d.nl as usize;
        for r in ne..ne + nf + nl {
            let t = d.efc_id[r] as usize;
            if d.efc_type[r] == CNSTR_FRICTION_TENDON {
                ten_frc[t] += d.efc_force[r];
            } else if d.efc_type[r] == CNSTR_LIMIT_TENDON {
                let old = ten_nlimit[t];
                ten_nlimit[t] += 1;
                let lower =
                    old == 0 && -(m.tendon_range[2 * t] - d.ten_length[t]) < m.tendon_margin[t];
                ten_frc[t] += if lower {
                    d.efc_force[r]
                } else {
                    -d.efc_force[r]
                };
            }
        }
        for j in 0..nactuator {
            if m.actuator_trntype[j] == TRN_TENDON {
                let out = m.actuator_outadr[j] as usize;
                ten_frc[m.actuator_trnid[2 * j] as usize] +=
                    d.actuator_force[out] * m.actuator_gear[6 * out];
            }
        }
        let sleep_filter = enabled(m, ENBL_SLEEP) && (d.ntree_awake as usize) < m.ntree;
        for t in 0..ntendon {
            if m.tendon_type[t] == TENDON_FIXED {
                continue;
            }
            if sleep_filter && sleep_state(m, d, OBJ_TENDON, t as i32) == S_ASLEEP {
                continue;
            }
            let mut frc_spring = 0.0;
            let mut frc_damper = 0.0;
            tendon_spring_damper(m, d, t, &mut frc_spring, &mut frc_damper);
            ten_frc[t] += frc_spring + frc_damper;
            let armature = m.tendon_armature[t] + actuator_armature(m, OBJ_TENDON, t);
            if armature != 0.0 {
                // Snapshot the input to permit tendon_dot's mutable scratch access.
                let mut qvel = d.scratch.num(m.nv);
                blas::copy(&mut qvel, &d.qvel, m.nv);
                let mut acc = tendon_dot(m, d, t as i32, &qvel);
                d.scratch.put_num(qvel);
                let end = m.ten_j_rowadr[t] + m.ten_j_rownnz[t];
                for j in m.ten_j_rowadr[t]..end {
                    acc += d.ten_j[j as usize] * d.qacc[m.ten_j_colind[j as usize] as usize];
                }
                ten_frc[t] -= armature * acc;
            }
            if ten_frc[t] != 0.0 {
                add_tendon(m, d, t, ten_frc[t]);
            }
        }
        d.scratch.put_num(ten_frc);
        d.scratch.put_int(ten_nlimit);
    }
    let mut cacc = [0.0; 6];
    let mut cfrc_body = [0.0; 6];
    let mut cfrc_corr = [0.0; 6];
    blas::zero(&mut d.cfrc_int, 6);
    for j in 1..nbody {
        let dofnum = m.body_dofnum[j] as usize;
        let bda = if dofnum == 0 {
            0
        } else {
            m.body_dofadr[j] as usize
        };
        spatial::mul_dof_vec(&mut cacc, &d.cdof_dot[6 * bda..], &d.qvel[bda..], dofnum);
        let parent = *at6(&d.cacc, 6 * m.body_parentid[j] as usize);
        blas::add(&mut d.cacc[6 * j..], &parent, &cacc, 6);
        spatial::mul_dof_vec(&mut cacc, &d.cdof[6 * bda..], &d.qacc[bda..], dofnum);
        blas::add_to(&mut d.cacc[6 * j..], &cacc, 6);
        spatial::mul_inert_vec(&mut cfrc_body, at10(&d.cinert, 10 * j), at6(&d.cacc, 6 * j));
        spatial::mul_inert_vec(&mut cfrc_corr, at10(&d.cinert, 10 * j), at6(&d.cvel, 6 * j));
        inline::cross_force(&mut cfrc, at6(&d.cvel, 6 * j), &cfrc_corr);
        blas::add_to(&mut cfrc_body, &cfrc, 6);
        blas::sub(
            &mut d.cfrc_int[6 * j..],
            &cfrc_body,
            &d.cfrc_ext[6 * j..],
            6,
        );
    }
    for j in (1..nbody).rev() {
        let child = *at6(&d.cfrc_int, 6 * j);
        blas::add_to(
            &mut d.cfrc_int[6 * m.body_parentid[j] as usize..],
            &child,
            6,
        );
    }
    d.flg_rnepost = 1;
}

/// None selects d.qfrc_bias, preserving C's output-identity sleep filtering.
pub fn tendon_bias(m: &Model, d: &mut Data, mut qfrc: Option<&mut [f64]>) {
    let sleep = enabled(m, ENBL_SLEEP) && (d.ntree_awake as usize) < m.ntree;
    let sleep_filter = qfrc.is_none() && sleep;
    let ntendon = m.ntendon;
    for i in 0..ntendon {
        if sleep && sleep_state(m, d, OBJ_TENDON, i as i32) == S_ASLEEP {
            continue;
        }
        let armature = m.tendon_armature[i] + actuator_armature(m, OBJ_TENDON, i);
        if armature == 0.0 {
            continue;
        }
        let mut qvel = d.scratch.num(m.nv);
        blas::copy(&mut qvel, &d.qvel, m.nv);
        let dot = tendon_dot(m, d, i as i32, &qvel);
        d.scratch.put_num(qvel);
        let coef = armature * dot;
        if coef != 0.0 {
            let nnz = m.ten_j_rownnz[i] as usize;
            let adr = m.ten_j_rowadr[i] as usize;
            let colind = &m.ten_j_colind[adr..];
            let ten_j = &d.ten_j[adr..];
            for j in 0..nnz {
                let dof = colind[j] as usize;
                if sleep_filter && sleep_state(m, d, OBJ_DOF, dof as i32) == S_ASLEEP {
                    continue;
                }
                if let Some(qfrc) = qfrc.as_deref_mut() {
                    qfrc[dof] += coef * ten_j[j];
                } else {
                    d.qfrc_bias[dof] += coef * ten_j[j];
                }
            }
        }
    }
}
