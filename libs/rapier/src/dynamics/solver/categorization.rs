use crate::dynamics::{JointGraphEdge, JointIndex, MultibodyJointSet, RigidBodySet};
use crate::geometry::{ContactManifold, ContactManifoldIndex};

pub(crate) fn categorize_contacts(
    _bodies: &RigidBodySet, // Unused but useful to simplify the parallel code.
    multibody_joints: &MultibodyJointSet,
    manifolds: &[&mut ContactManifold],
    manifold_indices: &[ContactManifoldIndex],
    out_two_body: &mut Vec<ContactManifoldIndex>,
    out_generic_two_body: &mut Vec<ContactManifoldIndex>,
) {
    for manifold_i in manifold_indices {
        let manifold = &manifolds[*manifold_i];

        if manifold
            .data
            .rigid_body1
            .and_then(|h| multibody_joints.rigid_body_link(h))
            .is_some()
            || manifold
                .data
                .rigid_body2
                .and_then(|h| multibody_joints.rigid_body_link(h))
                .is_some()
        {
            out_generic_two_body.push(*manifold_i);
        } else {
            out_two_body.push(*manifold_i)
        }
    }
}

pub(crate) fn categorize_joints(
    multibody_joints: &MultibodyJointSet,
    impulse_joints: &[JointGraphEdge],
    joint_indices: &[JointIndex],
    two_body_joints: &mut Vec<JointIndex>,
    generic_two_body_joints: &mut Vec<JointIndex>,
) {
    for joint_i in joint_indices {
        let joint = &impulse_joints[*joint_i].weight;

        if multibody_joints.rigid_body_link(joint.body1).is_some()
            || multibody_joints.rigid_body_link(joint.body2).is_some()
        {
            generic_two_body_joints.push(*joint_i);
        } else {
            two_body_joints.push(*joint_i);
        }
    }
}
