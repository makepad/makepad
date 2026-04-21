use crate::dynamics::{AxesMask, RigidBody, RigidBodyPosition, RigidBodyVelocity};
use crate::math::{AngVector, Pose, Real, Rotation, Vector};

/// A Proportional-Derivative (PD) controller.
///
/// This is useful for controlling a rigid-body at the velocity level so it matches a target
/// pose.
///
/// This is a [PID controller](https://en.wikipedia.org/wiki/Proportional%E2%80%93integral%E2%80%93derivative_controller)
/// without the Integral part to keep the API immutable, while having a behaviour generally
/// sufficient for games.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct PdController {
    /// The Proportional gain applied to the instantaneous linear position errors.
    ///
    /// This is usually set to a multiple of the inverse of simulation step time
    /// (e.g. `60` if the delta-time is `1.0 / 60.0`).
    pub lin_kp: Vector,
    /// The Derivative gain applied to the instantaneous linear velocity errors.
    ///
    /// This is usually set to a value in `[0.0, 1.0]` where `0.0` implies no damping
    /// (no correction of velocity errors) and `1.0` implies complete damping (velocity errors
    /// are corrected in a single simulation step).
    pub lin_kd: Vector,
    /// The Proportional gain applied to the instantaneous angular position errors.
    ///
    /// This is usually set to a multiple of the inverse of simulation step time
    /// (e.g. `60` if the delta-time is `1.0 / 60.0`).
    pub ang_kp: AngVector,
    /// The Derivative gain applied to the instantaneous angular velocity errors.
    ///
    /// This is usually set to a value in `[0.0, 1.0]` where `0.0` implies no damping
    /// (no correction of velocity errors) and `1.0` implies complete damping (velocity errors
    /// are corrected in a single simulation step).
    pub ang_kd: AngVector,
    /// The axes affected by this controller.
    ///
    /// Only coordinate axes with a bit flags set to `true` will be taken into
    /// account when calculating the errors and corrections.
    pub axes: AxesMask,
}

impl Default for PdController {
    fn default() -> Self {
        Self::new(60.0, 0.8, AxesMask::all())
    }
}

/// A Proportional-Integral-Derivative (PID) controller.
///
/// For video games, the Proportional-Derivative [`PdController`] is generally sufficient and
/// offers an immutable API.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct PidController {
    /// The Proportional-Derivative (PD) part of this PID controller.
    pub pd: PdController,
    /// The translational error accumulated through time for the Integral part of the PID controller.
    pub lin_integral: Vector,
    /// The angular error accumulated through time for the Integral part of the PID controller.
    pub ang_integral: AngVector,
    /// The linear gain applied to the Integral part of the PID controller.
    pub lin_ki: Vector,
    /// The angular gain applied to the Integral part of the PID controller.
    pub ang_ki: AngVector,
}

impl Default for PidController {
    fn default() -> Self {
        Self::new(60.0, 1.0, 0.8, AxesMask::all())
    }
}

/// Position or velocity errors measured for PID control.
pub struct PdErrors {
    /// The linear (translational) part of the error.
    pub linear: Vector,
    /// The angular (rotational) part of the error.
    pub angular: AngVector,
}

impl From<RigidBodyVelocity<Real>> for PdErrors {
    fn from(vels: RigidBodyVelocity<Real>) -> Self {
        Self {
            #[cfg(feature = "dim2")]
            linear: Vector::new(vels.linvel.x, vels.linvel.y),
            #[cfg(feature = "dim3")]
            linear: Vector::new(vels.linvel.x, vels.linvel.y, vels.linvel.z),
            #[cfg(feature = "dim2")]
            angular: vels.angvel,
            #[cfg(feature = "dim3")]
            angular: AngVector::new(vels.angvel.x, vels.angvel.y, vels.angvel.z),
        }
    }
}

impl PdController {
    /// Initialized the PD controller with uniform gain.
    ///
    /// The same gain are applied on all axes. To configure per-axes gains, construct
    /// the [`PdController`] by setting its fields explicitly instead.
    ///
    /// Only the axes specified in `axes` will be enabled (but the gain values are set
    /// on all axes regardless).
    pub fn new(kp: Real, kd: Real, axes: AxesMask) -> PdController {
        #[cfg(feature = "dim2")]
        return Self {
            lin_kp: Vector::splat(kp),
            lin_kd: Vector::splat(kd),
            ang_kp: kp,
            ang_kd: kd,
            axes,
        };

        #[cfg(feature = "dim3")]
        return Self {
            lin_kp: Vector::splat(kp),
            lin_kd: Vector::splat(kd),
            ang_kp: AngVector::splat(kp),
            ang_kd: AngVector::splat(kd),
            axes,
        };
    }

    /// Calculates the linear correction from positional and velocity errors calculated automatically
    /// from a rigid-body and the desired positions/velocities.
    ///
    /// The unit of the returned value depends on the gain values. In general, `kd` is proportional to
    /// the inverse of the simulation step so the returned value is a linear rigid-body velocity
    /// change.
    pub fn linear_rigid_body_correction(
        &self,
        rb: &RigidBody,
        target_pos: Vector,
        target_linvel: Vector,
    ) -> Vector {
        let angvel = rb.angvel();

        self.rigid_body_correction(
            rb,
            Pose::from_translation(target_pos),
            RigidBodyVelocity {
                linvel: target_linvel,
                angvel,
            },
        )
        .linvel
    }

    /// Calculates the angular correction from positional and velocity errors calculated automatically
    /// from a rigid-body and the desired positions/velocities.
    ///
    /// The unit of the returned value depends on the gain values. In general, `kd` is proportional to
    /// the inverse of the simulation step so the returned value is an angular rigid-body velocity
    /// change.
    pub fn angular_rigid_body_correction(
        &self,
        rb: &RigidBody,
        target_rot: Rotation,
        target_angvel: AngVector,
    ) -> AngVector {
        self.rigid_body_correction(
            rb,
            Pose::from_parts(Vector::ZERO, target_rot),
            RigidBodyVelocity {
                linvel: rb.linvel(),
                angvel: target_angvel,
            },
        )
        .angvel
    }

    /// Calculates the linear and angular  correction from positional and velocity errors calculated
    /// automatically from a rigid-body and the desired poses/velocities.
    ///
    /// The unit of the returned value depends on the gain values. In general, `kd` is proportional to
    /// the inverse of the simulation step so the returned value is a rigid-body velocity
    /// change.
    pub fn rigid_body_correction(
        &self,
        rb: &RigidBody,
        target_pose: Pose,
        target_vels: RigidBodyVelocity<Real>,
    ) -> RigidBodyVelocity<Real> {
        let pose_errors = RigidBodyPosition {
            position: rb.pos.position,
            next_position: target_pose,
        }
        .pose_errors(rb.local_center_of_mass());
        let vels_errors = target_vels - rb.vels;
        self.correction(&pose_errors, &vels_errors.into())
    }

    /// Mask where each component is 1.0 or 0.0 depending on whether
    /// the corresponding linear axis is enabled.
    fn lin_mask(&self) -> Vector {
        #[cfg(feature = "dim2")]
        return Vector::new(
            self.axes.contains(AxesMask::LIN_X) as u32 as Real,
            self.axes.contains(AxesMask::LIN_Y) as u32 as Real,
        );
        #[cfg(feature = "dim3")]
        return Vector::new(
            self.axes.contains(AxesMask::LIN_X) as u32 as Real,
            self.axes.contains(AxesMask::LIN_Y) as u32 as Real,
            self.axes.contains(AxesMask::LIN_Z) as u32 as Real,
        );
    }

    /// Mask where each component is 1.0 or 0.0 depending on whether
    /// the corresponding angular axis is enabled.
    fn ang_mask(&self) -> AngVector {
        #[cfg(feature = "dim2")]
        return self.axes.contains(AxesMask::ANG_Z) as u32 as Real;
        #[cfg(feature = "dim3")]
        return Vector::new(
            self.axes.contains(AxesMask::ANG_X) as u32 as Real,
            self.axes.contains(AxesMask::ANG_Y) as u32 as Real,
            self.axes.contains(AxesMask::ANG_Z) as u32 as Real,
        );
    }

    /// Calculates the linear and angular correction from the given positional and velocity errors.
    ///
    /// The unit of the returned value depends on the gain values. In general, `kd` is proportional to
    /// the inverse of the simulation step so the returned value is a rigid-body velocity
    /// change.
    pub fn correction(
        &self,
        pose_errors: &PdErrors,
        vel_errors: &PdErrors,
    ) -> RigidBodyVelocity<Real> {
        let lin_mask = self.lin_mask();
        let ang_mask = self.ang_mask();

        let linvel =
            (pose_errors.linear * self.lin_kp + vel_errors.linear * self.lin_kd) * lin_mask;
        let angvel =
            (pose_errors.angular * self.ang_kp + vel_errors.angular * self.ang_kd) * ang_mask;

        RigidBodyVelocity { linvel, angvel }
    }
}

impl PidController {
    /// Initialized the PDI controller with uniform gain.
    ///
    /// The same gain are applied on all axes. To configure per-axes gains, construct
    /// the [`PidController`] by setting its fields explicitly instead.
    ///
    /// Only the axes specified in `axes` will be enabled (but the gain values are set
    /// on all axes regardless).
    pub fn new(kp: Real, ki: Real, kd: Real, axes: AxesMask) -> PidController {
        #[cfg(feature = "dim2")]
        return Self {
            pd: PdController::new(kp, kd, axes),
            lin_integral: Vector::ZERO,
            ang_integral: 0.0,
            lin_ki: Vector::splat(ki),
            ang_ki: ki,
        };

        #[cfg(feature = "dim3")]
        return Self {
            pd: PdController::new(kp, kd, axes),
            lin_integral: Vector::ZERO,
            ang_integral: AngVector::ZERO,
            lin_ki: Vector::splat(ki),
            ang_ki: AngVector::splat(ki),
        };
    }

    /// Set the axes errors and corrections are computed for.
    ///
    /// This doesn’t modify any of the gains.
    pub fn set_axes(&mut self, axes: AxesMask) {
        self.pd.axes = axes;
    }

    /// Get the axes errors and corrections are computed for.
    pub fn axes(&self) -> AxesMask {
        self.pd.axes
    }

    /// Resets to zero the accumulated linear and angular errors used by
    /// the Integral part of the controller.
    pub fn reset_integrals(&mut self) {
        self.lin_integral = Vector::ZERO;
        #[cfg(feature = "dim2")]
        {
            self.ang_integral = 0.0;
        }
        #[cfg(feature = "dim3")]
        {
            self.ang_integral = AngVector::ZERO;
        }
    }

    /// Calculates the linear correction from positional and velocity errors calculated automatically
    /// from a rigid-body and the desired positions/velocities.
    ///
    /// The unit of the returned value depends on the gain values. In general, `kd` is proportional to
    /// the inverse of the simulation step so the returned value is a linear rigid-body velocity
    /// change.
    ///
    /// This method is mutable because of the need to update the accumulated positional
    /// errors for the Integral part of this controller. Prefer the [`PdController`] instead if
    /// an immutable API is needed.
    pub fn linear_rigid_body_correction(
        &mut self,
        dt: Real,
        rb: &RigidBody,
        target_pos: Vector,
        target_linvel: Vector,
    ) -> Vector {
        self.rigid_body_correction(
            dt,
            rb,
            Pose::from_translation(target_pos),
            RigidBodyVelocity {
                linvel: target_linvel,
                angvel: rb.angvel(),
            },
        )
        .linvel
    }

    /// Calculates the angular correction from positional and velocity errors calculated automatically
    /// from a rigid-body and the desired positions/velocities.
    ///
    /// The unit of the returned value depends on the gain values. In general, `kd` is proportional to
    /// the inverse of the simulation step so the returned value is an angular rigid-body velocity
    /// change.
    ///
    /// This method is mutable because of the need to update the accumulated positional
    /// errors for the Integral part of this controller. Prefer the [`PdController`] instead if
    /// an immutable API is needed.
    pub fn angular_rigid_body_correction(
        &mut self,
        dt: Real,
        rb: &RigidBody,
        target_rot: Rotation,
        target_angvel: AngVector,
    ) -> AngVector {
        self.rigid_body_correction(
            dt,
            rb,
            Pose::from_parts(Vector::ZERO, target_rot),
            RigidBodyVelocity {
                linvel: rb.linvel(),
                angvel: target_angvel,
            },
        )
        .angvel
    }

    /// Calculates the linear and angular  correction from positional and velocity errors calculated
    /// automatically from a rigid-body and the desired poses/velocities.
    ///
    /// The unit of the returned value depends on the gain values. In general, `kd` is proportional to
    /// the inverse of the simulation step so the returned value is a rigid-body velocity
    /// change.
    ///
    /// This method is mutable because of the need to update the accumulated positional
    /// errors for the Integral part of this controller. Prefer the [`PdController`] instead if
    /// an immutable API is needed.
    pub fn rigid_body_correction(
        &mut self,
        dt: Real,
        rb: &RigidBody,
        target_pose: Pose,
        target_vels: RigidBodyVelocity<Real>,
    ) -> RigidBodyVelocity<Real> {
        let pose_errors = RigidBodyPosition {
            position: rb.pos.position,
            next_position: target_pose,
        }
        .pose_errors(rb.local_center_of_mass());
        let vels_errors = target_vels - rb.vels;
        self.correction(dt, &pose_errors, &vels_errors.into())
    }

    /// Calculates the linear and angular correction from the given positional and velocity errors.
    ///
    /// The unit of the returned value depends on the gain values. In general, `kd` is proportional to
    /// the inverse of the simulation step so the returned value is a rigid-body velocity
    /// change.
    ///
    /// This method is mutable because of the need to update the accumulated positional
    /// errors for the Integral part of this controller. Prefer the [`PdController`] instead if
    /// an immutable API is needed.
    pub fn correction(
        &mut self,
        dt: Real,
        pose_errors: &PdErrors,
        vel_errors: &PdErrors,
    ) -> RigidBodyVelocity<Real> {
        self.lin_integral += pose_errors.linear * dt;
        self.ang_integral += pose_errors.angular * dt;

        let lin_mask = self.pd.lin_mask();
        let ang_mask = self.pd.ang_mask();

        let linvel = (pose_errors.linear * self.pd.lin_kp
            + vel_errors.linear * self.pd.lin_kd
            + self.lin_integral * self.lin_ki)
            * lin_mask;
        #[cfg(feature = "dim2")]
        let angvel = (pose_errors.angular * self.pd.ang_kp
            + vel_errors.angular * self.pd.ang_kd
            + self.ang_integral * self.ang_ki)
            * ang_mask;
        #[cfg(feature = "dim3")]
        let angvel = (pose_errors.angular * self.pd.ang_kp
            + vel_errors.angular * self.pd.ang_kd
            + self.ang_integral * self.ang_ki)
            * ang_mask;

        RigidBodyVelocity { linvel, angvel }
    }
}
