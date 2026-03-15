use crate::dynamics::{ImpulseJointSet, MultibodyJointSet, RigidBodyColliders, RigidBodyHandle};
use crate::geometry::{ColliderSet, NarrowPhase};

// Read all the contacts and push objects touching this rigid-body.
#[inline]
pub(super) fn push_contacting_bodies(
    rb_colliders: &RigidBodyColliders,
    colliders: &ColliderSet,
    narrow_phase: &NarrowPhase,
    stack: &mut Vec<RigidBodyHandle>,
) {
    for collider_handle in &rb_colliders.0 {
        for inter in narrow_phase.contact_pairs_with(*collider_handle) {
            if inter.has_any_active_contact() {
                let other = crate::utils::select_other(
                    (inter.collider1, inter.collider2),
                    *collider_handle,
                );
                if let Some(other_body) = colliders[other].parent {
                    stack.push(other_body.handle);
                }
            }
        }
    }
}

pub(super) fn push_linked_bodies(
    impulse_joints: &ImpulseJointSet,
    multibody_joints: &MultibodyJointSet,
    handle: RigidBodyHandle,
    stack: &mut Vec<RigidBodyHandle>,
) {
    for inter in impulse_joints.attached_enabled_joints(handle) {
        let other = crate::utils::select_other((inter.0, inter.1), handle);
        stack.push(other);
    }

    for other in multibody_joints.bodies_attached_with_enabled_joint(handle) {
        stack.push(other);
    }
}
