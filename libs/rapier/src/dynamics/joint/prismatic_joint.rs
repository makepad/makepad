use crate::dynamics::integration_parameters::SpringCoefficients;
use crate::dynamics::joint::{GenericJoint, GenericJointBuilder, JointAxesMask};
use crate::dynamics::{JointAxis, MotorModel};
use crate::math::{Real, Vector};

use super::{JointLimits, JointMotor};

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(transparent)]
/// A sliding joint that allows movement along one axis only (like a piston or sliding door).
///
/// Prismatic joints lock all motion except sliding along a single axis. Use for:
/// - Pistons and hydraulics
/// - Sliding doors and drawers
/// - Elevator platforms
/// - Linear actuators
/// - Telescoping mechanisms
///
/// You can optionally add:
/// - **Limits**: Restrict sliding distance (min/max positions)
/// - **Motor**: Powered sliding with target velocity or position
///
/// The axis is specified when creating the joint and is expressed in each body's local space.
pub struct PrismaticJoint {
    /// The underlying joint data.
    pub data: GenericJoint,
}

impl PrismaticJoint {
    /// Creates a new prismatic joint allowing only relative translations along the specified axis.
    ///
    /// This axis is expressed in the local-space of both rigid-bodies.
    pub fn new(axis: Vector) -> Self {
        let data = GenericJointBuilder::new(JointAxesMask::LOCKED_PRISMATIC_AXES)
            .local_axis1(axis)
            .local_axis2(axis)
            .build();
        Self { data }
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

    /// The principal axis of the joint, expressed in the local-space of the first rigid-body.
    #[must_use]
    pub fn local_axis1(&self) -> Vector {
        self.data.local_axis1()
    }

    /// Sets the principal axis of the joint, expressed in the local-space of the first rigid-body.
    pub fn set_local_axis1(&mut self, axis1: Vector) -> &mut Self {
        self.data.set_local_axis1(axis1);
        self
    }

    /// The principal axis of the joint, expressed in the local-space of the second rigid-body.
    #[must_use]
    pub fn local_axis2(&self) -> Vector {
        self.data.local_axis2()
    }

    /// Sets the principal axis of the joint, expressed in the local-space of the second rigid-body.
    pub fn set_local_axis2(&mut self, axis2: Vector) -> &mut Self {
        self.data.set_local_axis2(axis2);
        self
    }

    /// The motor affecting the joint’s translational degree of freedom.
    #[must_use]
    pub fn motor(&self) -> Option<&JointMotor> {
        self.data.motor(JointAxis::LinX)
    }

    /// Set the spring-like model used by the motor to reach the desired target velocity and position.
    pub fn set_motor_model(&mut self, model: MotorModel) -> &mut Self {
        self.data.set_motor_model(JointAxis::LinX, model);
        self
    }

    /// Sets the motor's target sliding speed.
    ///
    /// Makes the joint slide at a desired velocity (like a powered piston or conveyor).
    ///
    /// # Parameters
    /// * `target_vel` - Desired velocity in units/second
    /// * `factor` - Motor strength
    pub fn set_motor_velocity(&mut self, target_vel: Real, factor: Real) -> &mut Self {
        self.data
            .set_motor_velocity(JointAxis::LinX, target_vel, factor);
        self
    }

    /// Sets the motor's target position along the sliding axis.
    ///
    /// Makes the joint slide toward a specific position using spring-like behavior.
    ///
    /// # Parameters
    /// * `target_pos` - Desired position along the axis
    /// * `stiffness` - Spring constant
    /// * `damping` - Resistance to motion
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

    /// Configures both target position and target velocity for the motor.
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

    /// The limit distance attached bodies can translate along the joint’s principal axis.
    #[must_use]
    pub fn limits(&self) -> Option<&JointLimits<Real>> {
        self.data.limits(JointAxis::LinX)
    }

    /// Sets the `[min,max]` limit distances attached bodies can translate along the joint’s principal axis.
    pub fn set_limits(&mut self, limits: [Real; 2]) -> &mut Self {
        self.data.set_limits(JointAxis::LinX, limits);
        self
    }

    /// Gets the softness of this joint’s locked degrees of freedom.
    #[must_use]
    pub fn softness(&self) -> SpringCoefficients<Real> {
        self.data.softness
    }

    /// Sets the softness of this joint.
    #[must_use]
    pub fn set_softness(&mut self, softness: SpringCoefficients<Real>) -> &mut Self {
        self.data.softness = softness;
        self
    }
}

impl From<PrismaticJoint> for GenericJoint {
    fn from(val: PrismaticJoint) -> GenericJoint {
        val.data
    }
}

/// Create prismatic joints using the builder pattern.
///
/// A prismatic joint locks all relative motion except for translations along the joint’s principal axis.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PrismaticJointBuilder(pub PrismaticJoint);

impl PrismaticJointBuilder {
    /// Creates a new builder for prismatic joints.
    ///
    /// This axis is expressed in the local-space of both rigid-bodies.
    pub fn new(axis: Vector) -> Self {
        Self(PrismaticJoint::new(axis))
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

    /// Sets the principal axis of the joint, expressed in the local-space of the first rigid-body.
    #[must_use]
    pub fn local_axis1(mut self, axis1: Vector) -> Self {
        self.0.set_local_axis1(axis1);
        self
    }

    /// Sets the principal axis of the joint, expressed in the local-space of the second rigid-body.
    #[must_use]
    pub fn local_axis2(mut self, axis2: Vector) -> Self {
        self.0.set_local_axis2(axis2);
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

    /// Sets the `[min,max]` limit distances attached bodies can translate along the joint's principal axis.
    #[must_use]
    pub fn limits(mut self, limits: [Real; 2]) -> Self {
        self.0.set_limits(limits);
        self
    }

    /// Sets the softness of this joint’s locked degrees of freedom.
    #[must_use]
    pub fn softness(mut self, softness: SpringCoefficients<Real>) -> Self {
        self.0.data.softness = softness;
        self
    }

    /// Builds the prismatic joint.
    #[must_use]
    pub fn build(self) -> PrismaticJoint {
        self.0
    }
}

impl From<PrismaticJointBuilder> for GenericJoint {
    fn from(val: PrismaticJointBuilder) -> GenericJoint {
        val.0.into()
    }
}
