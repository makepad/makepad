//! Springs, dampers, gravity compensation, fluid forces and contact adhesion.
//!
//! Translated from `src/engine/engine_passive.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust.

use crate::{
    core_constraint, core_util,
    data::Data,
    inline::*,
    metric,
    model::{disabled, enabled, Model},
    sleep::sleep_state,
    support::apply_ft,
    types::*,
    util_blas, util_misc, util_spatial,
};

// Omitted flex functions: mj_flexPassiveInterp, mj_flexPassiveBendInterp,
// mj_flexPassiveBend, mj_flexPassiveBend1D, mj_flexPassiveStretch.
// Their private helper mju_dphi2D is omitted with its only caller.

// Spring and damper forces.
fn springdamper(m: &Model, d: &mut Data) {
    let nv = m.nv;
    let ntendon = m.ntendon;
    let enbl_spring = !disabled(m, DSBL_SPRING);
    let enbl_damper = !disabled(m, DSBL_DAMPER);
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.ntree_awake as usize) < m.ntree;
    let nbody = if sleep_filter {
        d.nbody_awake as usize
    } else {
        m.nbody
    };
    if enbl_spring {
        for b in 0..nbody {
            let i = if sleep_filter {
                d.body_awake_ind[b] as usize
            } else {
                b
            };
            let jnt_start = m.body_jntadr[i];
            let jnt_end = jnt_start + m.body_jntnum[i];
            for j in jnt_start..jnt_end {
                let j = j as usize;
                let stiffness = m.jnt_stiffness[j];
                let spoly = &m.jnt_stiffnesspoly[NPOLY * j..];
                if stiffness == 0.0 && util_misc::is_zero(spoly, NPOLY) != 0 {
                    continue;
                }
                let mut padr = m.jnt_qposadr[j] as usize;
                let mut dadr = m.jnt_dofadr[j] as usize;
                let mut qspring = if m.jnt_plasticadr[j] >= 0 {
                    &d.plastic[m.jnt_plasticadr[j] as usize..]
                } else {
                    &m.qpos_spring[padr..]
                };
                // C's free-joint case falls through to the ball-joint rotation.
                if m.jnt_type[j] == JNT_FREE {
                    let mut dif = [0.0; 3];
                    sub3(&mut dif, at3(&d.qpos, padr), at3(qspring, 0));
                    let r = util_blas::norm3(&dif);
                    let k = util_misc::poly_force(stiffness, spoly, r, NPOLY, 0);
                    add_to_scl3(at3_mut(&mut d.qfrc_spring, dadr), &dif, -k);
                    dadr += 3;
                    padr += 3;
                    qspring = &qspring[3..];
                }
                match m.jnt_type[j] {
                    JNT_FREE | JNT_BALL => {
                        let mut dif = [0.0; 3];
                        let mut quat = [0.0; 4];
                        copy4(&mut quat, at4(&d.qpos, padr));
                        util_blas::normalize4(&mut quat);
                        sub_quat(&mut dif, &quat, at4(qspring, 0));
                        let r = util_blas::norm3(&dif);
                        let k = util_misc::poly_force(stiffness, spoly, r, NPOLY, 0);
                        add_to_scl3(at3_mut(&mut d.qfrc_spring, dadr), &dif, -k);
                    }
                    JNT_SLIDE | JNT_HINGE => {
                        let x = d.qpos[padr] - qspring[0];
                        d.qfrc_spring[dadr] =
                            -x * util_misc::poly_force(stiffness, spoly, x, NPOLY, 0);
                    }
                    _ => {}
                }
            }
        }
    }
    if enbl_damper {
        let nv_awake = if sleep_filter {
            d.nv_awake as usize
        } else {
            nv
        };
        for j in 0..nv_awake {
            let i = if sleep_filter {
                d.dof_awake_ind[j] as usize
            } else {
                j
            };
            let mut poly = [0.0; NPOLY];
            util_blas::copy(&mut poly, &m.dof_dampingpoly[NPOLY * i..], NPOLY);
            let damping = m.dof_damping[i]
                + core_util::actuator_damping(m, OBJ_JOINT, m.dof_jntid[i] as usize, &mut poly);
            if damping != 0.0 || util_misc::is_zero(&poly, NPOLY) == 0 {
                let v = d.qvel[i];
                d.qfrc_damper[i] = -v * util_misc::poly_force(damping, &poly, v, NPOLY, 1);
            }
        }
    }
    for f in 0..m.nflex {
        if m.flex_rigid[f] != 0
            || (sleep_filter && sleep_state(m, d, OBJ_FLEX, f as i32) != S_AWAKE)
        {
            continue;
        }
        if m.flex_dim[f] == 1 {
            unported!("mj_flexPassiveBend1D");
        }
        if m.flex_interp[f] != 0 {
            // mj_flexPassiveBendInterp follows the untranslated interpolation call.
            unported!("mj_flexPassiveInterp");
        } else {
            // mj_flexPassiveStretch follows the untranslated bending call.
            unported!("mj_flexPassiveBend");
        }
    }
    for f in 0..m.nflex {
        let stiffness = if enbl_spring {
            m.flex_edgestiffness[f]
        } else {
            0.0
        };
        let damping = if enbl_damper {
            m.flex_edgedamping[f]
        } else {
            0.0
        };
        if m.flex_rigid[f] != 0 || (stiffness == 0.0 && damping == 0.0) {
            continue;
        }
        if sleep_filter && sleep_state(m, d, OBJ_FLEX, f as i32) != S_AWAKE {
            continue;
        }
        let edgeend = m.flex_edgeadr[f] + m.flex_edgenum[f];
        for e in m.flex_edgeadr[f]..edgeend {
            let e = e as usize;
            if m.flexedge_rigid[e] != 0 {
                continue;
            }
            let frc_spring = stiffness * (m.flexedge_length0[e] - d.flexedge_length[e]);
            let frc_damper = -damping * d.flexedge_velocity[e];
            let end = m.flexedge_j_rowadr[e] + m.flexedge_j_rownnz[e];
            for j in m.flexedge_j_rowadr[e]..end {
                let j = j as usize;
                let colind = m.flexedge_j_colind[j] as usize;
                // C's J clashes with j after snake-casing.
                let j_mat = d.flexedge_j[j];
                d.qfrc_spring[colind] += j_mat * frc_spring;
                d.qfrc_damper[colind] += j_mat * frc_damper;
            }
        }
    }
    for i in 0..ntendon {
        if sleep_filter && sleep_state(m, d, OBJ_TENDON, i as i32) != S_AWAKE {
            continue;
        }
        let mut frc_spring = 0.0;
        let mut frc_damper = 0.0;
        core_util::tendon_spring_damper(m, d, i, &mut frc_spring, &mut frc_damper);
        if frc_spring != 0.0 || frc_damper != 0.0 {
            let end = m.ten_j_rowadr[i] + m.ten_j_rownnz[i];
            for j in m.ten_j_rowadr[i]..end {
                let j = j as usize;
                let k = m.ten_j_colind[j] as usize;
                // C's J clashes with j after snake-casing.
                let j_mat = d.ten_j[j];
                d.qfrc_spring[k] += j_mat * frc_spring;
                d.qfrc_damper[k] += j_mat * frc_damper;
            }
        }
    }
}

// Body-level gravity compensation, return 1 if any, 0 otherwise.
fn gravcomp(m: &Model, d: &mut Data) -> i32 {
    if m.flg_gravcomp == 0 || disabled(m, DSBL_GRAVITY) || util_blas::norm3(&m.opt.gravity) == 0.0 {
        return 0;
    }
    let mut has_gravcomp = 0;
    let mut force = [0.0; 3];
    let torque = [0.0; 3];
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nbody_awake as usize) < m.nbody;
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
        if m.body_gravcomp[i] != 0.0 {
            has_gravcomp = 1;
            scl3(
                &mut force,
                &m.opt.gravity,
                -(m.body_mass[i] * m.body_gravcomp[i]),
            );
            let point = *at3(&d.xipos, 3 * i);
            // Move the output out while apply_ft borrows Data for scratch and Jacobians.
            let mut qfrc = std::mem::take(&mut d.qfrc_gravcomp);
            apply_ft(
                m,
                d,
                Some(&force),
                Some(&torque),
                &point,
                i as i32,
                &mut qfrc,
            );
            d.qfrc_gravcomp = qfrc;
        }
    }
    has_gravcomp
}

fn fluid(m: &Model, d: &mut Data) -> i32 {
    if m.opt.viscosity == 0.0 && m.opt.density == 0.0 {
        return 0;
    }
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nbody_awake as usize) < m.nbody;
    let nbody = if sleep_filter {
        d.nbody_awake as usize
    } else {
        m.nbody
    };
    for b in 0..nbody {
        let i = if sleep_filter {
            d.body_awake_ind[b] as usize
        } else {
            b
        };
        if m.body_mass[i] < MINVAL {
            continue;
        }
        let mut use_ellipsoid_model = 0;
        let geomnum = m.body_geomnum[i] as usize;
        let mut j = 0;
        while j < geomnum && use_ellipsoid_model == 0 {
            let geomid = m.body_geomadr[i] as usize + j;
            use_ellipsoid_model += (m.geom_fluid[NFLUID * geomid] > 0.0) as i32;
            j += 1;
        }
        if use_ellipsoid_model != 0 {
            ellipsoid_fluid_model(m, d, i);
        } else {
            inertia_box_fluid_model(m, d, i);
        }
    }
    1
}

pub fn contact_passive(m: &Model, d: &mut Data) -> i32 {
    if disabled(m, DSBL_CONTACT) || d.ncon == 0 || m.nv == 0 {
        return 0;
    }
    if d.efm_active == 0 {
        for f in 0..m.nflex {
            if core_util::eff_flex_contact_possible(m, f) != 0 {
                panic!("passive flex contact needs the effective metric: run the position stage (mj_forward, or mj_fwdPosition) before mj_passive");
            }
        }
        return 0;
    }
    if d.nefmcon == 0 {
        return 0;
    }
    let mut qfrc = std::mem::take(&mut d.qfrc_spring);
    metric::eff_contact_force(d, &mut qfrc);
    d.qfrc_spring = qfrc;
    1
}

// Constant attraction along the normals of adhesive contacts.
pub fn adhesion(m: &Model, d: &mut Data) -> i32 {
    let ncon = d.ncon as usize;
    let issparse = core_util::is_sparse(m) != 0;
    let nv = m.nv;
    let mut has_adhesion = 0;
    if m.flg_adhesion == 0 || disabled(m, DSBL_CONTACT) || ncon == 0 || nv == 0 {
        return 0;
    }
    for i in 0..ncon {
        if d.contact[i].adhesion != 0.0 && d.contact[i].exclude <= 1 {
            has_adhesion = 1;
            break;
        }
    }
    if has_adhesion == 0 {
        return 0;
    }
    let mut jac = d.scratch.num(nv);
    let mut jacdif = d.scratch.num(3 * nv);
    let mut jac1p = d.scratch.num(3 * nv);
    let mut jac2p = d.scratch.num(3 * nv);
    let mut qfrc = d.scratch.num(nv);
    let mut chain = if issparse {
        d.scratch.int(nv)
    } else {
        Vec::new()
    };
    for i in 0..ncon {
        let con = d.contact[i];
        if con.adhesion == 0.0 || con.exclude > 1 {
            continue;
        }
        // C's NV clashes with nv after snake-casing.
        let nv_mat = core_constraint::contact_jacobian(
            m,
            d,
            i,
            1,
            &mut jacdif,
            None,
            &mut jac1p,
            &mut jac2p,
            None,
            None,
            &mut chain,
        );
        if nv_mat == 0 {
            continue;
        }
        util_blas::mul_mat_mat(&mut jac, &con.frame, &jacdif, 1, 3, nv_mat);
        if !issparse {
            util_blas::add_to_scl(&mut d.qfrc_adhesion, &jac, -con.adhesion, nv);
        } else {
            util_blas::scl(&mut qfrc, &jac, -con.adhesion, nv_mat);
            for j in 0..nv_mat {
                d.qfrc_adhesion[chain[j] as usize] += qfrc[j];
            }
        }
    }
    d.scratch.put_num(jac);
    d.scratch.put_num(jacdif);
    d.scratch.put_num(jac1p);
    d.scratch.put_num(jac2p);
    d.scratch.put_num(qfrc);
    if issparse {
        d.scratch.put_int(chain);
    }
    1
}

pub fn passive(m: &Model, d: &mut Data) {
    let sleep_filter = enabled(m, ENBL_SLEEP) && (d.nv_awake as usize) < m.nv;
    let nv = if sleep_filter {
        d.nv_awake as usize
    } else {
        m.nv
    };
    if sleep_filter {
        util_blas::zero_ind(&mut d.qfrc_spring, nv, &d.dof_awake_ind);
        util_blas::zero_ind(&mut d.qfrc_damper, nv, &d.dof_awake_ind);
        util_blas::zero_ind(&mut d.qfrc_gravcomp, nv, &d.dof_awake_ind);
        util_blas::zero_ind(&mut d.qfrc_fluid, nv, &d.dof_awake_ind);
        util_blas::zero_ind(&mut d.qfrc_adhesion, nv, &d.dof_awake_ind);
        util_blas::zero_ind(&mut d.qfrc_passive, nv, &d.dof_awake_ind);
    } else {
        util_blas::zero(&mut d.qfrc_spring, nv);
        util_blas::zero(&mut d.qfrc_damper, nv);
        util_blas::zero(&mut d.qfrc_gravcomp, nv);
        util_blas::zero(&mut d.qfrc_fluid, nv);
        util_blas::zero(&mut d.qfrc_adhesion, nv);
        util_blas::zero(&mut d.qfrc_passive, nv);
    }
    if disabled(m, DSBL_SPRING) && disabled(m, DSBL_DAMPER) {
        return;
    }
    springdamper(m, d);
    let has_gravcomp = gravcomp(m, d);
    let has_fluid = fluid(m, d);
    contact_passive(m, d);
    let has_adhesion = adhesion(m, d);
    if sleep_filter {
        util_blas::add_ind(
            &mut d.qfrc_passive,
            &d.qfrc_spring,
            &d.qfrc_damper,
            &d.dof_awake_ind,
            nv,
        );
    } else {
        util_blas::add(&mut d.qfrc_passive, &d.qfrc_spring, &d.qfrc_damper, nv);
    }
    if has_fluid != 0 {
        if sleep_filter {
            util_blas::add_to_ind(&mut d.qfrc_passive, &d.qfrc_fluid, &d.dof_awake_ind, nv);
        } else {
            util_blas::add_to(&mut d.qfrc_passive, &d.qfrc_fluid, nv);
        }
    }
    if has_adhesion != 0 {
        if sleep_filter {
            util_blas::add_to_ind(&mut d.qfrc_passive, &d.qfrc_adhesion, &d.dof_awake_ind, nv);
        } else {
            util_blas::add_to(&mut d.qfrc_passive, &d.qfrc_adhesion, nv);
        }
    }
    if has_gravcomp != 0 {
        let ndof = if sleep_filter {
            d.nv_awake as usize
        } else {
            nv
        };
        for v in 0..ndof {
            let dof = if sleep_filter {
                d.dof_awake_ind[v] as usize
            } else {
                v
            };
            if m.jnt_actgravcomp[m.dof_jntid[dof] as usize] == 0 {
                d.qfrc_passive[dof] += d.qfrc_gravcomp[dof];
            }
        }
    }
    // mjcb_passive: user callbacks are not supported yet.
    // The plugin loop calls compute for plugins with mjPLUGIN_PASSIVE; every plugin the port admits
    // is an SDF plugin (check_supported), which has no PASSIVE capability, so it calls nothing.
}

// Fluid forces based on inertia-box approximation.
pub fn inertia_box_fluid_model(m: &Model, d: &mut Data, i: usize) {
    let mut lvel = [0.0; 6];
    let mut wind = [0.0; 6];
    let mut lwind = [0.0; 6];
    let mut lfrc = [0.0; 6];
    let mut bfrc = [0.0; 6];
    let mut box_ = [0.0; 3];
    let inertia = at3(&m.body_inertia, 3 * i);
    box_[0] = (util_misc::max(MINVAL, inertia[1] + inertia[2] - inertia[0]) / m.body_mass[i] * 6.0)
        .sqrt();
    box_[1] = (util_misc::max(MINVAL, inertia[0] + inertia[2] - inertia[1]) / m.body_mass[i] * 6.0)
        .sqrt();
    box_[2] = (util_misc::max(MINVAL, inertia[0] + inertia[1] - inertia[2]) / m.body_mass[i] * 6.0)
        .sqrt();
    core_util::object_velocity(m, d, OBJ_BODY, i, &mut lvel, 1);
    util_blas::zero(&mut wind, 6);
    copy3(at3_mut(&mut wind, 3), &m.opt.wind);
    util_spatial::transform_spatial(
        &mut lwind,
        &wind,
        0,
        at3(&d.xipos, 3 * i),
        at3(&d.subtree_com, 3 * m.body_rootid[i] as usize),
        Some(at9(&d.ximat, 9 * i)),
    );
    sub_from3(at3_mut(&mut lvel, 3), at3(&lwind, 3));
    util_blas::zero(&mut lfrc, 6);
    if m.opt.viscosity > 0.0 {
        let diam = (box_[0] + box_[1] + box_[2]) / 3.0;
        scl3(
            at3_mut(&mut lfrc, 0),
            at3(&lvel, 0),
            -PI * diam * diam * diam * m.opt.viscosity,
        );
        scl3(
            at3_mut(&mut lfrc, 3),
            at3(&lvel, 3),
            -3.0 * PI * diam * m.opt.viscosity,
        );
    }
    if m.opt.density > 0.0 {
        lfrc[3] -= 0.5 * m.opt.density * box_[1] * box_[2] * lvel[3].abs() * lvel[3];
        lfrc[4] -= 0.5 * m.opt.density * box_[0] * box_[2] * lvel[4].abs() * lvel[4];
        lfrc[5] -= 0.5 * m.opt.density * box_[0] * box_[1] * lvel[5].abs() * lvel[5];
        lfrc[0] -= m.opt.density
            * box_[0]
            * (box_[1] * box_[1] * box_[1] * box_[1] + box_[2] * box_[2] * box_[2] * box_[2])
            * lvel[0].abs()
            * lvel[0]
            / 64.0;
        lfrc[1] -= m.opt.density
            * box_[1]
            * (box_[0] * box_[0] * box_[0] * box_[0] + box_[2] * box_[2] * box_[2] * box_[2])
            * lvel[1].abs()
            * lvel[1]
            / 64.0;
        lfrc[2] -= m.opt.density
            * box_[2]
            * (box_[0] * box_[0] * box_[0] * box_[0] + box_[1] * box_[1] * box_[1] * box_[1])
            * lvel[2].abs()
            * lvel[2]
            / 64.0;
    }
    mul_mat_vec3(at3_mut(&mut bfrc, 0), at9(&d.ximat, 9 * i), at3(&lfrc, 0));
    mul_mat_vec3(at3_mut(&mut bfrc, 3), at9(&d.ximat, 9 * i), at3(&lfrc, 3));
    let point = *at3(&d.xipos, 3 * i);
    let mut qfrc = std::mem::take(&mut d.qfrc_fluid);
    apply_ft(
        m,
        d,
        Some(at3(&bfrc, 3)),
        Some(at3(&bfrc, 0)),
        &point,
        i as i32,
        &mut qfrc,
    );
    d.qfrc_fluid = qfrc;
}

// Fluid forces based on ellipsoid approximation.
pub fn ellipsoid_fluid_model(m: &Model, d: &mut Data, bodyid: usize) {
    let mut lvel = [0.0; 6];
    let mut wind = [0.0; 6];
    let mut lwind = [0.0; 6];
    let mut lfrc = [0.0; 6];
    let mut bfrc = [0.0; 6];
    let mut geom_interaction_coef = 0.0;
    let mut magnus_lift_coef = 0.0;
    let mut kutta_lift_coef = 0.0;
    let mut semiaxes = [0.0; 3];
    let mut virtual_mass = [0.0; 3];
    let mut virtual_inertia = [0.0; 3];
    let mut blunt_drag_coef = 0.0;
    let mut slender_drag_coef = 0.0;
    let mut ang_drag_coef = 0.0;
    for j in 0..m.body_geomnum[bodyid] as usize {
        let geomid = m.body_geomadr[bodyid] as usize + j;
        util_misc::geom_semi_axes(
            &mut semiaxes,
            at3(&m.geom_size, 3 * geomid),
            m.geom_type[geomid],
        );
        read_fluid_geom_interaction(
            &m.geom_fluid[NFLUID * geomid..],
            &mut geom_interaction_coef,
            &mut blunt_drag_coef,
            &mut slender_drag_coef,
            &mut ang_drag_coef,
            &mut kutta_lift_coef,
            &mut magnus_lift_coef,
            &mut virtual_mass,
            &mut virtual_inertia,
        );
        if geom_interaction_coef == 0.0 {
            continue;
        }
        core_util::object_velocity(m, d, OBJ_GEOM, geomid, &mut lvel, 1);
        util_blas::zero(&mut wind, 6);
        copy3(at3_mut(&mut wind, 3), &m.opt.wind);
        util_spatial::transform_spatial(
            &mut lwind,
            &wind,
            0,
            at3(&d.geom_xpos, 3 * geomid),
            at3(&d.subtree_com, 3 * m.body_rootid[bodyid] as usize),
            Some(at9(&d.geom_xmat, 9 * geomid)),
        );
        sub_from3(at3_mut(&mut lvel, 3), at3(&lwind, 3));
        util_blas::zero(&mut lfrc, 6);
        added_mass_forces(
            &lvel,
            None,
            m.opt.density,
            &virtual_mass,
            &virtual_inertia,
            &mut lfrc,
        );
        viscous_forces(
            &lvel,
            m.opt.density,
            m.opt.viscosity,
            &semiaxes,
            magnus_lift_coef,
            kutta_lift_coef,
            blunt_drag_coef,
            slender_drag_coef,
            ang_drag_coef,
            &mut lfrc,
        );
        let input = lfrc;
        util_blas::scl(&mut lfrc, &input, geom_interaction_coef, 6);
        mul_mat_vec3(
            at3_mut(&mut bfrc, 0),
            at9(&d.geom_xmat, 9 * geomid),
            at3(&lfrc, 0),
        );
        mul_mat_vec3(
            at3_mut(&mut bfrc, 3),
            at9(&d.geom_xmat, 9 * geomid),
            at3(&lfrc, 3),
        );
        let point = *at3(&d.geom_xpos, 3 * geomid);
        let mut qfrc = std::mem::take(&mut d.qfrc_fluid);
        apply_ft(
            m,
            d,
            Some(at3(&bfrc, 3)),
            Some(at3(&bfrc, 0)),
            &point,
            bodyid as i32,
            &mut qfrc,
        );
        d.qfrc_fluid = qfrc;
    }
}

// Compute forces due to fluid mass moving with the body.
pub fn added_mass_forces(
    local_vels: &[f64; 6],
    local_accels: Option<&[f64; 6]>,
    fluid_density: f64,
    virtual_mass: &[f64; 3],
    virtual_inertia: &[f64; 3],
    local_force: &mut [f64; 6],
) {
    let lin_vel = [local_vels[3], local_vels[4], local_vels[5]];
    let ang_vel = [local_vels[0], local_vels[1], local_vels[2]];
    let virtual_lin_mom = [
        fluid_density * virtual_mass[0] * lin_vel[0],
        fluid_density * virtual_mass[1] * lin_vel[1],
        fluid_density * virtual_mass[2] * lin_vel[2],
    ];
    let virtual_ang_mom = [
        fluid_density * virtual_inertia[0] * ang_vel[0],
        fluid_density * virtual_inertia[1] * ang_vel[1],
        fluid_density * virtual_inertia[2] * ang_vel[2],
    ];

    // disabled due to dependency on qacc but included for completeness
    if let Some(local_accels) = local_accels {
        local_force[0] -= fluid_density * virtual_inertia[0] * local_accels[0];
        local_force[1] -= fluid_density * virtual_inertia[1] * local_accels[1];
        local_force[2] -= fluid_density * virtual_inertia[2] * local_accels[2];
        local_force[3] -= fluid_density * virtual_mass[0] * local_accels[3];
        local_force[4] -= fluid_density * virtual_mass[1] * local_accels[4];
        local_force[5] -= fluid_density * virtual_mass[2] * local_accels[5];
    }

    let mut added_mass_force = [0.0; 3];
    let mut added_mass_torque1 = [0.0; 3];
    let mut added_mass_torque2 = [0.0; 3];
    cross(&mut added_mass_force, &virtual_lin_mom, &ang_vel);
    cross(&mut added_mass_torque1, &virtual_lin_mom, &lin_vel);
    cross(&mut added_mass_torque2, &virtual_ang_mom, &ang_vel);

    add_to3(at3_mut(local_force, 0), &added_mass_torque1);
    add_to3(at3_mut(local_force, 0), &added_mass_torque2);
    add_to3(at3_mut(local_force, 3), &added_mass_force);
}

// inlined helper functions
fn pow4(val: f64) -> f64 {
    return (val * val) * (val * val);
}

fn pow2(val: f64) -> f64 {
    return val * val;
}

fn ellipsoid_max_moment(size: &[f64; 3], dir: usize) -> f64 {
    let d0 = size[dir];
    let d1 = size[(dir + 1) % 3];
    let d2 = size[(dir + 2) % 3];
    return 8.0 / 15.0 * PI * d0 * pow4(util_misc::max(d1, d2));
}

// lift and drag forces due to motion in the fluid
pub fn viscous_forces(
    local_vels: &[f64; 6],
    fluid_density: f64,
    fluid_viscosity: f64,
    size: &[f64; 3],
    magnus_lift_coef: f64,
    kutta_lift_coef: f64,
    blunt_drag_coef: f64,
    slender_drag_coef: f64,
    ang_drag_coef: f64,
    local_force: &mut [f64; 6],
) {
    let lin_vel = [local_vels[3], local_vels[4], local_vels[5]];
    let ang_vel = [local_vels[0], local_vels[1], local_vels[2]];
    let volume = 4.0 / 3.0 * PI * size[0] * size[1] * size[2];
    let d_max = util_misc::max(util_misc::max(size[0], size[1]), size[2]);
    let d_min = util_misc::min(util_misc::min(size[0], size[1]), size[2]);
    let d_mid = size[0] + size[1] + size[2] - d_max - d_min;
    let a_max = PI * d_max * d_mid;

    let mut magnus_force = [0.0; 3];
    cross(&mut magnus_force, &ang_vel, &lin_vel);
    magnus_force[0] *= magnus_lift_coef * fluid_density * volume;
    magnus_force[1] *= magnus_lift_coef * fluid_density * volume;
    magnus_force[2] *= magnus_lift_coef * fluid_density * volume;

    // unit velocity and normalized semi-axes
    let speed = util_blas::norm3(&lin_vel);
    let inv_speed = if speed > MINVAL { 1.0 / speed } else { 0.0 };
    let v = [
        lin_vel[0] * inv_speed,
        lin_vel[1] * inv_speed,
        lin_vel[2] * inv_speed,
    ];
    let inv_dmax = if d_max > MINVAL { 1.0 / d_max } else { 0.0 };
    let s = [size[0] * inv_dmax, size[1] * inv_dmax, size[2] * inv_dmax];

    // the dot product between velocity and the normal to the cross-section that
    // defines the body's projection along velocity is proj_num/sqrt(proj_denom)
    let a_raw = pow2(s[1] * s[2]);
    let b_raw = pow2(s[2] * s[0]);
    let c_raw = pow2(s[0] * s[1]);
    let proj_num = a_raw * v[0] * v[0] + b_raw * v[1] * v[1] + c_raw * v[2] * v[2];
    let inv_proj_num = if proj_num > MINVAL {
        1.0 / proj_num
    } else {
        0.0
    };
    let a = a_raw * inv_proj_num;
    let b = b_raw * inv_proj_num;
    let c = c_raw * inv_proj_num;
    let proj_denom = a * a * v[0] * v[0] + b * b * v[1] * v[1] + c * c * v[2] * v[2];

    // projected surface in the direction of the velocity
    let a_proj = PI * d_max * d_max * (proj_num * proj_denom).sqrt();

    // not-unit normal to ellipsoid's projected area in the direction of velocity
    let norm = [a * v[0], b * v[1], c * v[2]];

    // cosine between velocity and normal to the surface
    // divided by proj_denom instead of sqrt(proj_denom) to account for skipped normalization in norm
    let cos_alpha = if proj_denom > MINVAL {
        1.0 / proj_denom
    } else {
        0.0
    };
    let mut kutta_circ = [0.0; 3];
    cross(&mut kutta_circ, &norm, &lin_vel);
    kutta_circ[0] *= kutta_lift_coef * fluid_density * cos_alpha * a_proj;
    kutta_circ[1] *= kutta_lift_coef * fluid_density * cos_alpha * a_proj;
    kutta_circ[2] *= kutta_lift_coef * fluid_density * cos_alpha * a_proj;
    let mut kutta_force = [0.0; 3];
    cross(&mut kutta_force, &kutta_circ, &lin_vel);

    // viscous force and torque in Stokes flow, analytical for spherical bodies
    let eq_sphere_d = 2.0 / 3.0 * (size[0] + size[1] + size[2]);
    let lin_visc_force_coef = 3.0 * PI * eq_sphere_d;
    let lin_visc_torq_coef = PI * eq_sphere_d * eq_sphere_d * eq_sphere_d;

    // moments of inertia used to compute angular quadratic drag
    let i_max = 8.0 / 15.0 * PI * d_mid * pow4(d_max);
    let ii = [
        ellipsoid_max_moment(size, 0),
        ellipsoid_max_moment(size, 1),
        ellipsoid_max_moment(size, 2),
    ];
    let mom_visc = [
        ang_vel[0] * (ang_drag_coef * ii[0] + slender_drag_coef * (i_max - ii[0])),
        ang_vel[1] * (ang_drag_coef * ii[1] + slender_drag_coef * (i_max - ii[1])),
        ang_vel[2] * (ang_drag_coef * ii[2] + slender_drag_coef * (i_max - ii[2])),
    ];

    let drag_lin_coef =  // linear plus quadratic
                               fluid_viscosity*lin_visc_force_coef + fluid_density*speed*(
    a_proj*blunt_drag_coef + slender_drag_coef*(a_max - a_proj));
    let drag_ang_coef =  // linear plus quadratic
                               fluid_viscosity * lin_visc_torq_coef +
                               fluid_density * util_blas::norm3(&mom_visc);

    local_force[0] -= drag_ang_coef * ang_vel[0];
    local_force[1] -= drag_ang_coef * ang_vel[1];
    local_force[2] -= drag_ang_coef * ang_vel[2];
    local_force[3] += magnus_force[0] + kutta_force[0] - drag_lin_coef * lin_vel[0];
    local_force[4] += magnus_force[1] + kutta_force[1] - drag_lin_coef * lin_vel[1];
    local_force[5] += magnus_force[2] + kutta_force[2] - drag_lin_coef * lin_vel[2];
}

// Read the geom_fluid_coefs array into its constituent parts.
pub fn read_fluid_geom_interaction(
    geom_fluid_coefs: &[f64],
    geom_fluid_coef: &mut f64,
    blunt_drag_coef: &mut f64,
    slender_drag_coef: &mut f64,
    ang_drag_coef: &mut f64,
    kutta_lift_coef: &mut f64,
    magnus_lift_coef: &mut f64,
    virtual_mass: &mut [f64; 3],
    virtual_inertia: &mut [f64; 3],
) {
    let mut i = 0;
    *geom_fluid_coef = geom_fluid_coefs[i];
    i += 1;
    *blunt_drag_coef = geom_fluid_coefs[i];
    i += 1;
    *slender_drag_coef = geom_fluid_coefs[i];
    i += 1;
    *ang_drag_coef = geom_fluid_coefs[i];
    i += 1;
    *kutta_lift_coef = geom_fluid_coefs[i];
    i += 1;
    *magnus_lift_coef = geom_fluid_coefs[i];
    i += 1;
    virtual_mass[0] = geom_fluid_coefs[i];
    i += 1;
    virtual_mass[1] = geom_fluid_coefs[i];
    i += 1;
    virtual_mass[2] = geom_fluid_coefs[i];
    i += 1;
    virtual_inertia[0] = geom_fluid_coefs[i];
    i += 1;
    virtual_inertia[1] = geom_fluid_coefs[i];
    i += 1;
    virtual_inertia[2] = geom_fluid_coefs[i];
    i += 1;
    if i != NFLUID {
        panic!("wrong number of entries.");
    }
}
