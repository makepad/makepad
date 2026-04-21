use crate::dynamics::{GenericJoint, ImpulseJointHandle, RigidBodyHandle};
use crate::math::SpatialVector;

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
/// An impulse-based joint attached to two bodies.
pub struct ImpulseJoint {
    /// Handle to the first body attached to this joint.
    pub body1: RigidBodyHandle,
    /// Handle to the second body attached to this joint.
    pub body2: RigidBodyHandle,

    /// The joint’s description.
    pub data: GenericJoint,

    /// The impulses applied by this joint.
    pub impulses: SpatialVector,

    // A joint needs to know its handle to simplify its removal.
    pub(crate) handle: ImpulseJointHandle,
}
