use crate::dynamics::integration_parameters::SpringCoefficients;
use crate::dynamics::joint::{GenericJoint, GenericJointBuilder, JointAxesMask};
use crate::dynamics::{JointAxis, MotorModel};
use crate::math::{Real, Vector};

use super::JointMotor;

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(transparent)]
/// A distance-limiting joint (like a rope or cable connecting two objects).
///
/// Rope joints keep two bodies from getting too far apart, but allow them to get closer.
/// They only apply force when stretched to their maximum length. Use for:
/// - Ropes and chains
/// - Cables and tethers
/// - Leashes
/// - Grappling hooks
/// - Maximum distance constraints
///
/// Unlike spring joints, rope joints are inelastic - they don't bounce or stretch smoothly,
/// they just enforce a hard maximum distance.
pub struct RopeJoint {
    /// The underlying joint data.
    pub data: GenericJoint,
}

impl RopeJoint {
    /// Creates a new rope joint limiting the max distance between two bodies.
    ///
    /// The `max_dist` must be strictly greater than 0.0.
    pub fn new(max_dist: Real) -> Self {
        let data = GenericJointBuilder::new(JointAxesMask::empty())
            .coupled_axes(JointAxesMask::LIN_AXES)
            .build();
        let mut result = Self { data };
        result.set_max_distance(max_dist);
        result
    }

    /// The underlying generic joint.
    pub fn data(&self) -> &GenericJoint {
        &self.data
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

    /// The motor affecting the joint’s translational degree of freedom.
    #[must_use]
    pub fn motor(&self, axis: JointAxis) -> Option<&JointMotor> {
        self.data.motor(axis)
    }

    /// Set the spring-like model used by the motor to reach the desired target velocity and position.
    pub fn set_motor_model(&mut self, model: MotorModel) -> &mut Self {
        self.data.set_motor_model(JointAxis::LinX, model);
        self
    }

    /// Sets the target velocity this motor needs to reach.
    pub fn set_motor_velocity(&mut self, target_vel: Real, factor: Real) -> &mut Self {
        self.data
            .set_motor_velocity(JointAxis::LinX, target_vel, factor);
        self
    }

    /// Sets the target angle this motor needs to reach.
    pub fn set_motor_position(
        &mut self,
        target_pos: Real,
        stiffness: Real,
        damping: Real,
    ) -> &mut Self {
        self.data
            .set_motor_position(JointAxis::LinX, target_pos, stiffness, damping);
        self
    }

    /// Configure both the target angle and target velocity of the motor.
    pub fn set_motor(
        &mut self,
        target_pos: Real,
        target_vel: Real,
        stiffness: Real,
        damping: Real,
    ) -> &mut Self {
        self.data
            .set_motor(JointAxis::LinX, target_pos, target_vel, stiffness, damping);
        self
    }

    /// Sets the maximum force the motor can deliver.
    pub fn set_motor_max_force(&mut self, max_force: Real) -> &mut Self {
        self.data.set_motor_max_force(JointAxis::LinX, max_force);
        self
    }

    /// The maximum rope length (distance between anchor points).
    ///
    /// Bodies can get closer but not farther than this distance.
    #[must_use]
    pub fn max_distance(&self) -> Real {
        self.data
            .limits(JointAxis::LinX)
            .map(|l| l.max)
            .unwrap_or(Real::MAX)
    }

    /// Changes the maximum rope length.
    ///
    /// Must be greater than 0.0. Bodies will be pulled together if farther apart.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # use rapier3d::dynamics::RopeJoint;
    /// # let mut rope_joint = RopeJoint::new(5.0);
    /// rope_joint.set_max_distance(10.0);  // Max 10 units apart
    /// ```
    pub fn set_max_distance(&mut self, max_dist: Real) -> &mut Self {
        self.data.set_limits(JointAxis::LinX, [0.0, max_dist]);
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

impl From<RopeJoint> for GenericJoint {
    fn from(val: RopeJoint) -> GenericJoint {
        val.data
    }
}

/// Create rope joints using the builder pattern.
///
/// A rope joint, limits the maximum distance between two bodies.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RopeJointBuilder(pub RopeJoint);

impl RopeJointBuilder {
    /// Creates a new builder for rope joints.
    pub fn new(max_dist: Real) -> Self {
        Self(RopeJoint::new(max_dist))
    }

    /// Sets whether contacts between the attached rigid-bodies are enabled.
    #[must_use]
    pub fn contacts_enabled(mut self, enabled: bool) -> Self {
        self.0.set_contacts_enabled(enabled);
        self
    }

    /// Sets the joint’s anchor, expressed in the local-space of the first rigid-body.
    #[must_use]
    pub fn local_anchor1(mut self, anchor1: Vector) -> Self {
        self.0.set_local_anchor1(anchor1);
        self
    }

    /// Sets the joint’s anchor, expressed in the local-space of the second rigid-body.
    #[must_use]
    pub fn local_anchor2(mut self, anchor2: Vector) -> Self {
        self.0.set_local_anchor2(anchor2);
        self
    }

    /// Set the spring-like model used by the motor to reach the desired target velocity and position.
    #[must_use]
    pub fn motor_model(mut self, model: MotorModel) -> Self {
        self.0.set_motor_model(model);
        self
    }

    /// Sets the target velocity this motor needs to reach.
    #[must_use]
    pub fn motor_velocity(mut self, target_vel: Real, factor: Real) -> Self {
        self.0.set_motor_velocity(target_vel, factor);
        self
    }

    /// Sets the target angle this motor needs to reach.
    #[must_use]
    pub fn motor_position(mut self, target_pos: Real, stiffness: Real, damping: Real) -> Self {
        self.0.set_motor_position(target_pos, stiffness, damping);
        self
    }

    /// Configure both the target angle and target velocity of the motor.
    #[must_use]
    pub fn set_motor(
        mut self,
        target_pos: Real,
        target_vel: Real,
        stiffness: Real,
        damping: Real,
    ) -> Self {
        self.0.set_motor(target_pos, target_vel, stiffness, damping);
        self
    }

    /// Sets the maximum force the motor can deliver.
    #[must_use]
    pub fn motor_max_force(mut self, max_force: Real) -> Self {
        self.0.set_motor_max_force(max_force);
        self
    }

    /// Sets the maximum allowed distance between the attached bodies.
    ///
    /// The `max_dist` must be strictly greater than 0.0.
    #[must_use]
    pub fn max_distance(mut self, max_dist: Real) -> Self {
        self.0.set_max_distance(max_dist);
        self
    }

    /// Sets the softness of this joint’s locked degrees of freedom.
    #[must_use]
    pub fn softness(mut self, softness: SpringCoefficients<Real>) -> Self {
        self.0.data.softness = softness;
        self
    }

    /// Builds the rope joint.
    #[must_use]
    pub fn build(self) -> RopeJoint {
        self.0
    }
}

impl From<RopeJointBuilder> for GenericJoint {
    fn from(val: RopeJointBuilder) -> GenericJoint {
        val.0.into()
    }
}
