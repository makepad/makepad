use crate::dynamics::{JointAxesMask, Multibody, MultibodyLink, RigidBodySet};
use crate::math::{ANG_DIM, DIM, DVector, Jacobian, Pose, Real, SPATIAL_DIM};
use na::{self, SMatrix, SVector};

#[derive(Copy, Clone, Debug, PartialEq)]
/// Options for the jacobian-based Inverse Kinematics solver for multibodies.
pub struct InverseKinematicsOption {
    /// A damping coefficient.
    ///
    /// Small value can lead to overshooting preventing convergence. Large
    /// values can slow down convergence, requiring more iterations to converge.
    pub damping: Real,
    /// The maximum number of iterations the iterative IK solver can take.
    pub max_iters: usize,
    /// The axes the IK solver will solve for.
    pub constrained_axes: JointAxesMask,
    /// The error threshold on the linear error.
    ///
    /// If errors on both linear and angular parts fall below this
    /// threshold, the iterative resolution will stop.
    pub epsilon_linear: Real,
    /// The error threshold on the angular error.
    ///
    /// If errors on both linear and angular parts fall below this
    /// threshold, the iterative resolution will stop.
    pub epsilon_angular: Real,
}

impl Default for InverseKinematicsOption {
    fn default() -> Self {
        Self {
            damping: 1.0,
            max_iters: 10,
            constrained_axes: JointAxesMask::all(),
            epsilon_linear: 1.0e-3,
            epsilon_angular: 1.0e-3,
        }
    }
}

impl Multibody {
    /// Computes the displacement needed to have the link identified by `link_id` move by the
    /// desired transform.
    ///
    /// The displacement calculated by this function is added to the `displacement` vector.
    pub fn inverse_kinematics_delta(
        &self,
        link_id: usize,
        desired_movement: &SVector<Real, SPATIAL_DIM>,
        damping: Real,
        displacements: &mut DVector,
    ) {
        let body_jacobian = self.body_jacobian(link_id);
        Self::inverse_kinematics_delta_with_jacobian(
            body_jacobian,
            desired_movement,
            damping,
            displacements,
        );
    }

    /// Computes the displacement needed to have a link with the given jacobian move by the
    /// desired transform.
    ///
    /// The displacement calculated by this function is added to the `displacement` vector.
    pub fn inverse_kinematics_delta_with_jacobian(
        jacobian: &Jacobian<Real>,
        desired_movement: &SVector<Real, SPATIAL_DIM>,
        damping: Real,
        displacements: &mut DVector,
    ) {
        let identity = SMatrix::<Real, SPATIAL_DIM, SPATIAL_DIM>::identity();
        let jj = jacobian * &jacobian.transpose() + identity * (damping * damping);
        let inv_jj = jj.pseudo_inverse(1.0e-5).unwrap_or(identity);
        displacements.gemv_tr(1.0, jacobian, &(inv_jj * desired_movement), 1.0);
    }

    /// Computes the displacement needed to have the link identified by `link_id` have a pose
    /// equal (or as close as possible) to `target_pose`.
    ///
    /// If `displacement` is given non-zero, the current pose of the rigid-body is considered to be
    /// obtained from its current generalized coordinates summed with the `displacement` vector.
    ///
    /// The `displacements` vector is overwritten with the new displacement.
    ///
    /// The `joint_can_move` argument is a closure that lets you indicate which joint
    /// can be moved through the inverse-kinematics process. Any joint for which `joint_can_move`
    /// returns `false` will have its corresponding displacement constrained to 0.
    /// Set the closure to `|_| true` if all the joints are free to move.
    pub fn inverse_kinematics(
        &self,
        bodies: &RigidBodySet,
        link_id: usize,
        options: &InverseKinematicsOption,
        target_pose: &Pose,
        joint_can_move: impl Fn(&MultibodyLink) -> bool,
        displacements: &mut DVector,
    ) {
        let mut jacobian = Jacobian::zeros(0);
        let branch = self.kinematic_branch(link_id);
        let can_move: Vec<_> = branch
            .iter()
            .map(|id| joint_can_move(&self.links[*id]))
            .collect();

        for _ in 0..options.max_iters {
            let pose = self.forward_kinematics_single_branch(
                bodies,
                &branch,
                Some(displacements.as_slice()),
                Some(&mut jacobian),
            );

            // Adjust the jacobian to account for non-movable joints.
            for (id, can_move) in branch.iter().zip(can_move.iter()) {
                if !*can_move {
                    let link = &self.links[*id];
                    jacobian
                        .columns_mut(link.assembly_id, link.joint.ndofs())
                        .fill(0.0);
                }
            }

            let delta_lin = target_pose.translation - pose.translation;
            #[cfg(feature = "dim2")]
            let delta_ang = (target_pose.rotation * pose.rotation.inverse()).angle();
            #[cfg(feature = "dim3")]
            let delta_ang = (target_pose.rotation * pose.rotation.inverse()).to_scaled_axis();

            #[cfg(feature = "dim2")]
            let mut delta = SVector::<Real, 3>::new(delta_lin.x, delta_lin.y, delta_ang);
            #[cfg(feature = "dim3")]
            let mut delta = SVector::<Real, 6>::new(
                delta_lin.x,
                delta_lin.y,
                delta_lin.z,
                delta_ang.x,
                delta_ang.y,
                delta_ang.z
            );

            if !options.constrained_axes.contains(JointAxesMask::LIN_X) {
                delta[0] = 0.0;
            }
            if !options.constrained_axes.contains(JointAxesMask::LIN_Y) {
                delta[1] = 0.0;
            }
            #[cfg(feature = "dim3")]
            if !options.constrained_axes.contains(JointAxesMask::LIN_Z) {
                delta[2] = 0.0;
            }
            if !options.constrained_axes.contains(JointAxesMask::ANG_X) {
                delta[DIM] = 0.0;
            }
            #[cfg(feature = "dim3")]
            if !options.constrained_axes.contains(JointAxesMask::ANG_Y) {
                delta[DIM + 1] = 0.0;
            }
            #[cfg(feature = "dim3")]
            if !options.constrained_axes.contains(JointAxesMask::ANG_Z) {
                delta[DIM + 2] = 0.0;
            }

            // TODO: measure convergence on the error variation instead?
            if delta.rows(0, DIM).norm() <= options.epsilon_linear
                && delta.rows(DIM, ANG_DIM).norm() <= options.epsilon_angular
            {
                break;
            }

            Self::inverse_kinematics_delta_with_jacobian(
                &jacobian,
                &delta,
                options.damping,
                displacements,
            );
        }
    }
}

#[cfg(test)]
mod test {
    use crate::dynamics::{
        MultibodyJointHandle, MultibodyJointSet, RevoluteJointBuilder, RigidBodyBuilder,
        RigidBodySet,
    };
    use crate::math::{Jacobian, Real, Vector};
    use approx::assert_relative_eq;

    #[test]
    fn one_link_fwd_kinematics() {
        let mut bodies = RigidBodySet::new();
        let mut multibodies = MultibodyJointSet::new();

        let num_segments = 10;
        let body = RigidBodyBuilder::fixed();
        let mut last_body = bodies.insert(body);
        let mut last_link = MultibodyJointHandle::invalid();

        for _ in 0..num_segments {
            let body = RigidBodyBuilder::dynamic().can_sleep(false);
            let new_body = bodies.insert(body);

            #[cfg(feature = "dim2")]
            let builder = RevoluteJointBuilder::new();
            #[cfg(feature = "dim3")]
            let builder = RevoluteJointBuilder::new(Vector::Z);
            let link_ab = builder
                .local_anchor1((Vector::Y * (0.5 / num_segments as Real)).into())
                .local_anchor2((Vector::Y * (-0.5 / num_segments as Real)).into());
            last_link = multibodies
                .insert(last_body, new_body, link_ab, true)
                .unwrap();

            last_body = new_body;
        }

        let (multibody, last_id) = multibodies.get_mut(last_link).unwrap();
        multibody.forward_kinematics(&bodies, true); // Be sure all the dofs are up to date.
        assert_eq!(multibody.ndofs(), num_segments);

        /*
         * No displacement.
         */
        let mut jacobian2 = Jacobian::zeros(0);
        let link_pose1 = *multibody.link(last_id).unwrap().local_to_world();
        let jacobian1 = multibody.body_jacobian(last_id);
        let link_pose2 =
            multibody.forward_kinematics_single_link(&bodies, last_id, None, Some(&mut jacobian2));
        assert_eq!(link_pose1, link_pose2);
        assert_eq!(jacobian1, &jacobian2);

        /*
         * Arbitrary displacement.
         */
        let niter = 100;
        let displacement_part: Vec<_> = (0..multibody.ndofs())
            .map(|i| i as Real * -0.1 / niter as Real)
            .collect();
        let displacement_total: Vec<_> = displacement_part
            .iter()
            .map(|d| *d * niter as Real)
            .collect();
        let link_pose2 = multibody.forward_kinematics_single_link(
            &bodies,
            last_id,
            Some(&displacement_total),
            Some(&mut jacobian2),
        );

        for _ in 0..niter {
            multibody.apply_displacements(&displacement_part);
            multibody.forward_kinematics(&bodies, false);
        }

        let link_pose1 = *multibody.link(last_id).unwrap().local_to_world();
        let jacobian1 = multibody.body_jacobian(last_id);
        assert_relative_eq!(link_pose1, link_pose2, epsilon = 1.0e-5);
        assert_relative_eq!(jacobian1, &jacobian2, epsilon = 1.0e-5);
    }
}
