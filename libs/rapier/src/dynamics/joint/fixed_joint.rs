use crate::dynamics::integration_parameters::SpringCoefficients;
use crate::dynamics::{GenericJoint, GenericJointBuilder, JointAxesMask};
use crate::math::{Pose, Real, Vector};

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(transparent)]
/// A joint that rigidly connects two bodies together (like welding them).
///
/// Fixed joints lock all relative motion - the two bodies move as if they were a single
/// solid object. Use for:
/// - Permanently attaching objects (gluing, welding)
/// - Composite objects made of multiple bodies
/// - Connecting parts of a structure
///
/// Unlike simply using one body with multiple colliders, fixed joints let you attach
/// bodies that were created separately, and you can break the connection later if needed.
pub struct FixedJoint {
    /// The underlying joint data.
    pub data: GenericJoint,
}

impl Default for FixedJoint {
    fn default() -> Self {
        FixedJoint::new()
    }
}

impl FixedJoint {
    /// Creates a new fixed joint.
    #[must_use]
    pub fn new() -> Self {
        let data = GenericJointBuilder::new(JointAxesMask::LOCKED_FIXED_AXES).build();
        Self { data }
    }

    /// Are contacts between the attached rigid-bodies enabled?
    pub fn contacts_enabled(&self) -> bool {
        self.data.contacts_enabled
    }

    /// Sets whether contacts between the attached rigid-bodies are enabled.
    pub fn set_contacts_enabled(&mut self, enabled: bool) -> &mut Self {
        self.data.set_contacts_enabled(enabled);
        self
    }

    /// The joint’s frame, expressed in the first rigid-body’s local-space.
    #[must_use]
    pub fn local_frame1(&self) -> &Pose {
        &self.data.local_frame1
    }

    /// Sets the joint’s frame, expressed in the first rigid-body’s local-space.
    pub fn set_local_frame1(&mut self, local_frame: Pose) -> &mut Self {
        self.data.set_local_frame1(local_frame);
        self
    }

    /// The joint’s frame, expressed in the second rigid-body’s local-space.
    #[must_use]
    pub fn local_frame2(&self) -> &Pose {
        &self.data.local_frame2
    }

    /// Sets joint’s frame, expressed in the second rigid-body’s local-space.
    pub fn set_local_frame2(&mut self, local_frame: Pose) -> &mut Self {
        self.data.set_local_frame2(local_frame);
        self
    }

    /// The joint’s anchor, expressed in the local-space of the first rigid-body.
    #[must_use]
    pub fn local_anchor1(&self) -> Vector {
        self.data.local_anchor1()
    }

    /// Sets the joint’s anchor, expressed in the local-space of the first rigid-body.
    pub fn set_local_anchor1(&mut self, anchor1: Vector) -> &mut Self {
        self.data.set_local_anchor1(anchor1);
        self
    }

    /// The joint’s anchor, expressed in the local-space of the second rigid-body.
    #[must_use]
    pub fn local_anchor2(&self) -> Vector {
        self.data.local_anchor2()
    }

    /// Sets the joint’s anchor, expressed in the local-space of the second rigid-body.
    pub fn set_local_anchor2(&mut self, anchor2: Vector) -> &mut Self {
        self.data.set_local_anchor2(anchor2);
        self
    }

    /// Gets the softness of this joint’s locked degrees of freedom.
    #[must_use]
    pub fn softness(&self) -> SpringCoefficients<Real> {
        self.data.softness
    }

    /// Sets the softness of this joint’s locked degrees of freedom.
    #[must_use]
    pub fn set_softness(&mut self, softness: SpringCoefficients<Real>) -> &mut Self {
        self.data.softness = softness;
        self
    }
}

impl From<FixedJoint> for GenericJoint {
    fn from(val: FixedJoint) -> GenericJoint {
        val.data
    }
}

/// Create fixed joints using the builder pattern.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub struct FixedJointBuilder(pub FixedJoint);

impl FixedJointBuilder {
    /// Creates a new builder for fixed joints.
    pub fn new() -> Self {
        Self(FixedJoint::new())
    }

    /// Sets whether contacts between the attached rigid-bodies are enabled.
    #[must_use]
    pub fn contacts_enabled(mut self, enabled: bool) -> Self {
        self.0.set_contacts_enabled(enabled);
        self
    }

    /// Sets the joint’s frame, expressed in the first rigid-body’s local-space.
    #[must_use]
    pub fn local_frame1(mut self, local_frame: Pose) -> Self {
        self.0.set_local_frame1(local_frame);
        self
    }

    /// Sets joint’s frame, expressed in the second rigid-body’s local-space.
    #[must_use]
    pub fn local_frame2(mut self, local_frame: Pose) -> Self {
        self.0.set_local_frame2(local_frame);
        self
    }

    /// Sets the joint’s anchor, expressed in the local-space of the first rigid-body.
    #[must_use]
    pub fn local_anchor1(mut self, anchor1: Vector) -> Self {
        self.0.set_local_anchor1(anchor1);
        self
    }

    /// Sets the joint's anchor, expressed in the local-space of the second rigid-body.
    #[must_use]
    pub fn local_anchor2(mut self, anchor2: Vector) -> Self {
        self.0.set_local_anchor2(anchor2);
        self
    }

    /// Sets the softness of this joint’s locked degrees of freedom.
    #[must_use]
    pub fn softness(mut self, softness: SpringCoefficients<Real>) -> Self {
        self.0.data.softness = softness;
        self
    }

    /// Build the fixed joint.
    #[must_use]
    pub fn build(self) -> FixedJoint {
        self.0
    }
}

impl From<FixedJointBuilder> for GenericJoint {
    fn from(val: FixedJointBuilder) -> GenericJoint {
        val.0.into()
    }
}
