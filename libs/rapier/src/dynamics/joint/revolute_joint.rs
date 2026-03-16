use crate::dynamics::integration_parameters::SpringCoefficients;
use crate::dynamics::joint::{GenericJoint, GenericJointBuilder, JointAxesMask};
use crate::dynamics::{JointAxis, JointLimits, JointMotor, MotorModel};
use crate::math::{Real, Rotation, Vector};

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(transparent)]
/// A hinge joint that allows rotation around one axis (like a door hinge or wheel axle).
///
/// Revolute joints lock all movement except rotation around a single axis. Use for:
/// - Door hinges
/// - Wheels and gears
/// - Joints in robotic arms
/// - Pendulums
/// - Any rotating connection
///
/// You can optionally add:
/// - **Limits**: Restrict rotation to a range (e.g., door that only opens 90°)
/// - **Motor**: Powered rotation with target velocity or position
///
/// In 2D there's only one rotation axis (Z). In 3D you specify which axis (X, Y, or Z).
pub struct RevoluteJoint {
    /// The underlying joint data.
    pub data: GenericJoint,
}

impl RevoluteJoint {
    /// Creates a new revolute joint allowing only relative rotations.
    #[cfg(feature = "dim2")]
    #[allow(clippy::new_without_default)] // For symmetry with 3D which can’t have a Default impl.
    pub fn new() -> Self {
        let data = GenericJointBuilder::new(JointAxesMask::LOCKED_REVOLUTE_AXES);
        Self { data: data.build() }
    }

    /// Creates a new revolute joint allowing only relative rotations along the specified axis.
    ///
    /// This axis is expressed in the local-space of both rigid-bodies.
    #[cfg(feature = "dim3")]
    pub fn new(axis: Vector) -> Self {
        let data = GenericJointBuilder::new(JointAxesMask::LOCKED_REVOLUTE_AXES)
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

    /// The angle along the free degree of freedom of this revolute joint in `[-π, π]`.
    ///
    /// # Parameters
    /// - `rb_rot1`: the rotation of the first rigid-body attached to this revolute joint.
    /// - `rb_rot2`: the rotation of the second rigid-body attached to this revolute joint.
    pub fn angle(&self, rb_rot1: &Rotation, rb_rot2: &Rotation) -> Real {
        let joint_rot1 = rb_rot1 * self.data.local_frame1.rotation;
        let joint_rot2 = rb_rot2 * self.data.local_frame2.rotation;
        let ang_err = joint_rot1.inverse() * joint_rot2;

        #[cfg(feature = "dim3")]
        if joint_rot1.dot(joint_rot2) < 0.0 {
            -ang_err.x.clamp(-1.0, 1.0).asin() * 2.0
        } else {
            ang_err.x.clamp(-1.0, 1.0).asin() * 2.0
        }

        #[cfg(feature = "dim2")]
        {
            ang_err.angle()
        }
    }

    /// The motor affecting the joint’s rotational degree of freedom.
    #[must_use]
    pub fn motor(&self) -> Option<&JointMotor> {
        self.data.motor(JointAxis::AngX)
    }

    /// Set the spring-like model used by the motor to reach the desired target velocity and position.
    pub fn set_motor_model(&mut self, model: MotorModel) -> &mut Self {
        self.data.set_motor_model(JointAxis::AngX, model);
        self
    }

    /// Sets the motor's target rotation speed.
    ///
    /// Makes the joint spin at a desired velocity (like a powered motor or wheel).
    ///
    /// # Parameters
    /// * `target_vel` - Desired angular velocity in radians/second
    /// * `factor` - Motor strength (higher = stronger, approaches target faster)
    pub fn set_motor_velocity(&mut self, target_vel: Real, factor: Real) -> &mut Self {
        self.data
            .set_motor_velocity(JointAxis::AngX, target_vel, factor);
        self
    }

    /// Sets the motor's target angle (position control).
    ///
    /// Makes the joint rotate toward a specific angle using spring-like behavior.
    ///
    /// # Parameters
    /// * `target_pos` - Desired angle in radians
    /// * `stiffness` - How strongly to pull toward target (spring constant)
    /// * `damping` - Resistance to motion (higher = less oscillation)
    pub fn set_motor_position(
        &mut self,
        target_pos: Real,
        stiffness: Real,
        damping: Real,
    ) -> &mut Self {
        self.data
            .set_motor_position(JointAxis::AngX, target_pos, stiffness, damping);
        self
    }

    /// Configures both target angle and target velocity for the motor.
    ///
    /// Combines position and velocity control for precise motor behavior.
    pub fn set_motor(
        &mut self,
        target_pos: Real,
        target_vel: Real,
        stiffness: Real,
        damping: Real,
    ) -> &mut Self {
        self.data
            .set_motor(JointAxis::AngX, target_pos, target_vel, stiffness, damping);
        self
    }

    /// Sets the maximum torque the motor can apply.
    ///
    /// Limits how strong the motor is. Without this, motors can apply infinite force.
    pub fn set_motor_max_force(&mut self, max_force: Real) -> &mut Self {
        self.data.set_motor_max_force(JointAxis::AngX, max_force);
        self
    }

    /// The rotation limits of this joint, if any.
    ///
    /// Returns `None` if no limits are set (unlimited rotation).
    #[must_use]
    pub fn limits(&self) -> Option<&JointLimits<Real>> {
        self.data.limits(JointAxis::AngX)
    }

    /// Restricts rotation to a specific angle range.
    ///
    /// # Parameters
    /// * `limits` - `[min_angle, max_angle]` in radians
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # use rapier3d::dynamics::RevoluteJoint;
    /// # let mut joint = RevoluteJoint::new(Vector::Y);
    /// // Door that opens 0° to 90°
    /// joint.set_limits([0.0, std::f32::consts::PI / 2.0]);
    /// ```
    pub fn set_limits(&mut self, limits: [Real; 2]) -> &mut Self {
        self.data.set_limits(JointAxis::AngX, limits);
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

impl From<RevoluteJoint> for GenericJoint {
    fn from(val: RevoluteJoint) -> GenericJoint {
        val.data
    }
}

/// Create revolute joints using the builder pattern.
///
/// A revolute joint locks all relative motion except for rotations along the joint’s principal axis.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RevoluteJointBuilder(pub RevoluteJoint);

impl RevoluteJointBuilder {
    /// Creates a new revolute joint builder.
    #[cfg(feature = "dim2")]
    #[allow(clippy::new_without_default)] // For symmetry with 3D which can’t have a Default impl.
    pub fn new() -> Self {
        Self(RevoluteJoint::new())
    }

    /// Creates a new revolute joint builder, allowing only relative rotations along the specified axis.
    ///
    /// This axis is expressed in the local-space of both rigid-bodies.
    #[cfg(feature = "dim3")]
    pub fn new(axis: Vector) -> Self {
        Self(RevoluteJoint::new(axis))
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
    pub fn motor(
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

    /// Sets the `[min,max]` limit angles attached bodies can rotate along the joint's principal axis.
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

    /// Builds the revolute joint.
    #[must_use]
    pub fn build(self) -> RevoluteJoint {
        self.0
    }
}

impl From<RevoluteJointBuilder> for GenericJoint {
    fn from(val: RevoluteJointBuilder) -> GenericJoint {
        val.0.into()
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn test_revolute_joint_angle() {
        #[cfg(feature = "dim3")]
        use crate::math::{AngVector, Vector};
        use crate::math::{Real, rotation_from_angle};
        use crate::na::RealField;

        #[cfg(feature = "dim2")]
        let revolute = super::RevoluteJointBuilder::new().build();
        #[cfg(feature = "dim2")]
        let rot1 = rotation_from_angle(1.0);
        #[cfg(feature = "dim3")]
        let revolute = super::RevoluteJointBuilder::new(Vector::Y).build();
        #[cfg(feature = "dim3")]
        let rot1 = rotation_from_angle(AngVector::new(0.0, 1.0, 0.0));

        let steps = 100;

        // The -pi and pi values will be checked later.
        for i in 1..steps {
            let delta = -Real::pi() + i as Real * Real::two_pi() / steps as Real;
            #[cfg(feature = "dim2")]
            let rot2 = rotation_from_angle(1.0 + delta);
            #[cfg(feature = "dim3")]
            let rot2 = rotation_from_angle(AngVector::new(0.0, 1.0 + delta, 0.0));
            approx::assert_relative_eq!(revolute.angle(&rot1, &rot2), delta, epsilon = 1.0e-5);
        }

        // Check the special case for -pi and pi that may return an angle with a flipped sign
        // (because they are equivalent).
        for delta in [-Real::pi(), Real::pi()] {
            #[cfg(feature = "dim2")]
            let rot2 = rotation_from_angle(1.0 + delta);
            #[cfg(feature = "dim3")]
            let rot2 = rotation_from_angle(AngVector::new(0.0, 1.0 + delta, 0.0));
            approx::assert_relative_eq!(
                revolute.angle(&rot1, &rot2).abs(),
                delta.abs(),
                epsilon = 1.0e-2
            );
        }
    }
}
