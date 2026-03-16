#![allow(clippy::bad_bit_mask)] // Clippy will complain about the bitmasks due to JointAxesMask::FREE_FIXED_AXES being 0.
#![allow(clippy::unnecessary_cast)] // Casts are needed for switching between f32/f64.

use crate::dynamics::integration_parameters::SpringCoefficients;
use crate::dynamics::solver::MotorParameters;
use crate::dynamics::{
    FixedJoint, MotorModel, PrismaticJoint, RevoluteJoint, RigidBody, RopeJoint,
};
use crate::math::{Pose, Real, Rotation, SPATIAL_DIM, Vector};
use crate::utils::{OrthonormalBasis, SimdRealCopy};
use parry::math::Matrix;

#[cfg(feature = "dim3")]
use crate::dynamics::SphericalJoint;

#[cfg(feature = "dim3")]
bitflags::bitflags! {
    /// A bit mask identifying multiple degrees of freedom of a joint.
    #[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub struct JointAxesMask: u8 {
        /// The linear (translational) degree of freedom along the local X axis of a joint.
        const LIN_X = 1 << 0;
        /// The linear (translational) degree of freedom along the local Y axis of a joint.
        const LIN_Y = 1 << 1;
        /// The linear (translational) degree of freedom along the local Z axis of a joint.
        const LIN_Z = 1 << 2;
        /// The angular degree of freedom along the local X axis of a joint.
        const ANG_X = 1 << 3;
        /// The angular degree of freedom along the local Y axis of a joint.
        const ANG_Y = 1 << 4;
        /// The angular degree of freedom along the local Z axis of a joint.
        const ANG_Z = 1 << 5;
        /// The set of degrees of freedom locked by a revolute joint.
        const LOCKED_REVOLUTE_AXES = Self::LIN_X.bits() | Self::LIN_Y.bits() | Self::LIN_Z.bits() | Self::ANG_Y.bits() | Self::ANG_Z.bits();
        /// The set of degrees of freedom locked by a prismatic joint.
        const LOCKED_PRISMATIC_AXES = Self::LIN_Y.bits() | Self::LIN_Z.bits() | Self::ANG_X.bits() | Self::ANG_Y.bits() | Self::ANG_Z.bits();
        /// The set of degrees of freedom locked by a fixed joint.
        const LOCKED_FIXED_AXES = Self::LIN_X.bits() | Self::LIN_Y.bits() | Self::LIN_Z.bits() | Self::ANG_X.bits() | Self::ANG_Y.bits() | Self::ANG_Z.bits();
        /// The set of degrees of freedom locked by a spherical joint.
        const LOCKED_SPHERICAL_AXES = Self::LIN_X.bits() | Self::LIN_Y.bits() | Self::LIN_Z.bits();
        /// The set of degrees of freedom left free by a revolute joint.
        const FREE_REVOLUTE_AXES = Self::ANG_X.bits();
        /// The set of degrees of freedom left free by a prismatic joint.
        const FREE_PRISMATIC_AXES = Self::LIN_X.bits();
        /// The set of degrees of freedom left free by a fixed joint.
        const FREE_FIXED_AXES = 0;
        /// The set of degrees of freedom left free by a spherical joint.
        const FREE_SPHERICAL_AXES = Self::ANG_X.bits() | Self::ANG_Y.bits() | Self::ANG_Z.bits();
        /// The set of all translational degrees of freedom.
        const LIN_AXES = Self::LIN_X.bits() | Self::LIN_Y.bits() | Self::LIN_Z.bits();
        /// The set of all angular degrees of freedom.
        const ANG_AXES = Self::ANG_X.bits() | Self::ANG_Y.bits() | Self::ANG_Z.bits();
    }
}

#[cfg(feature = "dim2")]
bitflags::bitflags! {
    /// A bit mask identifying multiple degrees of freedom of a joint.
    #[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub struct JointAxesMask: u8 {
        /// The linear (translational) degree of freedom along the local X axis of a joint.
        const LIN_X = 1 << 0;
        /// The linear (translational) degree of freedom along the local Y axis of a joint.
        const LIN_Y = 1 << 1;
        /// The angular degree of freedom of a joint.
        const ANG_X = 1 << 2;
        /// The set of degrees of freedom locked by a revolute joint.
        const LOCKED_REVOLUTE_AXES = Self::LIN_X.bits() | Self::LIN_Y.bits();
        /// The set of degrees of freedom locked by a prismatic joint.
        const LOCKED_PRISMATIC_AXES = Self::LIN_Y.bits() | Self::ANG_X.bits();
        /// The set of degrees of freedom locked by a pin slot joint.
        const LOCKED_PIN_SLOT_AXES = Self::LIN_Y.bits();
        /// The set of degrees of freedom locked by a fixed joint.
        const LOCKED_FIXED_AXES = Self::LIN_X.bits() | Self::LIN_Y.bits() | Self::ANG_X.bits();
        /// The set of degrees of freedom left free by a revolute joint.
        const FREE_REVOLUTE_AXES = Self::ANG_X.bits();
        /// The set of degrees of freedom left free by a prismatic joint.
        const FREE_PRISMATIC_AXES = Self::LIN_X.bits();
        /// The set of degrees of freedom left free by a fixed joint.
        const FREE_FIXED_AXES = 0;
        /// The set of all translational degrees of freedom.
        const LIN_AXES = Self::LIN_X.bits() | Self::LIN_Y.bits();
        /// The set of all angular degrees of freedom.
        const ANG_AXES = Self::ANG_X.bits();
    }
}

impl Default for JointAxesMask {
    fn default() -> Self {
        Self::empty()
    }
}

/// Identifiers of degrees of freedoms of a joint.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum JointAxis {
    /// The linear (translational) degree of freedom along the joint’s local X axis.
    LinX = 0,
    /// The linear (translational) degree of freedom along the joint’s local Y axis.
    LinY,
    /// The linear (translational) degree of freedom along the joint’s local Z axis.
    #[cfg(feature = "dim3")]
    LinZ,
    /// The rotational degree of freedom along the joint’s local X axis.
    AngX,
    /// The rotational degree of freedom along the joint’s local Y axis.
    #[cfg(feature = "dim3")]
    AngY,
    /// The rotational degree of freedom along the joint’s local Z axis.
    #[cfg(feature = "dim3")]
    AngZ,
}

impl From<JointAxis> for JointAxesMask {
    fn from(axis: JointAxis) -> Self {
        JointAxesMask::from_bits(1 << axis as usize).unwrap()
    }
}

/// Limits that restrict a joint's range of motion along one axis.
///
/// Use to constrain how far a joint can move/rotate. Examples:
/// - Door that only opens 90°: revolute joint with limits `[0.0, PI/2.0]`
/// - Piston with 2-unit stroke: prismatic joint with limits `[0.0, 2.0]`
/// - Elbow that bends 0-150°: revolute joint with limits `[0.0, 5*PI/6]`
///
/// When a joint hits its limit, forces are applied to prevent further movement in that direction.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct JointLimits<N> {
    /// Minimum allowed value (angle for revolute, distance for prismatic).
    pub min: N,
    /// Maximum allowed value (angle for revolute, distance for prismatic).
    pub max: N,
    /// Internal: impulse being applied to enforce the limit.
    pub impulse: N,
}

impl<N: SimdRealCopy> Default for JointLimits<N> {
    fn default() -> Self {
        Self {
            min: -N::splat(Real::MAX),
            max: N::splat(Real::MAX),
            impulse: N::splat(0.0),
        }
    }
}

impl<N: SimdRealCopy> From<[N; 2]> for JointLimits<N> {
    fn from(value: [N; 2]) -> Self {
        Self {
            min: value[0],
            max: value[1],
            impulse: N::splat(0.0),
        }
    }
}

/// A powered motor that drives a joint toward a target position/velocity.
///
/// Motors add actuation to joints - they apply forces to make the joint move toward
/// a desired state. Think of them as servos, electric motors, or hydraulic actuators.
///
/// ## Two control modes
///
/// 1. **Velocity control**: Set `target_vel` to make the motor spin/slide at constant speed
/// 2. **Position control**: Set `target_pos` with `stiffness`/`damping` to reach a target angle/position
///
/// You can combine both for precise control.
///
/// ## Parameters
///
/// - `stiffness`: How strongly to pull toward target (spring constant)
/// - `damping`: Resistance to motion (prevents oscillation)
/// - `max_force`: Maximum force/torque the motor can apply
///
/// # Example
/// ```
/// # use rapier3d::prelude::*;
/// # use rapier3d::dynamics::{RevoluteJoint, PrismaticJoint};
/// # let mut revolute_joint = RevoluteJoint::new(Vector::X);
/// # let mut prismatic_joint = PrismaticJoint::new(Vector::X);
/// // Motor that spins a wheel at 10 rad/s
/// revolute_joint.set_motor_velocity(10.0, 0.8);
///
/// // Motor that moves to position 5.0
/// prismatic_joint.set_motor_position(5.0, 100.0, 10.0);  // stiffness=100, damping=10
/// ```
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct JointMotor {
    /// Target velocity (units/sec for prismatic, rad/sec for revolute).
    pub target_vel: Real,
    /// Target position (units for prismatic, radians for revolute).
    pub target_pos: Real,
    /// Spring constant - how strongly to pull toward target position.
    pub stiffness: Real,
    /// Damping coefficient - resistance to motion (prevents oscillation).
    pub damping: Real,
    /// Maximum force the motor can apply (Newtons for prismatic, Nm for revolute).
    pub max_force: Real,
    /// Internal: current impulse being applied.
    pub impulse: Real,
    /// Force-based or acceleration-based motor model.
    pub model: MotorModel,
}

impl Default for JointMotor {
    fn default() -> Self {
        Self {
            target_pos: 0.0,
            target_vel: 0.0,
            stiffness: 0.0,
            damping: 0.0,
            max_force: Real::MAX,
            impulse: 0.0,
            model: MotorModel::AccelerationBased,
        }
    }
}

impl JointMotor {
    pub(crate) fn motor_params(&self, dt: Real) -> MotorParameters<Real> {
        let (erp_inv_dt, cfm_coeff, cfm_gain) =
            self.model
                .combine_coefficients(dt, self.stiffness, self.damping);
        MotorParameters {
            erp_inv_dt,
            cfm_coeff,
            cfm_gain,
            // keep_lhs,
            target_pos: self.target_pos,
            target_vel: self.target_vel,
            max_impulse: self.max_force * dt,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
/// Enum indicating whether or not a joint is enabled.
pub enum JointEnabled {
    /// The joint is enabled.
    Enabled,
    /// The joint wasn’t disabled by the user explicitly but it is attached to
    /// a disabled rigid-body.
    DisabledByAttachedBody,
    /// The joint is disabled by the user explicitly.
    Disabled,
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
/// A generic joint.
pub struct GenericJoint {
    /// The joint’s frame, expressed in the first rigid-body’s local-space.
    pub local_frame1: Pose,
    /// The joint’s frame, expressed in the second rigid-body’s local-space.
    pub local_frame2: Pose,
    /// The degrees-of-freedoms locked by this joint.
    pub locked_axes: JointAxesMask,
    /// The degrees-of-freedoms limited by this joint.
    pub limit_axes: JointAxesMask,
    /// The degrees-of-freedoms motorised by this joint.
    pub motor_axes: JointAxesMask,
    /// The coupled degrees of freedom of this joint.
    ///
    /// Note that coupling degrees of freedoms (DoF) changes the interpretation of the coupled joint’s limits and motors.
    /// If multiple linear DoF are limited/motorized, only the limits/motor configuration for the first
    /// coupled linear DoF is applied to all coupled linear DoF. Similarly, if multiple angular DoF are limited/motorized
    /// only the limits/motor configuration for the first coupled angular DoF is applied to all coupled angular DoF.
    pub coupled_axes: JointAxesMask,
    /// The limits, along each degree of freedoms of this joint.
    ///
    /// Note that the limit must also be explicitly enabled by the `limit_axes` bitmask.
    /// For coupled degrees of freedoms (DoF), only the first linear (resp. angular) coupled DoF limit and `limit_axis`
    /// bitmask is applied to the coupled linear (resp. angular) axes.
    pub limits: [JointLimits<Real>; SPATIAL_DIM],
    /// The motors, along each degree of freedoms of this joint.
    ///
    /// Note that the motor must also be explicitly enabled by the `motor_axes` bitmask.
    /// For coupled degrees of freedoms (DoF), only the first linear (resp. angular) coupled DoF motor and `motor_axes`
    /// bitmask is applied to the coupled linear (resp. angular) axes.
    pub motors: [JointMotor; SPATIAL_DIM],
    /// The coefficients controlling the joint constraints’ softness.
    pub softness: SpringCoefficients<Real>,
    /// Are contacts between the attached rigid-bodies enabled?
    pub contacts_enabled: bool,
    /// Whether the joint is enabled.
    pub enabled: JointEnabled,
    /// User-defined data associated to this joint.
    pub user_data: u128,
}

impl Default for GenericJoint {
    fn default() -> Self {
        Self {
            local_frame1: Pose::IDENTITY,
            local_frame2: Pose::IDENTITY,
            locked_axes: JointAxesMask::empty(),
            limit_axes: JointAxesMask::empty(),
            motor_axes: JointAxesMask::empty(),
            coupled_axes: JointAxesMask::empty(),
            limits: [JointLimits::default(); SPATIAL_DIM],
            motors: [JointMotor::default(); SPATIAL_DIM],
            softness: SpringCoefficients::joint_defaults(),
            contacts_enabled: true,
            enabled: JointEnabled::Enabled,
            user_data: 0,
        }
    }
}

impl GenericJoint {
    /// Creates a new generic joint that locks the specified degrees of freedom.
    #[must_use]
    pub fn new(locked_axes: JointAxesMask) -> Self {
        *Self::default().lock_axes(locked_axes)
    }

    #[cfg(feature = "simd-is-enabled")]
    /// Can this joint use SIMD-accelerated constraint formulations?
    pub(crate) fn supports_simd_constraints(&self) -> bool {
        self.limit_axes.is_empty() && self.motor_axes.is_empty()
    }

    #[doc(hidden)]
    pub fn complete_ang_frame(axis: Vector) -> Rotation {
        let basis = axis.orthonormal_basis();

        #[cfg(feature = "dim2")]
        {
            let mat = Matrix::from_cols(axis, basis[0]);
            Rotation::from_matrix_unchecked(mat)
        }

        #[cfg(feature = "dim3")]
        {
            let mat = Matrix::from_cols(axis, basis[0], basis[1]);
            Rotation::from_mat3(&mat)
        }
    }

    /// Is this joint enabled?
    pub fn is_enabled(&self) -> bool {
        self.enabled == JointEnabled::Enabled
    }

    /// Set whether this joint is enabled or not.
    pub fn set_enabled(&mut self, enabled: bool) {
        match self.enabled {
            JointEnabled::Enabled | JointEnabled::DisabledByAttachedBody => {
                if !enabled {
                    self.enabled = JointEnabled::Disabled;
                }
            }
            JointEnabled::Disabled => {
                if enabled {
                    self.enabled = JointEnabled::Enabled;
                }
            }
        }
    }

    /// Add the specified axes to the set of axes locked by this joint.
    pub fn lock_axes(&mut self, axes: JointAxesMask) -> &mut Self {
        self.locked_axes |= axes;
        self
    }

    /// Sets the joint’s frame, expressed in the first rigid-body’s local-space.
    pub fn set_local_frame1(&mut self, local_frame: Pose) -> &mut Self {
        self.local_frame1 = local_frame;
        self
    }

    /// Sets the joint’s frame, expressed in the second rigid-body’s local-space.
    pub fn set_local_frame2(&mut self, local_frame: Pose) -> &mut Self {
        self.local_frame2 = local_frame;
        self
    }

    /// The principal (local X) axis of this joint, expressed in the first rigid-body’s local-space.
    #[must_use]
    pub fn local_axis1(&self) -> Vector {
        self.local_frame1 * Vector::X
    }

    /// Sets the principal (local X) axis of this joint, expressed in the first rigid-body’s local-space.
    pub fn set_local_axis1(&mut self, local_axis: Vector) -> &mut Self {
        self.local_frame1.rotation = Self::complete_ang_frame(local_axis);
        self
    }

    /// The principal (local X) axis of this joint, expressed in the second rigid-body’s local-space.
    #[must_use]
    pub fn local_axis2(&self) -> Vector {
        self.local_frame2 * Vector::X
    }

    /// Sets the principal (local X) axis of this joint, expressed in the second rigid-body’s local-space.
    pub fn set_local_axis2(&mut self, local_axis: Vector) -> &mut Self {
        self.local_frame2.rotation = Self::complete_ang_frame(local_axis);
        self
    }

    /// The anchor of this joint, expressed in the first rigid-body’s local-space.
    #[must_use]
    pub fn local_anchor1(&self) -> Vector {
        self.local_frame1.translation
    }

    /// Sets anchor of this joint, expressed in the first rigid-body's local-space.
    pub fn set_local_anchor1(&mut self, anchor1: Vector) -> &mut Self {
        self.local_frame1.translation = anchor1;
        self
    }

    /// The anchor of this joint, expressed in the second rigid-body's local-space.
    #[must_use]
    pub fn local_anchor2(&self) -> Vector {
        self.local_frame2.translation
    }

    /// Sets anchor of this joint, expressed in the second rigid-body's local-space.
    pub fn set_local_anchor2(&mut self, anchor2: Vector) -> &mut Self {
        self.local_frame2.translation = anchor2;
        self
    }

    /// Are contacts between the attached rigid-bodies enabled?
    pub fn contacts_enabled(&self) -> bool {
        self.contacts_enabled
    }

    /// Sets whether contacts between the attached rigid-bodies are enabled.
    pub fn set_contacts_enabled(&mut self, enabled: bool) -> &mut Self {
        self.contacts_enabled = enabled;
        self
    }

    /// Sets the spring coefficients controlling this joint constraint’s softness.
    #[must_use]
    pub fn set_softness(&mut self, softness: SpringCoefficients<Real>) -> &mut Self {
        self.softness = softness;
        self
    }

    /// The joint limits along the specified axis.
    #[must_use]
    pub fn limits(&self, axis: JointAxis) -> Option<&JointLimits<Real>> {
        let i = axis as usize;
        if self.limit_axes.contains(axis.into()) {
            Some(&self.limits[i])
        } else {
            None
        }
    }

    /// Sets the joint limits along the specified axis.
    pub fn set_limits(&mut self, axis: JointAxis, limits: [Real; 2]) -> &mut Self {
        let i = axis as usize;
        self.limit_axes |= axis.into();
        self.limits[i].min = limits[0];
        self.limits[i].max = limits[1];
        self
    }

    /// The spring-like motor model along the specified axis of this joint.
    #[must_use]
    pub fn motor_model(&self, axis: JointAxis) -> Option<MotorModel> {
        let i = axis as usize;
        if self.motor_axes.contains(axis.into()) {
            Some(self.motors[i].model)
        } else {
            None
        }
    }

    /// Set the spring-like model used by the motor to reach the desired target velocity and position.
    pub fn set_motor_model(&mut self, axis: JointAxis, model: MotorModel) -> &mut Self {
        self.motors[axis as usize].model = model;
        self
    }

    /// Sets the target velocity this motor needs to reach.
    pub fn set_motor_velocity(
        &mut self,
        axis: JointAxis,
        target_vel: Real,
        factor: Real,
    ) -> &mut Self {
        self.set_motor(
            axis,
            self.motors[axis as usize].target_pos,
            target_vel,
            0.0,
            factor,
        )
    }

    /// Sets the target angle this motor needs to reach.
    pub fn set_motor_position(
        &mut self,
        axis: JointAxis,
        target_pos: Real,
        stiffness: Real,
        damping: Real,
    ) -> &mut Self {
        self.set_motor(axis, target_pos, 0.0, stiffness, damping)
    }

    /// Sets the maximum force the motor can deliver along the specified axis.
    pub fn set_motor_max_force(&mut self, axis: JointAxis, max_force: Real) -> &mut Self {
        self.motors[axis as usize].max_force = max_force;
        self
    }

    /// The motor affecting the joint’s degree of freedom along the specified axis.
    #[must_use]
    pub fn motor(&self, axis: JointAxis) -> Option<&JointMotor> {
        let i = axis as usize;
        if self.motor_axes.contains(axis.into()) {
            Some(&self.motors[i])
        } else {
            None
        }
    }

    /// Configure both the target angle and target velocity of the motor.
    pub fn set_motor(
        &mut self,
        axis: JointAxis,
        target_pos: Real,
        target_vel: Real,
        stiffness: Real,
        damping: Real,
    ) -> &mut Self {
        self.motor_axes |= axis.into();
        let i = axis as usize;
        self.motors[i].target_vel = target_vel;
        self.motors[i].target_pos = target_pos;
        self.motors[i].stiffness = stiffness;
        self.motors[i].damping = damping;
        self
    }

    /// Flips the orientation of the joint, including limits and motors.
    pub fn flip(&mut self) {
        std::mem::swap(&mut self.local_frame1, &mut self.local_frame2);

        let coupled_bits = self.coupled_axes.bits();

        for dim in 0..SPATIAL_DIM {
            if coupled_bits & (1 << dim) == 0 {
                let limit = self.limits[dim];
                self.limits[dim].min = -limit.max;
                self.limits[dim].max = -limit.min;
            }

            self.motors[dim].target_vel = -self.motors[dim].target_vel;
            self.motors[dim].target_pos = -self.motors[dim].target_pos;
        }
    }

    pub(crate) fn transform_to_solver_body_space(&mut self, rb1: &RigidBody, rb2: &RigidBody) {
        if rb1.is_fixed() {
            self.local_frame1 = rb1.pos.position * self.local_frame1;
        } else {
            self.local_frame1.translation -= rb1.mprops.local_mprops.local_com;
        }

        if rb2.is_fixed() {
            self.local_frame2 = rb2.pos.position * self.local_frame2;
        } else {
            self.local_frame2.translation -= rb2.mprops.local_mprops.local_com;
        }
    }
}

macro_rules! joint_conversion_methods(
    ($as_joint: ident, $as_joint_mut: ident, $Joint: ty, $axes: expr) => {
        /// Converts the joint to its specific variant, if it is one.
        #[must_use]
        pub fn $as_joint(&self) -> Option<&$Joint> {
            if self.locked_axes == $axes {
                // SAFETY: this is OK because the target joint type is
                //         a `repr(transparent)` newtype of `Joint`.
                Some(unsafe { std::mem::transmute::<&Self, &$Joint>(self) })
            } else {
                None
            }
        }

        /// Converts the joint to its specific mutable variant, if it is one.
        #[must_use]
        pub fn $as_joint_mut(&mut self) -> Option<&mut $Joint> {
            if self.locked_axes == $axes {
                // SAFETY: this is OK because the target joint type is
                //         a `repr(transparent)` newtype of `Joint`.
                Some(unsafe { std::mem::transmute::<&mut Self, &mut $Joint>(self) })
            } else {
                None
            }
        }
    }
);

impl GenericJoint {
    joint_conversion_methods!(
        as_revolute,
        as_revolute_mut,
        RevoluteJoint,
        JointAxesMask::LOCKED_REVOLUTE_AXES
    );
    joint_conversion_methods!(
        as_fixed,
        as_fixed_mut,
        FixedJoint,
        JointAxesMask::LOCKED_FIXED_AXES
    );
    joint_conversion_methods!(
        as_prismatic,
        as_prismatic_mut,
        PrismaticJoint,
        JointAxesMask::LOCKED_PRISMATIC_AXES
    );
    joint_conversion_methods!(
        as_rope,
        as_rope_mut,
        RopeJoint,
        JointAxesMask::FREE_FIXED_AXES
    );

    #[cfg(feature = "dim3")]
    joint_conversion_methods!(
        as_spherical,
        as_spherical_mut,
        SphericalJoint,
        JointAxesMask::LOCKED_SPHERICAL_AXES
    );
}

/// Create generic joints using the builder pattern.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GenericJointBuilder(pub GenericJoint);

impl GenericJointBuilder {
    /// Creates a new generic joint builder.
    #[must_use]
    pub fn new(locked_axes: JointAxesMask) -> Self {
        Self(GenericJoint::new(locked_axes))
    }

    /// Sets the degrees of freedom locked by the joint.
    #[must_use]
    pub fn locked_axes(mut self, axes: JointAxesMask) -> Self {
        self.0.locked_axes = axes;
        self
    }

    /// Sets whether contacts between the attached rigid-bodies are enabled.
    #[must_use]
    pub fn contacts_enabled(mut self, enabled: bool) -> Self {
        self.0.contacts_enabled = enabled;
        self
    }

    /// Sets the joint’s frame, expressed in the first rigid-body’s local-space.
    #[must_use]
    pub fn local_frame1(mut self, local_frame: Pose) -> Self {
        self.0.set_local_frame1(local_frame);
        self
    }

    /// Sets the joint’s frame, expressed in the second rigid-body’s local-space.
    #[must_use]
    pub fn local_frame2(mut self, local_frame: Pose) -> Self {
        self.0.set_local_frame2(local_frame);
        self
    }

    /// Sets the principal (local X) axis of this joint, expressed in the first rigid-body’s local-space.
    #[must_use]
    pub fn local_axis1(mut self, local_axis: Vector) -> Self {
        self.0.set_local_axis1(local_axis);
        self
    }

    /// Sets the principal (local X) axis of this joint, expressed in the second rigid-body’s local-space.
    #[must_use]
    pub fn local_axis2(mut self, local_axis: Vector) -> Self {
        self.0.set_local_axis2(local_axis);
        self
    }

    /// Sets the anchor of this joint, expressed in the first rigid-body’s local-space.
    #[must_use]
    pub fn local_anchor1(mut self, anchor1: Vector) -> Self {
        self.0.set_local_anchor1(anchor1);
        self
    }

    /// Sets the anchor of this joint, expressed in the second rigid-body’s local-space.
    #[must_use]
    pub fn local_anchor2(mut self, anchor2: Vector) -> Self {
        self.0.set_local_anchor2(anchor2);
        self
    }

    /// Sets the joint limits along the specified axis.
    #[must_use]
    pub fn limits(mut self, axis: JointAxis, limits: [Real; 2]) -> Self {
        self.0.set_limits(axis, limits);
        self
    }

    /// Sets the coupled degrees of freedom for this joint’s limits and motor.
    #[must_use]
    pub fn coupled_axes(mut self, axes: JointAxesMask) -> Self {
        self.0.coupled_axes = axes;
        self
    }

    /// Set the spring-like model used by the motor to reach the desired target velocity and position.
    #[must_use]
    pub fn motor_model(mut self, axis: JointAxis, model: MotorModel) -> Self {
        self.0.set_motor_model(axis, model);
        self
    }

    /// Sets the target velocity this motor needs to reach.
    #[must_use]
    pub fn motor_velocity(mut self, axis: JointAxis, target_vel: Real, factor: Real) -> Self {
        self.0.set_motor_velocity(axis, target_vel, factor);
        self
    }

    /// Sets the target angle this motor needs to reach.
    #[must_use]
    pub fn motor_position(
        mut self,
        axis: JointAxis,
        target_pos: Real,
        stiffness: Real,
        damping: Real,
    ) -> Self {
        self.0
            .set_motor_position(axis, target_pos, stiffness, damping);
        self
    }

    /// Configure both the target angle and target velocity of the motor.
    #[must_use]
    pub fn set_motor(
        mut self,
        axis: JointAxis,
        target_pos: Real,
        target_vel: Real,
        stiffness: Real,
        damping: Real,
    ) -> Self {
        self.0
            .set_motor(axis, target_pos, target_vel, stiffness, damping);
        self
    }

    /// Sets the maximum force the motor can deliver along the specified axis.
    #[must_use]
    pub fn motor_max_force(mut self, axis: JointAxis, max_force: Real) -> Self {
        self.0.set_motor_max_force(axis, max_force);
        self
    }

    /// Sets the softness of this joint’s locked degrees of freedom.
    #[must_use]
    pub fn softness(mut self, softness: SpringCoefficients<Real>) -> Self {
        self.0.softness = softness;
        self
    }

    /// An arbitrary user-defined 128-bit integer associated to the joints built by this builder.
    pub fn user_data(mut self, data: u128) -> Self {
        self.0.user_data = data;
        self
    }

    /// Builds the generic joint.
    #[must_use]
    pub fn build(self) -> GenericJoint {
        self.0
    }
}

impl From<GenericJointBuilder> for GenericJoint {
    fn from(val: GenericJointBuilder) -> GenericJoint {
        val.0
    }
}
