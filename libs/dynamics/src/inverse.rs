//! Inverse dynamics: the forces that produce a given acceleration, and the forward/inverse comparison.
//!
//! Translated from `src/engine/engine_inverse.c` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2021 DeepMind Technologies Limited);
//! changed: translated to Rust; the active implicit effective metric's mirror stays untranslated, as in
//! metric.rs.

use crate::collision_driver::collision;
use crate::core_constraint as constraint;
use crate::core_smooth as smooth;
use crate::core_util;
use crate::data::Data;
use crate::derivative;
use crate::forward::{check_discrete, fwd_velocity};
use crate::metric;
use crate::model::{Model, disabled, enabled};
use crate::sensor;
use crate::support;
use crate::types::*;
use crate::util_blas as blas;
use crate::util_errmem::error;
use crate::util_misc as misc;
use crate::util_sparse as sparse;

/// Position-dependent computations.
pub fn inv_position(m: &Model, d: &mut Data) {
    // clear flag for lazy evaluation
    d.flg_energypos = 0;

    smooth::kinematics(m, d);
    smooth::com_pos(m, d);
    smooth::camlight(m, d);
    smooth::flex(m, d);
    smooth::tendon(m, d);

    smooth::make_m(m, d);
    smooth::factor_m(m, d);

    collision(m, d);

    constraint::make_constraint(m, d);

    // compute exact diagonal if enabled
    if enabled(m, ENBL_DIAGEXACT) {
        constraint::project_constraint(m, d);
    }

    smooth::transmission(m, d);

    // implicit effective metric: multiply-only build (no factorization) for the inverse
    metric::eff_build(m, d, core_util::is_metric(m), 0);
}

/// Velocity-dependent computations.
pub fn inv_velocity(m: &Model, d: &mut Data) {
    fwd_velocity(m, d);
}

/// Convert discrete-time qacc to continuous-time qacc.
fn discrete_acc(m: &Model, d: &mut Data) {
    let nv = m.nv;
    let h = m.opt.timestep;
    let mut qfrc = d.scratch.num(nv);

    // use selected integrator
    match m.opt.integrator {
        INT_DISCRETE => {
            if enabled(m, ENBL_IPC) {
                error("mj_discreteAcc: discrete inverse dynamics is not supported with flag ipc");
            }
            // qacc is already the discrete step map: the effective-metric terms are handled natively by
            // inverse_skip, so INVDISCRETE is implied and there is nothing to do
            d.scratch.put_num(qfrc);
            return;
        }
        INT_RK4 => {
            // not supported by RK4
            error("mj_discreteAcc: discrete inverse dynamics is not supported by RK4 integrator");
        }
        INT_EULER => {
            // check for dof damping if disable flag is not set
            let mut dof_damping = false;
            if !disabled(m, DSBL_EULERDAMP) && !disabled(m, DSBL_DAMPER) {
                for i in 0..nv {
                    if m.dof_damping[i] > 0.0
                        || misc::is_zero(&m.dof_dampingpoly[NPOLY * i..], NPOLY) == 0
                        || m.jnt_actuatorid[m.dof_jntid[i] as usize] != -1
                    {
                        dof_damping = true;
                        break;
                    }
                }
            }

            // if disabled or no dof damping, nothing to do
            if !dof_damping {
                d.scratch.put_num(qfrc);
                return;
            }

            // set qfrc = (M + h*diag(B)) * qacc
            support::mul_m(m, d, &mut qfrc, &d.qacc);
            for i in 0..nv {
                let v = d.qvel[i];
                let mut poly = [0.0; NPOLY];
                blas::copy(&mut poly, &m.dof_dampingpoly[NPOLY * i..], NPOLY);
                let damping = m.dof_damping[i]
                    + core_util::actuator_damping(m, OBJ_JOINT, m.dof_jntid[i] as usize, &mut poly);
                let damp_deriv = misc::d_x_poly_force(damping, &poly, v, NPOLY, 1);
                qfrc[i] += h * damp_deriv * d.qacc[i];
            }
        }
        INT_IMPLICIT => {
            // compute qDeriv
            derivative::d_smooth_vel(m, d, 1);

            // gather qLU <- M (lower to full), in scratch: d.q_lu holds the forward factorization
            let mut q_lu = d.scratch.num(m.n_d);
            misc::gather_masked(&mut q_lu, &d.m, &m.map_m2d, m.n_d);

            // set qLU = M - dt*qDeriv
            blas::add_to_scl(&mut q_lu, &d.q_deriv, -h, m.n_d);

            // set qfrc = qLU * qacc
            sparse::mul_mat_vec_sparse(&mut qfrc, &q_lu, &d.qacc, nv, &m.d_rownnz, &m.d_rowadr, &m.d_colind, None);
            d.scratch.put_num(q_lu);
        }
        INT_IMPLICITFAST => {
            // compute analytical derivative qDeriv; skip rne derivative
            derivative::d_smooth_vel(m, d, 0);

            // modified mass matrix: gather qH <- qDeriv (full to lower), in scratch: d.q_h holds the forward
            // factorization
            let mut q_h = d.scratch.num(m.n_c);
            misc::gather(&mut q_h, &d.q_deriv, Some(&m.map_d2m), m.n_c);

            // set qH = M - dt*qDeriv (mju_addScl aliases its second input; it reads and writes each element in
            // order)
            for i in 0..m.n_c {
                q_h[i] = d.m[i] + q_h[i] * -h;
            }

            // set qfrc = (M - dt*qDeriv) * qacc
            sparse::mul_sym_vec_sparse(&mut qfrc, &q_h, &d.qacc, nv, &m.m_rownnz, &m.m_rowadr, &m.m_colind);

            // standalone free bodies: overwrite block rows with the unsymmetric local product, including the
            // bias (gyroscopic) derivative, mirroring implicit_skip
            for j in 0..m.njnt {
                let mut a = [0.0; 36];
                if derivative::d_free_mhat(m, d, j, h, &mut a, 0) == 0 {
                    continue;
                }
                let adr = m.jnt_dofadr[j] as usize;
                blas::mul_mat_vec(&mut qfrc[adr..], &a, &d.qacc[adr..], 6, 6);
            }
            d.scratch.put_num(q_h);
        }
        _ => {}
    }

    // solve for qacc: qfrc = M * qacc
    let mut qacc = std::mem::take(&mut d.qacc);
    smooth::solve_m(m, d, &mut qacc, Some(&qfrc), 1);
    d.qacc = qacc;

    d.scratch.put_num(qfrc);

    // refresh the metric's velocity-stage values
    metric::eff_shift(m, d);
}

/// Inverse constraint solver.
pub fn inv_constraint(m: &Model, d: &mut Data) {
    let nefc = d.nefc as usize;

    // no constraints: clear, return
    if nefc == 0 {
        blas::zero(&mut d.qfrc_constraint, m.nv);
        return;
    }

    let mut jar = d.scratch.num(nefc);

    // compute jar = Jac*qacc - aref
    constraint::mul_jac_vec(m, d, &mut jar, &d.qacc);
    blas::sub_from(&mut jar, &d.efc_aref, nefc);

    // call update function
    constraint::constraint_update(m, d, &jar, None, 0);

    d.scratch.put_num(jar);
}

/// Inverse dynamics with skip; `skipstage` is a STAGE_ value.
pub fn inverse_skip(m: &Model, d: &mut Data, skipstage: i32, skipsensor: i32) {
    let nv = m.nv;

    // validate option combinations for the discrete integrator
    check_discrete(m);

    // position-dependent
    if skipstage < STAGE_POS {
        inv_position(m, d);
        if skipsensor == 0 {
            sensor::sensor_pos(m, d);
        }
        if enabled(m, ENBL_ENERGY) && d.flg_energypos == 0 {
            sensor::energy_pos(m, d);
        }
    }

    // velocity-dependent
    if skipstage < STAGE_VEL {
        inv_velocity(m, d);
        if skipsensor == 0 {
            sensor::sensor_vel(m, d);
        }
        if enabled(m, ENBL_ENERGY) && d.flg_energyvel == 0 {
            sensor::energy_vel(m, d);
        }
    }

    // actuation-stage metric refresh: reads ctrl/act from Data without running actuation. The inverse only
    // multiplies by the metric: the backbone factor is needed only by the exact constraint diagonal
    metric::eff_actuation(m, d, enabled(m, ENBL_DIAGEXACT) as i32);
    if core_util::is_metric(m) != 0 {
        constraint::regularize_constraint(m, d, 0);
        constraint::reference_constraint(m, d);
    }

    let invdiscrete = enabled(m, ENBL_INVDISCRETE);
    let mut saved_qacc = None;
    if invdiscrete {
        // save current qacc
        let mut qacc = d.scratch.num(nv);
        blas::copy(&mut qacc, &d.qacc, nv);
        saved_qacc = Some(qacc);

        // modify qacc in-place
        discrete_acc(m, d);
    }

    // acceleration-dependent
    inv_constraint(m, d);

    // sum of bias forces in qfrc_inverse = centripetal + Coriolis + tendon bias
    let mut qfrc_inverse = std::mem::take(&mut d.qfrc_inverse);
    smooth::rne(m, d, 0, Some(&mut qfrc_inverse));
    smooth::tendon_bias(m, d, Some(&mut qfrc_inverse));
    d.qfrc_inverse = qfrc_inverse;

    if skipsensor == 0 {
        d.flg_rnepost = 0; // clear flag for lazy evaluation
        sensor::sensor_acc(m, d);
    }

    // compute Ma = M*qacc
    let mut ma = d.scratch.num(nv);
    support::mul_m(m, d, &mut ma, &d.qacc);

    // implicit effective metric (built in inv_position): the forward dynamics solved (M+K)*qacc = qfrc + c +
    // J'*f, so the discrete-consistent inverse adds K*qacc - c
    if d.efm_active != 0 {
        crate::unported!("mj_inverseSkip (active metric)");
    }

    // qfrc_inverse += Ma - qfrc_passive - qfrc_constraint
    for i in 0..nv {
        d.qfrc_inverse[i] += ma[i] - d.qfrc_passive[i] - d.qfrc_constraint[i];
    }

    if let Some(qacc) = saved_qacc.as_ref() {
        // restore qacc
        blas::copy(&mut d.qacc, qacc, nv);
    }

    d.scratch.put_num(ma);
    if let Some(qacc) = saved_qacc {
        d.scratch.put_num(qacc);
    }
}

/// Inverse dynamics.
pub fn inverse(m: &Model, d: &mut Data) {
    inverse_skip(m, d, STAGE_NONE, 0);
}

/// Compare forward and inverse dynamics, without changing the results of forward:
///    solver_fwdinv[0] = norm(qfrc_constraint(forward) - qfrc_constraint(inverse))
///    solver_fwdinv[1] = norm(qfrc_applied(forward) - qfrc_inverse)
pub fn compare_fwd_inv(m: &Model, d: &mut Data) {
    let nv = m.nv;
    let nefc = d.nefc as usize;

    // clear result, return if no constraints
    d.solver_fwdinv = [0.0; 2];
    if nefc == 0 {
        return;
    }

    // allocate
    let mut qforce = d.scratch.num(nv);
    let mut dif = d.scratch.num(nv);
    let mut save_qfrc_constraint = d.scratch.num(nv);
    let mut save_efc_force = d.scratch.num(nefc);

    // qforce = qfrc_applied + J'*xfrc_applied + qfrc_actuator: should equal the result of inverse dynamics
    blas::add(&mut qforce, &d.qfrc_applied, &d.qfrc_actuator, nv);
    support::xfrc_accumulate(m, d, &mut qforce);

    // save forward dynamics results that are about to be modified
    blas::copy(&mut save_qfrc_constraint, &d.qfrc_constraint, nv);
    blas::copy(&mut save_efc_force, &d.efc_force, nefc);

    // run inverse dynamics, do not update position and velocity
    inverse_skip(m, d, STAGE_VEL, 1); // 1: do not recompute sensors and energy

    // compute statistics
    blas::sub(&mut dif, &save_qfrc_constraint, &d.qfrc_constraint, nv);
    d.solver_fwdinv[0] = blas::norm(&dif, nv);
    blas::sub(&mut dif, &qforce, &d.qfrc_inverse, nv);
    d.solver_fwdinv[1] = blas::norm(&dif, nv);

    // restore forward dynamics results
    blas::copy(&mut d.qfrc_constraint, &save_qfrc_constraint, nv);
    blas::copy(&mut d.efc_force, &save_efc_force, nefc);

    d.scratch.put_num(save_efc_force);
    d.scratch.put_num(save_qfrc_constraint);
    d.scratch.put_num(dif);
    d.scratch.put_num(qforce);
}
