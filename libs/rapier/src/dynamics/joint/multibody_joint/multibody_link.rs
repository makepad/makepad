use std::ops::{Deref, DerefMut};

use crate::dynamics::{MultibodyJoint, RigidBodyHandle};
use crate::math::{Pose, Real, Vector};
use crate::prelude::RigidBodyVelocity;

/// One link of a multibody.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug)]
pub struct MultibodyLink {
    // FIXME: make all those private.
    pub(crate) internal_id: usize,
    pub(crate) assembly_id: usize,

    pub(crate) parent_internal_id: usize,
    pub(crate) rigid_body: RigidBodyHandle,

    /*
     * Change at each time step.
     */
    /// The multibody joint of this link.
    pub joint: MultibodyJoint,
    // TODO: should this be removed in favor of the rigid-body position?
    pub(crate) local_to_world: Pose,
    pub(crate) local_to_parent: Pose,
    pub(crate) shift02: Vector,
    pub(crate) shift23: Vector,

    /// The velocity added by the joint, in world-space.
    pub(crate) joint_velocity: RigidBodyVelocity<Real>,
}

impl MultibodyLink {
    /// Creates a new multibody link.
    pub fn new(
        rigid_body: RigidBodyHandle,
        internal_id: usize,
        assembly_id: usize,
        parent_internal_id: usize,
        joint: MultibodyJoint,
        local_to_world: Pose,
        local_to_parent: Pose,
    ) -> Self {
        let joint_velocity = RigidBodyVelocity::zero();

        MultibodyLink {
            internal_id,
            assembly_id,
            parent_internal_id,
            joint,
            local_to_world,
            local_to_parent,
            shift02: Vector::ZERO,
            shift23: Vector::ZERO,
            joint_velocity,
            rigid_body,
        }
    }

    /// The multibody joint of this link.
    pub fn joint(&self) -> &MultibodyJoint {
        &self.joint
    }

    /// The handle of the rigid-body of this link.
    pub fn rigid_body_handle(&self) -> RigidBodyHandle {
        self.rigid_body
    }

    /// Checks if this link is the root of the multibody.
    #[inline]
    pub fn is_root(&self) -> bool {
        self.internal_id == 0
    }

    /// The handle of this multibody link.
    #[inline]
    pub fn link_id(&self) -> usize {
        self.internal_id
    }

    /// The handle of the parent link.
    #[inline]
    pub fn parent_id(&self) -> Option<usize> {
        if self.internal_id != 0 {
            Some(self.parent_internal_id)
        } else {
            None
        }
    }

    /// The world-space transform of the rigid-body attached to this link.
    #[inline]
    pub fn local_to_world(&self) -> &Pose {
        &self.local_to_world
    }

    /// The position of the rigid-body attached to this link relative to its parent.
    #[inline]
    pub fn local_to_parent(&self) -> &Pose {
        &self.local_to_parent
    }
}

// FIXME: keep this even if we already have the Index2 traits?
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
pub(crate) struct MultibodyLinkVec(pub Vec<MultibodyLink>);

impl MultibodyLinkVec {
    #[inline]
    pub fn get_mut_with_parent(&mut self, i: usize) -> (&mut MultibodyLink, &MultibodyLink) {
        let parent_id = self[i].parent_internal_id;

        assert!(
            parent_id != i,
            "Internal error: circular rigid body dependency."
        );
        assert!(parent_id < self.len(), "Invalid parent index.");

        unsafe {
            let rb = &mut *(self.get_unchecked_mut(i) as *mut _);
            let parent_rb = &*(self.get_unchecked(parent_id) as *const _);
            (rb, parent_rb)
        }
    }
}

impl Deref for MultibodyLinkVec {
    type Target = Vec<MultibodyLink>;

    #[inline]
    fn deref(&self) -> &Vec<MultibodyLink> {
        let MultibodyLinkVec(ref me) = *self;
        me
    }
}

impl DerefMut for MultibodyLinkVec {
    #[inline]
    fn deref_mut(&mut self) -> &mut Vec<MultibodyLink> {
        let MultibodyLinkVec(ref mut me) = *self;
        me
    }
}
