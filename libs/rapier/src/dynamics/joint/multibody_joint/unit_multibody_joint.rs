#![allow(missing_docs)] // For downcast.

use crate::dynamics::integration_parameters::SpringCoefficients;
use crate::dynamics::joint::MultibodyLink;
use crate::dynamics::solver::{GenericJointConstraint, WritebackId};
use crate::dynamics::{IntegrationParameters, JointMotor, Multibody};
use crate::math::{DVector, Real};

/// Initializes and generate the velocity constraints applicable to the multibody links attached
/// to this multibody_joint.
pub fn unit_joint_limit_constraint(
    params: &IntegrationParameters,
    multibody: &Multibody,
    link: &MultibodyLink,
    limits: [Real; 2],
    curr_pos: Real,
    dof_id: usize,
    j_id: &mut usize,
    jacobians: &mut DVector,
    constraints: &mut [GenericJointConstraint],
    insert_at: &mut usize,
    softness: SpringCoefficients<Real>,
) {
    let ndofs = multibody.ndofs();
    let min_enabled = curr_pos < limits[0];
    let max_enabled = limits[1] < curr_pos;

    // Compute per-joint ERP and CFM
    let erp_inv_dt = softness.erp_inv_dt(params.dt);
    let cfm_coeff = softness.cfm_coeff(params.dt);

    let rhs_bias = ((curr_pos - limits[1]).max(0.0) - (limits[0] - curr_pos).max(0.0)) * erp_inv_dt;
    let rhs_wo_bias = 0.0;

    let dof_j_id = *j_id + dof_id + link.assembly_id;
    jacobians.rows_mut(*j_id, ndofs * 2).fill(0.0);
    jacobians[dof_j_id] = 1.0;
    jacobians[dof_j_id + ndofs] = 1.0;
    multibody
        .inv_augmented_mass()
        .solve_mut(&mut jacobians.rows_mut(*j_id + ndofs, ndofs));

    let lhs = jacobians[dof_j_id + ndofs]; // = J^t * M^-1 J
    let impulse_bounds = [
        min_enabled as u32 as Real * -Real::MAX,
        max_enabled as u32 as Real * Real::MAX,
    ];

    let constraint = GenericJointConstraint {
        is_rigid_body1: false,
        solver_vel1: u32::MAX,
        ndofs1: 0,
        j_id1: 0,

        is_rigid_body2: false,
        solver_vel2: multibody.solver_id,
        ndofs2: ndofs,
        j_id2: *j_id,
        joint_id: usize::MAX, // TODO: we don’t support impulse writeback for internal constraints yet.
        impulse: 0.0,
        impulse_bounds,
        inv_lhs: crate::utils::inv(lhs),
        rhs: rhs_wo_bias + rhs_bias,
        rhs_wo_bias,
        cfm_coeff,
        cfm_gain: 0.0,
        writeback_id: WritebackId::Limit(dof_id),
    };

    constraints[*insert_at] = constraint;
    *insert_at += 1;

    *j_id += 2 * ndofs;
}

/// Initializes and generate the velocity constraints applicable to the multibody links attached
/// to this multibody_joint.
pub fn unit_joint_motor_constraint(
    params: &IntegrationParameters,
    multibody: &Multibody,
    link: &MultibodyLink,
    motor: &JointMotor,
    curr_pos: Real,
    limits: Option<[Real; 2]>,
    dof_id: usize,
    j_id: &mut usize,
    jacobians: &mut DVector,
    constraints: &mut [GenericJointConstraint],
    insert_at: &mut usize,
) {
    let inv_dt = params.inv_dt();
    let ndofs = multibody.ndofs();
    let motor_params = motor.motor_params(params.dt);

    let dof_j_id = *j_id + dof_id + link.assembly_id;
    jacobians.rows_mut(*j_id, ndofs * 2).fill(0.0);
    jacobians[dof_j_id] = 1.0;
    jacobians[dof_j_id + ndofs] = 1.0;
    multibody
        .inv_augmented_mass()
        .solve_mut(&mut jacobians.rows_mut(*j_id + ndofs, ndofs));

    let lhs = jacobians[dof_j_id + ndofs]; // = J^t * M^-1 J
    let impulse_bounds = [-motor_params.max_impulse, motor_params.max_impulse];

    let mut rhs_wo_bias = 0.0;
    if motor_params.erp_inv_dt != 0.0 {
        rhs_wo_bias += (curr_pos - motor_params.target_pos) * motor_params.erp_inv_dt;
    }

    let mut target_vel = motor_params.target_vel;
    if let Some(limits) = limits {
        target_vel = target_vel.clamp(
            (limits[0] - curr_pos) * inv_dt,
            (limits[1] - curr_pos) * inv_dt,
        );
    };

    rhs_wo_bias += -target_vel;

    let constraint = GenericJointConstraint {
        is_rigid_body1: false,
        solver_vel1: u32::MAX,
        ndofs1: 0,
        j_id1: 0,

        is_rigid_body2: false,
        solver_vel2: multibody.solver_id,
        ndofs2: ndofs,
        j_id2: *j_id,
        joint_id: usize::MAX, // TODO: we don’t support impulse writeback for internal constraints yet.
        impulse: 0.0,
        impulse_bounds,
        cfm_coeff: motor_params.cfm_coeff,
        cfm_gain: motor_params.cfm_gain,
        inv_lhs: crate::utils::inv(lhs),
        rhs: rhs_wo_bias,
        rhs_wo_bias,
        writeback_id: WritebackId::Limit(dof_id),
    };

    constraints[*insert_at] = constraint;
    *insert_at += 1;

    *j_id += 2 * ndofs;
}
