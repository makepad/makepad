use crate::dynamics::JointGraphEdge;
use crate::dynamics::solver::joint_constraint::generic_joint_constraint::GenericJointConstraint;
use crate::dynamics::solver::joint_constraint::joint_velocity_constraint::JointConstraint;
use crate::math::{DVector, Real};

#[cfg(feature = "simd-is-enabled")]
use crate::math::{SIMD_WIDTH, SimdReal};

use crate::dynamics::solver::solver_body::SolverBodies;

#[derive(Debug)]
pub enum AnyJointConstraintMut<'a> {
    Generic(&'a mut GenericJointConstraint),
    Rigid(&'a mut JointConstraint<Real, 1>),
    #[cfg(feature = "simd-is-enabled")]
    SimdRigid(&'a mut JointConstraint<SimdReal, SIMD_WIDTH>),
}

impl AnyJointConstraintMut<'_> {
    pub fn remove_bias(&mut self) {
        match self {
            Self::Rigid(c) => c.remove_bias_from_rhs(),
            Self::Generic(c) => c.remove_bias_from_rhs(),
            #[cfg(feature = "simd-is-enabled")]
            Self::SimdRigid(c) => c.remove_bias_from_rhs(),
        }
    }

    pub fn solve(
        &mut self,
        generic_jacobians: &DVector,
        solver_vels: &mut SolverBodies,
        generic_solver_vels: &mut DVector,
    ) {
        match self {
            Self::Rigid(c) => c.solve(solver_vels),
            Self::Generic(c) => c.solve(generic_jacobians, solver_vels, generic_solver_vels),
            #[cfg(feature = "simd-is-enabled")]
            Self::SimdRigid(c) => c.solve(solver_vels),
        }
    }

    pub fn writeback_impulses(&mut self, joints_all: &mut [JointGraphEdge]) {
        match self {
            Self::Rigid(c) => c.writeback_impulses(joints_all),
            Self::Generic(c) => c.writeback_impulses(joints_all),
            #[cfg(feature = "simd-is-enabled")]
            Self::SimdRigid(c) => c.writeback_impulses(joints_all),
        }
    }
}
