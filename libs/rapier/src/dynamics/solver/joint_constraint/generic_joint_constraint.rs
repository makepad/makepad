use super::LinkOrBodyRef;
use crate::dynamics::solver::joint_constraint::joint_velocity_constraint::WritebackId;
use crate::dynamics::solver::joint_constraint::{JointConstraintHelper, JointSolverBody};
use crate::dynamics::solver::solver_body::SolverBodies;
use crate::dynamics::{GenericJoint, IntegrationParameters, JointGraphEdge, JointIndex};
use crate::math::{DIM, DVector, Real, SPATIAL_DIM};
use na::{DVectorView, DVectorViewMut};
use parry::math::Pose;

#[derive(Debug, Copy, Clone)]
pub struct GenericJointConstraint {
    pub is_rigid_body1: bool,
    pub is_rigid_body2: bool,
    pub solver_vel1: u32,
    pub solver_vel2: u32,

    pub ndofs1: usize,
    pub j_id1: usize,
    pub ndofs2: usize,
    pub j_id2: usize,

    pub joint_id: JointIndex,

    pub impulse: Real,
    pub impulse_bounds: [Real; 2],
    pub inv_lhs: Real,
    pub rhs: Real,
    pub rhs_wo_bias: Real,
    pub cfm_coeff: Real,
    pub cfm_gain: Real,

    pub writeback_id: WritebackId,
}

impl Default for GenericJointConstraint {
    fn default() -> Self {
        GenericJointConstraint::invalid()
    }
}

impl GenericJointConstraint {
    pub fn invalid() -> Self {
        Self {
            is_rigid_body1: false,
            is_rigid_body2: false,
            solver_vel1: u32::MAX,
            solver_vel2: u32::MAX,
            ndofs1: usize::MAX,
            j_id1: usize::MAX,
            ndofs2: usize::MAX,
            j_id2: usize::MAX,
            joint_id: usize::MAX,
            impulse: 0.0,
            impulse_bounds: [-Real::MAX, Real::MAX],
            inv_lhs: Real::MAX,
            rhs: Real::MAX,
            rhs_wo_bias: Real::MAX,
            cfm_coeff: Real::MAX,
            cfm_gain: Real::MAX,
            writeback_id: WritebackId::Dof(0),
        }
    }

    pub fn lock_axes(
        params: &IntegrationParameters,
        joint_id: JointIndex,
        body1: &JointSolverBody<Real, 1>,
        body2: &JointSolverBody<Real, 1>,
        mb1: LinkOrBodyRef,
        mb2: LinkOrBodyRef,
        frame1: &Pose,
        frame2: &Pose,
        joint: &GenericJoint,
        jacobians: &mut DVector,
        j_id: &mut usize,
        out: &mut [Self],
    ) -> usize {
        let mut len = 0;
        let locked_axes = joint.locked_axes.bits();
        let motor_axes = joint.motor_axes.bits();
        let limit_axes = joint.limit_axes.bits();

        let builder = JointConstraintHelper::<Real>::new(
            frame1,
            frame2,
            &body1.world_com,
            &body2.world_com,
            locked_axes,
        );

        let start = len;
        for i in DIM..SPATIAL_DIM {
            if motor_axes & (1 << i) != 0 {
                out[len] = builder.motor_angular_generic(
                    jacobians,
                    j_id,
                    joint_id,
                    body1,
                    body2,
                    mb1,
                    mb2,
                    i - DIM,
                    &joint.motors[i].motor_params(params.dt),
                    WritebackId::Motor(i),
                );
                len += 1;
            }
        }
        for i in 0..DIM {
            if motor_axes & (1 << i) != 0 {
                out[len] = builder.motor_linear_generic(
                    jacobians,
                    j_id,
                    joint_id,
                    body1,
                    body2,
                    mb1,
                    mb2,
                    // locked_ang_axes,
                    i,
                    &joint.motors[i].motor_params(params.dt),
                    WritebackId::Motor(i),
                );
                len += 1;
            }
        }
        JointConstraintHelper::finalize_generic_constraints(jacobians, &mut out[start..len]);

        let start = len;
        for i in DIM..SPATIAL_DIM {
            if locked_axes & (1 << i) != 0 {
                out[len] = builder.lock_angular_generic(
                    params,
                    jacobians,
                    j_id,
                    joint_id,
                    body1,
                    body2,
                    mb1,
                    mb2,
                    i - DIM,
                    joint.softness,
                    WritebackId::Dof(i),
                );
                len += 1;
            }
        }
        for i in 0..DIM {
            if locked_axes & (1 << i) != 0 {
                out[len] = builder.lock_linear_generic(
                    params,
                    jacobians,
                    j_id,
                    joint_id,
                    body1,
                    body2,
                    mb1,
                    mb2,
                    i,
                    joint.softness,
                    WritebackId::Dof(i),
                );
                len += 1;
            }
        }

        for i in DIM..SPATIAL_DIM {
            if limit_axes & (1 << i) != 0 {
                out[len] = builder.limit_angular_generic(
                    params,
                    jacobians,
                    j_id,
                    joint_id,
                    body1,
                    body2,
                    mb1,
                    mb2,
                    i - DIM,
                    [joint.limits[i].min, joint.limits[i].max],
                    joint.softness,
                    WritebackId::Limit(i),
                );
                len += 1;
            }
        }
        for i in 0..DIM {
            if limit_axes & (1 << i) != 0 {
                out[len] = builder.limit_linear_generic(
                    params,
                    jacobians,
                    j_id,
                    joint_id,
                    body1,
                    body2,
                    mb1,
                    mb2,
                    i,
                    [joint.limits[i].min, joint.limits[i].max],
                    joint.softness,
                    WritebackId::Limit(i),
                );
                len += 1;
            }
        }

        JointConstraintHelper::finalize_generic_constraints(jacobians, &mut out[start..len]);
        len
    }

    fn wj_id1(&self) -> usize {
        self.j_id1 + self.ndofs1
    }

    fn wj_id2(&self) -> usize {
        self.j_id2 + self.ndofs2
    }

    fn solver_vel1<'a>(
        &self,
        solver_vels: &'a SolverBodies,
        generic_solver_vels: &'a DVector,
    ) -> DVectorView<'a, Real> {
        if self.solver_vel1 == u32::MAX {
            generic_solver_vels.rows(0, 0) // empty
        } else if self.is_rigid_body1 {
            solver_vels.vels[self.solver_vel1 as usize].as_vector_slice()
        } else {
            generic_solver_vels.rows(self.solver_vel1 as usize, self.ndofs1)
        }
    }

    fn solver_vel1_mut<'a>(
        &self,
        solver_vels: &'a mut SolverBodies,
        generic_solver_vels: &'a mut DVector,
    ) -> DVectorViewMut<'a, Real> {
        if self.solver_vel1 == u32::MAX {
            generic_solver_vels.rows_mut(0, 0) // empty
        } else if self.is_rigid_body1 {
            solver_vels.vels[self.solver_vel1 as usize].as_vector_slice_mut()
        } else {
            generic_solver_vels.rows_mut(self.solver_vel1 as usize, self.ndofs1)
        }
    }

    fn solver_vel2<'a>(
        &self,
        solver_vels: &'a SolverBodies,
        generic_solver_vels: &'a DVector,
    ) -> DVectorView<'a, Real> {
        if self.solver_vel2 == u32::MAX {
            generic_solver_vels.rows(0, 0) // empty
        } else if self.is_rigid_body2 {
            solver_vels.vels[self.solver_vel2 as usize].as_vector_slice()
        } else {
            generic_solver_vels.rows(self.solver_vel2 as usize, self.ndofs2)
        }
    }

    fn solver_vel2_mut<'a>(
        &self,
        solver_vels: &'a mut SolverBodies,
        generic_solver_vels: &'a mut DVector,
    ) -> DVectorViewMut<'a, Real> {
        if self.solver_vel2 == u32::MAX {
            generic_solver_vels.rows_mut(0, 0) // empty
        } else if self.is_rigid_body2 {
            solver_vels.vels[self.solver_vel2 as usize].as_vector_slice_mut()
        } else {
            generic_solver_vels.rows_mut(self.solver_vel2 as usize, self.ndofs2)
        }
    }

    pub fn solve(
        &mut self,
        jacobians: &DVector,
        solver_vels: &mut SolverBodies,
        generic_solver_vels: &mut DVector,
    ) {
        let jacobians = jacobians.as_slice();

        let solver_vel1 = self.solver_vel1(solver_vels, generic_solver_vels);
        let j1 = DVectorView::from_slice(&jacobians[self.j_id1..], self.ndofs1);
        let vel1 = j1.dot(&solver_vel1);

        let solver_vel2 = self.solver_vel2(solver_vels, generic_solver_vels);
        let j2 = DVectorView::from_slice(&jacobians[self.j_id2..], self.ndofs2);
        let vel2 = j2.dot(&solver_vel2);

        let dvel = self.rhs + (vel2 - vel1);
        let total_impulse = na::clamp(
            self.impulse + self.inv_lhs * (dvel - self.cfm_gain * self.impulse),
            self.impulse_bounds[0],
            self.impulse_bounds[1],
        );
        let delta_impulse = total_impulse - self.impulse;
        self.impulse = total_impulse;

        let mut solver_vel1 = self.solver_vel1_mut(solver_vels, generic_solver_vels);
        let wj1 = DVectorView::from_slice(&jacobians[self.wj_id1()..], self.ndofs1);
        solver_vel1.axpy(delta_impulse, &wj1, 1.0);

        let mut solver_vel2 = self.solver_vel2_mut(solver_vels, generic_solver_vels);
        let wj2 = DVectorView::from_slice(&jacobians[self.wj_id2()..], self.ndofs2);
        solver_vel2.axpy(-delta_impulse, &wj2, 1.0);
    }

    pub fn writeback_impulses(&self, joints_all: &mut [JointGraphEdge]) {
        // TODO: we don’t support impulse writeback for internal constraints yet.
        if self.joint_id != JointIndex::MAX {
            let joint = &mut joints_all[self.joint_id].weight;
            match self.writeback_id {
                WritebackId::Dof(i) => joint.impulses[i] = self.impulse,
                WritebackId::Limit(i) => joint.data.limits[i].impulse = self.impulse,
                WritebackId::Motor(i) => joint.data.motors[i].impulse = self.impulse,
            }
        }
    }

    pub fn remove_bias_from_rhs(&mut self) {
        self.rhs = self.rhs_wo_bias;
    }
}
