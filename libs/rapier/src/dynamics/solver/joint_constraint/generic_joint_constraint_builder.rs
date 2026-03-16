use crate::dynamics::solver::MotorParameters;
use crate::dynamics::solver::joint_constraint::generic_joint_constraint::GenericJointConstraint;
use crate::dynamics::solver::joint_constraint::joint_velocity_constraint::WritebackId;
use crate::dynamics::solver::joint_constraint::{JointConstraintHelper, JointSolverBody};
use crate::dynamics::{
    GenericJoint, ImpulseJoint, IntegrationParameters, JointIndex, Multibody, MultibodyJointSet,
    MultibodyLinkId, RigidBodySet,
};
use crate::math::{ANG_DIM, DIM, DVector, Real, SPATIAL_DIM, Vector};
use crate::utils;
use crate::utils::{ComponentMul, IndexMut2, MatrixColumn};

use crate::dynamics::integration_parameters::SpringCoefficients;
use crate::dynamics::solver::ConstraintsCounts;
use crate::dynamics::solver::solver_body::SolverBodies;
#[cfg(feature = "dim3")]
use crate::utils::AngularInertiaOps;
use parry::math::{AngVector, Pose};

#[derive(Copy, Clone)]
enum LinkOrBody {
    Link(MultibodyLinkId),
    Body(u32),
    Fixed,
}

#[derive(Copy, Clone)]
pub enum LinkOrBodyRef<'a> {
    Link(&'a Multibody, usize),
    Body(u32),
    Fixed,
}

#[allow(clippy::large_enum_variant)]
#[derive(Copy, Clone)]
pub enum GenericJointConstraintBuilder {
    Internal(JointGenericInternalConstraintBuilder),
    External(JointGenericExternalConstraintBuilder),
    Empty, // No constraint
}

#[derive(Copy, Clone)]
pub struct JointGenericExternalConstraintBuilder {
    link1: LinkOrBody,
    link2: LinkOrBody,
    joint_id: JointIndex,
    joint: GenericJoint,
    j_id: usize,
    // These are solver body for both joints, except that
    // the world_com is actually in local-space.
    local_body1: JointSolverBody<Real, 1>,
    local_body2: JointSolverBody<Real, 1>,
    multibodies_ndof: usize,
    constraint_id: usize,
}

impl JointGenericExternalConstraintBuilder {
    pub fn generate(
        joint_id: JointIndex,
        joint: &ImpulseJoint,
        bodies: &RigidBodySet,
        multibodies: &MultibodyJointSet,
        out_builder: &mut GenericJointConstraintBuilder,
        j_id: &mut usize,
        jacobians: &mut DVector,
        out_constraint_id: &mut usize,
    ) {
        let starting_j_id = *j_id;
        let rb1 = &bodies[joint.body1];
        let rb2 = &bodies[joint.body2];

        let solver_vel1 = rb1.effective_active_set_offset();
        let solver_vel2 = rb2.effective_active_set_offset();
        let local_body1 = JointSolverBody {
            im: rb1.mprops.effective_inv_mass,
            ii: rb1.mprops.effective_world_inv_inertia,
            world_com: rb1.mprops.local_mprops.local_com,
            solver_vel: [solver_vel1],
        };
        let local_body2 = JointSolverBody {
            im: rb2.mprops.effective_inv_mass,
            ii: rb2.mprops.effective_world_inv_inertia,
            world_com: rb2.mprops.local_mprops.local_com,
            solver_vel: [solver_vel2],
        };

        let mut multibodies_ndof = 0;
        let link1 = if solver_vel1 == u32::MAX {
            LinkOrBody::Fixed
        } else if let Some(link) = multibodies.rigid_body_link(joint.body1) {
            multibodies_ndof += multibodies[link.multibody].ndofs();
            LinkOrBody::Link(*link)
        } else {
            // Dynamic rigid-body.
            multibodies_ndof += SPATIAL_DIM;
            LinkOrBody::Body(solver_vel1)
        };

        let link2 = if solver_vel2 == u32::MAX {
            LinkOrBody::Fixed
        } else if let Some(link) = multibodies.rigid_body_link(joint.body2) {
            multibodies_ndof += multibodies[link.multibody].ndofs();
            LinkOrBody::Link(*link)
        } else {
            // Dynamic rigid-body.
            multibodies_ndof += SPATIAL_DIM;
            LinkOrBody::Body(solver_vel2)
        };

        if multibodies_ndof == 0 {
            return;
        }

        // For each solver contact we generate up to SPATIAL_DIM constraints, and each
        // constraints appends the multibodies jacobian and weighted jacobians.
        // Also note that for impulse_joints, the rigid-bodies will also add their jacobians
        // to the generic DVector.
        // TODO: is this count correct when we take both motors and limits into account?
        let required_jacobian_len = *j_id + multibodies_ndof * 2 * SPATIAL_DIM;

        // TODO: use a more precise increment.
        *j_id += multibodies_ndof * 2 * SPATIAL_DIM;

        if jacobians.nrows() < required_jacobian_len && !cfg!(feature = "parallel") {
            jacobians.resize_vertically_mut(required_jacobian_len, 0.0);
        }

        let mut joint_data = joint.data;
        joint_data.transform_to_solver_body_space(rb1, rb2);
        *out_builder = GenericJointConstraintBuilder::External(Self {
            link1,
            link2,
            joint_id,
            joint: joint_data,
            j_id: starting_j_id,
            local_body1,
            local_body2,
            multibodies_ndof,
            constraint_id: *out_constraint_id,
        });

        *out_constraint_id += ConstraintsCounts::from_joint(joint).num_constraints;
    }

    pub fn update(
        &self,
        params: &IntegrationParameters,
        multibodies: &MultibodyJointSet,
        bodies: &SolverBodies,
        jacobians: &mut DVector,
        out: &mut [GenericJointConstraint],
    ) {
        if self.multibodies_ndof == 0 {
            // The joint is between two static bodies, no constraint needed.
            return;
        }

        // NOTE: right now, the "update", is basically reconstructing all the
        //       constraints. Could we make this more incremental?
        let pos1;
        let pos2;
        let mb1;
        let mb2;

        match self.link1 {
            LinkOrBody::Link(link) => {
                let mb = &multibodies[link.multibody];
                pos1 = mb.link(link.id).unwrap().local_to_world;
                mb1 = LinkOrBodyRef::Link(mb, link.id);
            }
            LinkOrBody::Body(body1) => {
                pos1 = bodies.get_pose(body1).pose();
                mb1 = LinkOrBodyRef::Body(body1);
            }
            LinkOrBody::Fixed => {
                pos1 = Pose::IDENTITY;
                mb1 = LinkOrBodyRef::Fixed;
            }
        };
        match self.link2 {
            LinkOrBody::Link(link) => {
                let mb = &multibodies[link.multibody];
                pos2 = mb.link(link.id).unwrap().local_to_world;
                mb2 = LinkOrBodyRef::Link(mb, link.id);
            }
            LinkOrBody::Body(body2) => {
                pos2 = bodies.get_pose(body2).pose();
                mb2 = LinkOrBodyRef::Body(body2);
            }
            LinkOrBody::Fixed => {
                pos2 = Pose::IDENTITY;
                mb2 = LinkOrBodyRef::Fixed;
            }
        };

        let frame1 = pos1 * self.joint.local_frame1;
        let frame2 = pos2 * self.joint.local_frame2;

        let joint_body1 = JointSolverBody {
            world_com: pos1.translation, // the solver body pose is at the center of mass.
            ..self.local_body1
        };
        let joint_body2 = JointSolverBody {
            world_com: pos2.translation, // the solver body pose is at the center of mass.
            ..self.local_body2
        };

        let mut j_id = self.j_id;

        GenericJointConstraint::lock_axes(
            params,
            self.joint_id,
            &joint_body1,
            &joint_body2,
            mb1,
            mb2,
            &frame1,
            &frame2,
            &self.joint,
            jacobians,
            &mut j_id,
            &mut out[self.constraint_id..],
        );
    }
}

#[derive(Copy, Clone)]
pub struct JointGenericInternalConstraintBuilder {
    link: MultibodyLinkId,
    j_id: usize,
    constraint_id: usize,
}

impl JointGenericInternalConstraintBuilder {
    pub fn num_constraints(multibodies: &MultibodyJointSet, link_id: &MultibodyLinkId) -> usize {
        let multibody = &multibodies[link_id.multibody];
        let link = multibody.link(link_id.id).unwrap();
        link.joint().num_velocity_constraints()
    }

    pub fn generate(
        multibodies: &MultibodyJointSet,
        link_id: &MultibodyLinkId,
        out_builder: &mut GenericJointConstraintBuilder,
        j_id: &mut usize,
        jacobians: &mut DVector,
        out_constraint_id: &mut usize,
    ) {
        let multibody = &multibodies[link_id.multibody];
        let link = multibody.link(link_id.id).unwrap();
        let num_constraints = link.joint().num_velocity_constraints();

        if num_constraints == 0 {
            return;
        }

        *out_builder = GenericJointConstraintBuilder::Internal(Self {
            link: *link_id,
            j_id: *j_id,
            constraint_id: *out_constraint_id,
        });

        *j_id += num_constraints * multibody.ndofs() * 2;
        if jacobians.nrows() < *j_id {
            jacobians.resize_vertically_mut(*j_id, 0.0);
        }

        *out_constraint_id += num_constraints;
    }

    pub fn update(
        &self,
        params: &IntegrationParameters,
        multibodies: &MultibodyJointSet,
        jacobians: &mut DVector,
        out: &mut [GenericJointConstraint],
    ) {
        let mb = &multibodies[self.link.multibody];
        let link = mb.link(self.link.id).unwrap();
        link.joint().velocity_constraints(
            params,
            mb,
            link,
            self.j_id,
            jacobians,
            &mut out[self.constraint_id..],
        );
    }
}

impl JointSolverBody<Real, 1> {
    pub fn fill_jacobians(
        &self,
        unit_force: Vector,
        unit_torque: AngVector,
        j_id: &mut usize,
        jacobians: &mut DVector,
    ) {
        let wj_id = *j_id + SPATIAL_DIM;
        jacobians
            .fixed_rows_mut::<DIM>(*j_id)
            .copy_from_slice(unit_force.as_ref());
        #[cfg(feature = "dim2")]
        jacobians
            .fixed_rows_mut::<ANG_DIM>(*j_id + DIM)
            .copy_from_slice(&[unit_torque]);
        #[cfg(feature = "dim3")]
        jacobians
            .fixed_rows_mut::<ANG_DIM>(*j_id + DIM)
            .copy_from_slice(unit_torque.as_ref());

        {
            let mut out_invm_j = jacobians.fixed_rows_mut::<SPATIAL_DIM>(wj_id);
            out_invm_j
                .fixed_rows_mut::<DIM>(0)
                .copy_from_slice(self.im.component_mul(&unit_force).as_ref());

            #[cfg(feature = "dim2")]
            {
                out_invm_j[DIM] *= self.ii;
            }
            #[cfg(feature = "dim3")]
            {
                let invm_j = self.ii.transform_vector(unit_torque);
                out_invm_j
                    .fixed_rows_mut::<ANG_DIM>(DIM)
                    .copy_from_slice(invm_j.as_ref());
            }
        }

        *j_id += SPATIAL_DIM * 2;
    }
}

impl JointConstraintHelper<Real> {
    pub fn lock_jacobians_generic(
        &self,
        jacobians: &mut DVector,
        j_id: &mut usize,
        joint_id: JointIndex,
        body1: &JointSolverBody<Real, 1>,
        body2: &JointSolverBody<Real, 1>,
        mb1: LinkOrBodyRef,
        mb2: LinkOrBodyRef,
        writeback_id: WritebackId,
        lin_jac: Vector,
        ang_jac1: AngVector,
        ang_jac2: AngVector,
    ) -> GenericJointConstraint {
        let j_id1 = *j_id;
        let (ndofs1, solver_vel1, is_rigid_body1) = match mb1 {
            LinkOrBodyRef::Link(mb1, link_id1) => {
                mb1.fill_jacobians(link_id1, lin_jac, ang_jac1, j_id, jacobians);
                (mb1.ndofs(), mb1.solver_id, false)
            }
            LinkOrBodyRef::Body(_) => {
                body1.fill_jacobians(lin_jac, ang_jac1, j_id, jacobians);
                (SPATIAL_DIM, body1.solver_vel[0], true)
            }
            LinkOrBodyRef::Fixed => (0, u32::MAX, true),
        };

        let j_id2 = *j_id;
        let (ndofs2, solver_vel2, is_rigid_body2) = match mb2 {
            LinkOrBodyRef::Link(mb2, link_id2) => {
                mb2.fill_jacobians(link_id2, lin_jac, ang_jac2, j_id, jacobians);
                (mb2.ndofs(), mb2.solver_id, false)
            }
            LinkOrBodyRef::Body(_) => {
                body2.fill_jacobians(lin_jac, ang_jac2, j_id, jacobians);
                (SPATIAL_DIM, body2.solver_vel[0], true)
            }
            LinkOrBodyRef::Fixed => (0, u32::MAX, true),
        };

        let rhs_wo_bias = 0.0;

        GenericJointConstraint {
            is_rigid_body1,
            is_rigid_body2,
            solver_vel1,
            solver_vel2,
            ndofs1,
            j_id1,
            ndofs2,
            j_id2,
            joint_id,
            impulse: 0.0,
            impulse_bounds: [-Real::MAX, Real::MAX],
            inv_lhs: 0.0,
            rhs: rhs_wo_bias,
            rhs_wo_bias,
            cfm_coeff: 0.0,
            cfm_gain: 0.0,
            writeback_id,
        }
    }

    pub fn lock_linear_generic(
        &self,
        params: &IntegrationParameters,
        jacobians: &mut DVector,
        j_id: &mut usize,
        joint_id: JointIndex,
        body1: &JointSolverBody<Real, 1>,
        body2: &JointSolverBody<Real, 1>,
        mb1: LinkOrBodyRef,
        mb2: LinkOrBodyRef,
        locked_axis: usize,
        softness: SpringCoefficients<Real>,
        writeback_id: WritebackId,
    ) -> GenericJointConstraint {
        let lin_jac = self.basis.col(locked_axis);
        let ang_jac1 = self.cmat1_basis.column(locked_axis);
        let ang_jac2 = self.cmat2_basis.column(locked_axis);

        let mut c = self.lock_jacobians_generic(
            jacobians,
            j_id,
            joint_id,
            body1,
            body2,
            mb1,
            mb2,
            writeback_id,
            lin_jac,
            ang_jac1,
            ang_jac2,
        );

        let erp_inv_dt = softness.erp_inv_dt(params.dt);
        let rhs_bias = lin_jac.dot(self.lin_err) * erp_inv_dt;
        c.rhs += rhs_bias;
        c
    }

    pub fn limit_linear_generic(
        &self,
        params: &IntegrationParameters,
        jacobians: &mut DVector,
        j_id: &mut usize,
        joint_id: JointIndex,
        body1: &JointSolverBody<Real, 1>,
        body2: &JointSolverBody<Real, 1>,
        mb1: LinkOrBodyRef,
        mb2: LinkOrBodyRef,
        limited_axis: usize,
        limits: [Real; 2],
        softness: SpringCoefficients<Real>,
        writeback_id: WritebackId,
    ) -> GenericJointConstraint {
        let lin_jac = self.basis.col(limited_axis);
        let ang_jac1 = self.cmat1_basis.column(limited_axis);
        let ang_jac2 = self.cmat2_basis.column(limited_axis);

        let mut constraint = self.lock_jacobians_generic(
            jacobians,
            j_id,
            joint_id,
            body1,
            body2,
            mb1,
            mb2,
            writeback_id,
            lin_jac,
            ang_jac1,
            ang_jac2,
        );

        let dist = self.lin_err.dot(lin_jac);
        let min_enabled = dist <= limits[0];
        let max_enabled = limits[1] <= dist;

        let erp_inv_dt = softness.erp_inv_dt(params.dt);

        let rhs_bias = ((dist - limits[1]).max(0.0) - (limits[0] - dist).max(0.0)) * erp_inv_dt;
        constraint.rhs += rhs_bias;
        constraint.impulse_bounds = [
            min_enabled as u32 as Real * -Real::MAX,
            max_enabled as u32 as Real * Real::MAX,
        ];

        constraint
    }

    pub fn motor_linear_generic(
        &self,
        jacobians: &mut DVector,
        j_id: &mut usize,
        joint_id: JointIndex,
        body1: &JointSolverBody<Real, 1>,
        body2: &JointSolverBody<Real, 1>,
        mb1: LinkOrBodyRef,
        mb2: LinkOrBodyRef,
        motor_axis: usize,
        motor_params: &MotorParameters<Real>,
        writeback_id: WritebackId,
    ) -> GenericJointConstraint {
        let lin_jac = self.basis.col(motor_axis);
        let ang_jac1 = self.cmat1_basis.column(motor_axis);
        let ang_jac2 = self.cmat2_basis.column(motor_axis);

        // TODO: do we need the same trick as for the non-generic constraint?
        // if locked_ang_axes & (1 << motor_axis) != 0 {
        //     // FIXME: check that this also works for cases
        //     // whene not all the angular axes are locked.
        //     constraint.ang_jac1.fill(0.0);
        //     constraint.ang_jac2.fill(0.0);
        // }

        let mut constraint = self.lock_jacobians_generic(
            jacobians,
            j_id,
            joint_id,
            body1,
            body2,
            mb1,
            mb2,
            writeback_id,
            lin_jac,
            ang_jac1,
            ang_jac2,
        );

        let mut rhs_wo_bias = 0.0;
        if motor_params.erp_inv_dt != 0.0 {
            let dist = self.lin_err.dot(lin_jac);
            rhs_wo_bias += (dist - motor_params.target_pos) * motor_params.erp_inv_dt;
        }

        rhs_wo_bias += -motor_params.target_vel;

        constraint.impulse_bounds = [-motor_params.max_impulse, motor_params.max_impulse];
        constraint.rhs = rhs_wo_bias;
        constraint.rhs_wo_bias = rhs_wo_bias;
        constraint.cfm_coeff = motor_params.cfm_coeff;
        constraint.cfm_gain = motor_params.cfm_gain;
        constraint
    }

    pub fn lock_angular_generic(
        &self,
        params: &IntegrationParameters,
        jacobians: &mut DVector,
        j_id: &mut usize,
        joint_id: JointIndex,
        body1: &JointSolverBody<Real, 1>,
        body2: &JointSolverBody<Real, 1>,
        mb1: LinkOrBodyRef,
        mb2: LinkOrBodyRef,
        _locked_axis: usize,
        softness: SpringCoefficients<Real>,
        writeback_id: WritebackId,
    ) -> GenericJointConstraint {
        #[cfg(feature = "dim2")]
        let ang_jac: AngVector = 1.0;
        #[cfg(feature = "dim3")]
        let ang_jac = self.ang_basis.column(_locked_axis);

        let mut constraint = self.lock_jacobians_generic(
            jacobians,
            j_id,
            joint_id,
            body1,
            body2,
            mb1,
            mb2,
            writeback_id,
            Vector::ZERO,
            ang_jac,
            ang_jac,
        );

        let erp_inv_dt = softness.erp_inv_dt(params.dt);
        #[cfg(feature = "dim2")]
        let rhs_bias = self.ang_err.im * erp_inv_dt;
        #[cfg(feature = "dim3")]
        let rhs_bias = self.ang_err.xyz()[_locked_axis] * erp_inv_dt;
        constraint.rhs += rhs_bias;
        constraint
    }

    pub fn limit_angular_generic(
        &self,
        params: &IntegrationParameters,
        jacobians: &mut DVector,
        j_id: &mut usize,
        joint_id: JointIndex,
        body1: &JointSolverBody<Real, 1>,
        body2: &JointSolverBody<Real, 1>,
        mb1: LinkOrBodyRef,
        mb2: LinkOrBodyRef,
        _limited_axis: usize,
        limits: [Real; 2],
        softness: SpringCoefficients<Real>,
        writeback_id: WritebackId,
    ) -> GenericJointConstraint {
        #[cfg(feature = "dim2")]
        let ang_jac: AngVector = 1.0;
        #[cfg(feature = "dim3")]
        let ang_jac = self.ang_basis.column(_limited_axis);

        let mut constraint = self.lock_jacobians_generic(
            jacobians,
            j_id,
            joint_id,
            body1,
            body2,
            mb1,
            mb2,
            writeback_id,
            Vector::ZERO,
            ang_jac,
            ang_jac,
        );

        let s_limits = [(limits[0] / 2.0).sin(), (limits[1] / 2.0).sin()];
        #[cfg(feature = "dim2")]
        let s_ang = (self.ang_err.angle() / 2.0).sin();
        #[cfg(feature = "dim3")]
        let s_ang = self.ang_err.xyz()[_limited_axis];
        let min_enabled = s_ang <= s_limits[0];
        let max_enabled = s_limits[1] <= s_ang;
        let impulse_bounds = [
            min_enabled as u32 as Real * -Real::MAX,
            max_enabled as u32 as Real * Real::MAX,
        ];

        let erp_inv_dt = softness.erp_inv_dt(params.dt);
        let rhs_bias =
            ((s_ang - s_limits[1]).max(0.0) - (s_limits[0] - s_ang).max(0.0)) * erp_inv_dt;

        constraint.rhs += rhs_bias;
        constraint.impulse_bounds = impulse_bounds;
        constraint
    }

    pub fn motor_angular_generic(
        &self,
        jacobians: &mut DVector,
        j_id: &mut usize,
        joint_id: JointIndex,
        body1: &JointSolverBody<Real, 1>,
        body2: &JointSolverBody<Real, 1>,
        mb1: LinkOrBodyRef,
        mb2: LinkOrBodyRef,
        _motor_axis: usize,
        motor_params: &MotorParameters<Real>,
        writeback_id: WritebackId,
    ) -> GenericJointConstraint {
        #[cfg(feature = "dim2")]
        let ang_jac = 1.0;
        #[cfg(feature = "dim3")]
        let ang_jac = self.basis.col(_motor_axis);

        let mut constraint = self.lock_jacobians_generic(
            jacobians,
            j_id,
            joint_id,
            body1,
            body2,
            mb1,
            mb2,
            writeback_id,
            Vector::ZERO,
            ang_jac,
            ang_jac,
        );

        let mut rhs_wo_bias = 0.0;
        if motor_params.erp_inv_dt != 0.0 {
            #[cfg(feature = "dim2")]
            let s_ang_dist = (self.ang_err.angle() / 2.0).sin();
            #[cfg(feature = "dim3")]
            let s_ang_dist = self.ang_err.xyz()[_motor_axis];
            let s_target_ang = (motor_params.target_pos / 2.0).sin();
            rhs_wo_bias += utils::smallest_abs_diff_between_sin_angles(s_ang_dist, s_target_ang)
                * motor_params.erp_inv_dt;
        }

        rhs_wo_bias += -motor_params.target_vel;

        constraint.rhs_wo_bias = rhs_wo_bias;
        constraint.rhs = rhs_wo_bias;
        constraint.cfm_coeff = motor_params.cfm_coeff;
        constraint.cfm_gain = motor_params.cfm_gain;
        constraint.impulse_bounds = [-motor_params.max_impulse, motor_params.max_impulse];
        constraint
    }

    pub fn finalize_generic_constraints(
        jacobians: &mut DVector,
        constraints: &mut [GenericJointConstraint],
    ) {
        // TODO: orthogonalization doesn’t seem to give good results for multibodies?
        const ORTHOGONALIZE: bool = false;
        let len = constraints.len();

        if len == 0 {
            return;
        }

        let ndofs1 = constraints[0].ndofs1;
        let ndofs2 = constraints[0].ndofs2;

        // Use the modified Gramm-Schmidt orthogonalization.
        for j in 0..len {
            let c_j = &mut constraints[j];

            let jac_j1 = jacobians.rows(c_j.j_id1, ndofs1);
            let jac_j2 = jacobians.rows(c_j.j_id2, ndofs2);
            let w_jac_j1 = jacobians.rows(c_j.j_id1 + ndofs1, ndofs1);
            let w_jac_j2 = jacobians.rows(c_j.j_id2 + ndofs2, ndofs2);

            let dot_jj = jac_j1.dot(&w_jac_j1) + jac_j2.dot(&w_jac_j2);
            let cfm_gain = dot_jj * c_j.cfm_coeff + c_j.cfm_gain;
            let inv_dot_jj = crate::utils::simd_inv(dot_jj);
            c_j.inv_lhs = crate::utils::simd_inv(dot_jj + cfm_gain); // Don’t forget to update the inv_lhs.
            c_j.cfm_gain = cfm_gain;

            if c_j.impulse_bounds != [-Real::MAX, Real::MAX] {
                // Don't remove constraints with limited forces from the others
                // because they may not deliver the necessary forces to fulfill
                // the removed parts of other constraints.
                continue;
            }

            if !ORTHOGONALIZE {
                continue;
            }

            for i in (j + 1)..len {
                let (c_i, c_j) = constraints.index_mut_const(i, j);

                let jac_i1 = jacobians.rows(c_i.j_id1, ndofs1);
                let jac_i2 = jacobians.rows(c_i.j_id2, ndofs2);
                let w_jac_j1 = jacobians.rows(c_j.j_id1 + ndofs1, ndofs1);
                let w_jac_j2 = jacobians.rows(c_j.j_id2 + ndofs2, ndofs2);

                let dot_ij = jac_i1.dot(&w_jac_j1) + jac_i2.dot(&w_jac_j2);
                let coeff = dot_ij * inv_dot_jj;

                let (mut jac_i, jac_j) = jacobians.rows_range_pair_mut(
                    c_i.j_id1..c_i.j_id1 + 2 * (ndofs1 + ndofs2),
                    c_j.j_id1..c_j.j_id1 + 2 * (ndofs1 + ndofs2),
                );

                jac_i.axpy(-coeff, &jac_j, 1.0);

                c_i.rhs_wo_bias -= c_j.rhs_wo_bias * coeff;
                c_i.rhs -= c_j.rhs * coeff;
            }
        }
    }
}
