#[cfg(doc)]
use super::IntegrationParameters;
use crate::control::PdErrors;
#[cfg(doc)]
use crate::control::PidController;
use crate::dynamics::MassProperties;
use crate::geometry::{
    ColliderChanges, ColliderHandle, ColliderMassProps, ColliderParent, ColliderPosition,
    ColliderSet, ColliderShape, ModifiedColliders,
};
use crate::math::{AngVector, AngularInertia, Pose, Real, Rotation, Vector};
use crate::utils::{
    AngularInertiaOps, CrossProduct, DotProduct, PoseOps, ScalarType, SimdRealCopy,
};
use num::Zero;
#[cfg(feature = "dim2")]
use parry::math::Rot2;

/// The unique handle of a rigid body added to a `RigidBodySet`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[repr(transparent)]
pub struct RigidBodyHandle(pub crate::data::arena::Index);

impl RigidBodyHandle {
    /// Converts this handle into its (index, generation) components.
    pub fn into_raw_parts(self) -> (u32, u32) {
        self.0.into_raw_parts()
    }

    /// Reconstructs an handle from its (index, generation) components.
    pub fn from_raw_parts(id: u32, generation: u32) -> Self {
        Self(crate::data::arena::Index::from_raw_parts(id, generation))
    }

    /// An always-invalid rigid-body handle.
    pub fn invalid() -> Self {
        Self(crate::data::arena::Index::from_raw_parts(
            crate::INVALID_U32,
            crate::INVALID_U32,
        ))
    }
}

/// The type of a body, governing the way it is affected by external forces.
#[deprecated(note = "renamed as RigidBodyType")]
pub type BodyStatus = RigidBodyType;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
/// The type of a rigid body, determining how it responds to forces and movement.
pub enum RigidBodyType {
    /// Fully simulated - responds to forces, gravity, and collisions.
    ///
    /// Use for: Falling objects, projectiles, physics-based characters, anything that should
    /// behave realistically under physics simulation.
    Dynamic = 0,

    /// Never moves - has infinite mass and is unaffected by anything.
    ///
    /// Use for: Static level geometry, walls, floors, terrain, buildings.
    Fixed = 1,

    /// Controlled by setting next position - pushes but isn't pushed.
    ///
    /// You control this by setting where it should be next frame. Rapier computes the
    /// velocity needed to get there. The body can push dynamic bodies but nothing can
    /// push it back (one-way interaction).
    ///
    /// Use for: Animated platforms, objects controlled by external animation systems.
    KinematicPositionBased = 2,

    /// Controlled by setting velocity - pushes but isn't pushed.
    ///
    /// You control this by setting its velocity directly. It moves predictably regardless
    /// of what it hits. Can push dynamic bodies but nothing can push it back (one-way interaction).
    ///
    /// Use for: Moving platforms, elevators, doors, player-controlled characters (when you want
    /// direct control rather than physics-based movement).
    KinematicVelocityBased = 3,
    // Semikinematic, // A kinematic that performs automatic CCD with the fixed environment to avoid traversing it?
    // Disabled,
}

impl RigidBodyType {
    /// Is this rigid-body fixed (i.e. cannot move)?
    pub fn is_fixed(self) -> bool {
        self == RigidBodyType::Fixed
    }

    /// Is this rigid-body dynamic (i.e. can move and be affected by forces)?
    pub fn is_dynamic(self) -> bool {
        self == RigidBodyType::Dynamic
    }

    /// Is this rigid-body kinematic (i.e. can move but is unaffected by forces)?
    pub fn is_kinematic(self) -> bool {
        self == RigidBodyType::KinematicPositionBased
            || self == RigidBodyType::KinematicVelocityBased
    }

    /// Is this rigid-body a dynamic rigid-body or a kinematic rigid-body?
    ///
    /// This method is mostly convenient internally where kinematic and dynamic rigid-body
    /// are subject to the same behavior.
    pub fn is_dynamic_or_kinematic(self) -> bool {
        self != RigidBodyType::Fixed
    }
}

bitflags::bitflags! {
    #[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    /// Flags describing how the rigid-body has been modified by the user.
    pub struct RigidBodyChanges: u32 {
        /// Flag indicating that this rigid-body is in the modified rigid-body set.
        const IN_MODIFIED_SET = 1 << 0;
        /// Flag indicating that the `RigidBodyPosition` component of this rigid-body has been modified.
        const POSITION    = 1 << 1;
        /// Flag indicating that the `RigidBodyActivation` component of this rigid-body has been modified.
        const SLEEP       = 1 << 2;
        /// Flag indicating that the `RigidBodyColliders` component of this rigid-body has been modified.
        const COLLIDERS   = 1 << 3;
        /// Flag indicating that the `RigidBodyType` component of this rigid-body has been modified.
        const TYPE        = 1 << 4;
        /// Flag indicating that the `RigidBodyDominance` component of this rigid-body has been modified.
        const DOMINANCE   = 1 << 5;
        /// Flag indicating that the local mass-properties of this rigid-body must be recomputed.
        const LOCAL_MASS_PROPERTIES = 1 << 6;
        /// Flag indicating that the rigid-body was enabled or disabled.
        const ENABLED_OR_DISABLED = 1 << 7;
    }
}

impl Default for RigidBodyChanges {
    fn default() -> Self {
        RigidBodyChanges::empty()
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Copy, PartialEq)]
/// The position of this rigid-body.
pub struct RigidBodyPosition {
    /// The world-space position of the rigid-body.
    pub position: Pose,
    /// The next position of the rigid-body.
    ///
    /// At the beginning of the timestep, and when the
    /// timestep is complete we must have position == next_position
    /// except for position-based kinematic bodies.
    ///
    /// The next_position is updated after the velocity and position
    /// resolution. Then it is either validated (ie. we set position := set_position)
    /// or clamped by CCD.
    pub next_position: Pose,
}

impl Default for RigidBodyPosition {
    fn default() -> Self {
        Self {
            position: Pose::IDENTITY,
            next_position: Pose::IDENTITY,
        }
    }
}

impl RigidBodyPosition {
    /// Computes the velocity need to travel from `self.position` to `self.next_position` in
    /// a time equal to `1.0 / inv_dt`.
    #[must_use]
    pub fn interpolate_velocity(&self, inv_dt: Real, local_com: Vector) -> RigidBodyVelocity<Real> {
        let pose_err = self.pose_errors(local_com);
        RigidBodyVelocity {
            linvel: pose_err.linear * inv_dt,
            angvel: pose_err.angular * inv_dt,
        }
    }

    /// Compute new positions after integrating the given forces and velocities.
    ///
    /// This uses a symplectic Euler integration scheme.
    #[must_use]
    pub fn integrate_forces_and_velocities(
        &self,
        dt: Real,
        forces: &RigidBodyForces,
        vels: &RigidBodyVelocity<Real>,
        mprops: &RigidBodyMassProps,
    ) -> Pose {
        let new_vels = forces.integrate(dt, vels, mprops);
        let local_com = mprops.local_mprops.local_com;
        new_vels.integrate(dt, &self.position, &local_com)
    }

    /// Computes the difference between [`Self::next_position`] and [`Self::position`].
    ///
    /// This error measure can for example be used for interpolating the velocity between two poses,
    /// or be given to the [`PidController`].
    ///
    /// Note that interpolating the velocity can be done more conveniently with
    /// [`Self::interpolate_velocity`].
    pub fn pose_errors(&self, local_com: Vector) -> PdErrors {
        let com = self.position * local_com;
        let shift = Pose::from_translation(com);
        let dpos = shift.inverse() * self.next_position * self.position.inverse() * shift;

        let angular;
        #[cfg(feature = "dim2")]
        {
            angular = dpos.rotation.angle();
        }
        #[cfg(feature = "dim3")]
        {
            angular = dpos.rotation.to_scaled_axis();
        }
        let linear = dpos.translation;

        PdErrors { linear, angular }
    }
}

impl<T> From<T> for RigidBodyPosition
where
    Pose: From<T>,
{
    fn from(position: T) -> Self {
        let position = position.into();
        Self {
            position,
            next_position: position,
        }
    }
}

bitflags::bitflags! {
    #[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    /// Flags affecting the behavior of the constraints solver for a given contact manifold.
    pub struct AxesMask: u8 {
        /// The translational X axis.
        const LIN_X = 1 << 0;
        /// The translational Y axis.
        const LIN_Y = 1 << 1;
        /// The translational Z axis.
        #[cfg(feature = "dim3")]
        const LIN_Z = 1 << 2;
        /// The rotational X axis.
        #[cfg(feature = "dim3")]
        const ANG_X = 1 << 3;
        /// The rotational Y axis.
        #[cfg(feature = "dim3")]
        const ANG_Y = 1 << 4;
        /// The rotational Z axis.
        const ANG_Z = 1 << 5;
    }
}

impl Default for AxesMask {
    fn default() -> Self {
        AxesMask::empty()
    }
}

bitflags::bitflags! {
    #[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    /// Flags that lock specific movement axes to prevent translation or rotation.
    ///
    /// Use this to constrain body movement to specific directions/axes. Common uses:
    /// - **2D games in 3D**: Lock Z translation and X/Y rotation to keep everything in the XY plane
    /// - **Upright characters**: Lock rotations to prevent tipping over
    /// - **Sliding objects**: Lock rotation while allowing translation
    /// - **Spinning objects**: Lock translation while allowing rotation
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let body_handle = bodies.insert(RigidBodyBuilder::dynamic());
    /// # let body = bodies.get_mut(body_handle).unwrap();
    /// // Character that can't tip over (rotation locked, but can move)
    /// body.set_locked_axes(LockedAxes::ROTATION_LOCKED, true);
    ///
    /// // Object that slides but doesn't rotate
    /// body.set_locked_axes(LockedAxes::ROTATION_LOCKED, true);
    ///
    /// // 2D game in 3D engine (lock Z movement and X/Y rotation)
    /// body.set_locked_axes(
    ///     LockedAxes::TRANSLATION_LOCKED_Z |
    ///     LockedAxes::ROTATION_LOCKED_X |
    ///     LockedAxes::ROTATION_LOCKED_Y,
    ///     true
    /// );
    /// ```
    pub struct LockedAxes: u8 {
        /// Prevents movement along the X axis.
        const TRANSLATION_LOCKED_X = 1 << 0;
        /// Prevents movement along the Y axis.
        const TRANSLATION_LOCKED_Y = 1 << 1;
        /// Prevents movement along the Z axis.
        const TRANSLATION_LOCKED_Z = 1 << 2;
        /// Prevents all translational movement.
        const TRANSLATION_LOCKED = Self::TRANSLATION_LOCKED_X.bits() | Self::TRANSLATION_LOCKED_Y.bits() | Self::TRANSLATION_LOCKED_Z.bits();
        /// Prevents rotation around the X axis.
        const ROTATION_LOCKED_X = 1 << 3;
        /// Prevents rotation around the Y axis.
        const ROTATION_LOCKED_Y = 1 << 4;
        /// Prevents rotation around the Z axis.
        const ROTATION_LOCKED_Z = 1 << 5;
        /// Prevents all rotational movement.
        const ROTATION_LOCKED = Self::ROTATION_LOCKED_X.bits() | Self::ROTATION_LOCKED_Y.bits() | Self::ROTATION_LOCKED_Z.bits();
    }
}

/// Mass and angular inertia added to a rigid-body on top of its attached colliders’ contributions.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum RigidBodyAdditionalMassProps {
    /// Mass properties to be added as-is.
    MassProps(MassProperties),
    /// Mass to be added to the rigid-body. This will also automatically scale
    /// the attached colliders total angular inertia to account for the added mass.
    Mass(Real),
}

impl Default for RigidBodyAdditionalMassProps {
    fn default() -> Self {
        RigidBodyAdditionalMassProps::MassProps(MassProperties::default())
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, PartialEq)]
// #[repr(C)]
/// The mass properties of a rigid-body.
pub struct RigidBodyMassProps {
    /// The world-space center of mass of the rigid-body.
    pub world_com: Vector,
    /// The inverse mass taking into account translation locking.
    pub effective_inv_mass: Vector,
    /// The square-root of the world-space inverse angular inertia tensor of the rigid-body,
    /// taking into account rotation locking.
    pub effective_world_inv_inertia: AngularInertia,
    /// The local mass properties of the rigid-body.
    pub local_mprops: MassProperties,
    /// Flags for locking rotation and translation.
    pub flags: LockedAxes,
    /// Mass-properties of this rigid-bodies, added to the contributions of its attached colliders.
    pub additional_local_mprops: Option<Box<RigidBodyAdditionalMassProps>>,
}

impl Default for RigidBodyMassProps {
    fn default() -> Self {
        Self {
            flags: LockedAxes::empty(),
            local_mprops: MassProperties::zero(),
            additional_local_mprops: None,
            world_com: Vector::ZERO,
            effective_inv_mass: Vector::ZERO,
            effective_world_inv_inertia: AngularInertia::zero(),
        }
    }
}

impl From<LockedAxes> for RigidBodyMassProps {
    fn from(flags: LockedAxes) -> Self {
        Self {
            flags,
            ..Self::default()
        }
    }
}

impl From<MassProperties> for RigidBodyMassProps {
    fn from(local_mprops: MassProperties) -> Self {
        Self {
            local_mprops,
            ..Default::default()
        }
    }
}

impl RigidBodyMassProps {
    /// The mass of the rigid-body.
    #[must_use]
    pub fn mass(&self) -> Real {
        crate::utils::inv(self.local_mprops.inv_mass)
    }

    /// The effective mass (that takes the potential translation locking into account) of
    /// this rigid-body.
    #[must_use]
    pub fn effective_mass(&self) -> Vector {
        self.effective_inv_mass.map(crate::utils::inv)
    }

    /// The square root of the effective world-space angular inertia (that takes the potential rotation locking into account) of
    /// this rigid-body.
    #[must_use]
    pub fn effective_angular_inertia(&self) -> AngularInertia {
        #[allow(unused_mut)] // mut needed in 3D.
        let mut ang_inertia = self.effective_world_inv_inertia;

        // Make the matrix invertible.
        #[cfg(feature = "dim3")]
        {
            if self.flags.contains(LockedAxes::ROTATION_LOCKED_X) {
                ang_inertia.m11 = 1.0;
            }
            if self.flags.contains(LockedAxes::ROTATION_LOCKED_Y) {
                ang_inertia.m22 = 1.0;
            }
            if self.flags.contains(LockedAxes::ROTATION_LOCKED_Z) {
                ang_inertia.m33 = 1.0;
            }
        }

        #[allow(unused_mut)] // mut needed in 3D.
        let mut result = ang_inertia.inverse();

        // Remove the locked axes again.
        #[cfg(feature = "dim3")]
        {
            if self.flags.contains(LockedAxes::ROTATION_LOCKED_X) {
                result.m11 = 0.0;
            }
            if self.flags.contains(LockedAxes::ROTATION_LOCKED_Y) {
                result.m22 = 0.0;
            }
            if self.flags.contains(LockedAxes::ROTATION_LOCKED_Z) {
                result.m33 = 0.0;
            }
        }

        result
    }

    /// Recompute the mass-properties of this rigid-bodies based on its currently attached colliders.
    pub fn recompute_mass_properties_from_colliders(
        &mut self,
        colliders: &ColliderSet,
        attached_colliders: &RigidBodyColliders,
        body_type: RigidBodyType,
        position: &Pose,
    ) {
        let added_mprops = self
            .additional_local_mprops
            .as_ref()
            .map(|mprops| **mprops)
            .unwrap_or_else(|| RigidBodyAdditionalMassProps::MassProps(MassProperties::default()));

        self.local_mprops = MassProperties::default();

        for handle in &attached_colliders.0 {
            if let Some(co) = colliders.get(*handle) {
                if co.is_enabled() {
                    if let Some(co_parent) = co.parent {
                        let to_add = co
                            .mprops
                            .mass_properties(&*co.shape)
                            .transform_by(&co_parent.pos_wrt_parent);
                        self.local_mprops += to_add;
                    }
                }
            }
        }

        match added_mprops {
            RigidBodyAdditionalMassProps::MassProps(mprops) => {
                self.local_mprops += mprops;
            }
            RigidBodyAdditionalMassProps::Mass(mass) => {
                let new_mass = self.local_mprops.mass() + mass;
                self.local_mprops.set_mass(new_mass, true);
            }
        }

        self.update_world_mass_properties(body_type, position);
    }

    /// Update the world-space mass properties of `self`, taking into account the new position.
    pub fn update_world_mass_properties(&mut self, body_type: RigidBodyType, position: &Pose) {
        self.world_com = self.local_mprops.world_com(position);
        self.effective_inv_mass = Vector::splat(self.local_mprops.inv_mass);
        self.effective_world_inv_inertia = self.local_mprops.world_inv_inertia(&position.rotation);

        // Take into account translation/rotation locking.
        if !body_type.is_dynamic() || self.flags.contains(LockedAxes::TRANSLATION_LOCKED_X) {
            self.effective_inv_mass.x = 0.0;
        }

        if !body_type.is_dynamic() || self.flags.contains(LockedAxes::TRANSLATION_LOCKED_Y) {
            self.effective_inv_mass.y = 0.0;
        }

        #[cfg(feature = "dim3")]
        if !body_type.is_dynamic() || self.flags.contains(LockedAxes::TRANSLATION_LOCKED_Z) {
            self.effective_inv_mass.z = 0.0;
        }

        #[cfg(feature = "dim2")]
        {
            if !body_type.is_dynamic() || self.flags.contains(LockedAxes::ROTATION_LOCKED_Z) {
                self.effective_world_inv_inertia = 0.0;
            }
        }
        #[cfg(feature = "dim3")]
        {
            if !body_type.is_dynamic() || self.flags.contains(LockedAxes::ROTATION_LOCKED_X) {
                self.effective_world_inv_inertia.m11 = 0.0;
                self.effective_world_inv_inertia.m12 = 0.0;
                self.effective_world_inv_inertia.m13 = 0.0;
            }

            if !body_type.is_dynamic() || self.flags.contains(LockedAxes::ROTATION_LOCKED_Y) {
                self.effective_world_inv_inertia.m22 = 0.0;
                self.effective_world_inv_inertia.m12 = 0.0;
                self.effective_world_inv_inertia.m23 = 0.0;
            }
            if !body_type.is_dynamic() || self.flags.contains(LockedAxes::ROTATION_LOCKED_Z) {
                self.effective_world_inv_inertia.m33 = 0.0;
                self.effective_world_inv_inertia.m13 = 0.0;
                self.effective_world_inv_inertia.m23 = 0.0;
            }
        }
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Copy, PartialEq)]
/// The velocities of this rigid-body.
pub struct RigidBodyVelocity<T: ScalarType> {
    /// The linear velocity of the rigid-body.
    pub linvel: T::Vector,
    /// The angular velocity of the rigid-body.
    pub angvel: T::AngVector,
}

impl Default for RigidBodyVelocity<Real> {
    fn default() -> Self {
        Self::zero()
    }
}

impl RigidBodyVelocity<Real> {
    /// Create a new rigid-body velocity component.
    #[must_use]
    #[cfg(feature = "dim2")]
    pub fn new(linvel: Vector, angvel: AngVector) -> Self {
        Self { linvel, angvel }
    }

    /// Create a new rigid-body velocity component.
    #[must_use]
    #[cfg(feature = "dim3")]
    pub fn new(linvel: Vector, angvel: AngVector) -> Self {
        Self {
            linvel: Vector::new(linvel.x, linvel.y, linvel.z),
            angvel: AngVector::new(angvel.x, angvel.y, angvel.z),
        }
    }

    /// Converts a slice to a rigid-body velocity.
    ///
    /// The slice must contain at least 3 elements: the `slice[0..2]` contains
    /// the linear velocity and the `slice[2]` contains the angular velocity.
    #[must_use]
    #[cfg(feature = "dim2")]
    pub fn from_slice(slice: &[Real]) -> Self {
        Self {
            linvel: Vector::new(slice[0], slice[1]),
            angvel: slice[2],
        }
    }

    /// Converts a slice to a rigid-body velocity.
    ///
    /// The slice must contain at least 6 elements: the `slice[0..3]` contains
    /// the linear velocity and the `slice[3..6]` contains the angular velocity.
    #[must_use]
    #[cfg(feature = "dim3")]
    pub fn from_slice(slice: &[Real]) -> Self {
        Self {
            linvel: Vector::new(slice[0], slice[1], slice[2]),
            angvel: AngVector::new(slice[3], slice[4], slice[5]),
        }
    }

    /// Velocities set to zero.
    #[must_use]
    pub fn zero() -> Self {
        Self {
            linvel: Default::default(),
            angvel: Default::default(),
        }
    }

    /// This velocity seen as a slice.
    ///
    /// The linear part is stored first.
    #[inline]
    pub fn as_slice(&self) -> &[Real] {
        self.as_vector().as_slice()
    }

    /// This velocity seen as a mutable slice.
    ///
    /// The linear part is stored first.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [Real] {
        self.as_vector_mut().as_mut_slice()
    }

    /// This velocity seen as a vector.
    ///
    /// The linear part is stored first.
    #[inline]
    #[cfg(feature = "dim2")]
    pub fn as_vector(&self) -> &na::Vector3<Real> {
        unsafe { std::mem::transmute(self) }
    }

    /// This velocity seen as a mutable vector.
    ///
    /// The linear part is stored first.
    #[inline]
    #[cfg(feature = "dim2")]
    pub fn as_vector_mut(&mut self) -> &mut na::Vector3<Real> {
        unsafe { std::mem::transmute(self) }
    }

    /// This velocity seen as a vector.
    ///
    /// The linear part is stored first.
    #[inline]
    #[cfg(feature = "dim3")]
    pub fn as_vector(&self) -> &na::Vector6<Real> {
        unsafe { std::mem::transmute(self) }
    }

    /// This velocity seen as a mutable vector.
    ///
    /// The linear part is stored first.
    #[inline]
    #[cfg(feature = "dim3")]
    pub fn as_vector_mut(&mut self) -> &mut na::Vector6<Real> {
        unsafe { std::mem::transmute(self) }
    }

    /// Return `self` rotated by `rotation`.
    #[must_use]
    #[cfg(feature = "dim2")]
    pub fn transformed(self, rotation: &Rotation) -> Self {
        Self {
            linvel: *rotation * self.linvel,
            angvel: self.angvel,
        }
    }

    /// Return `self` rotated by `rotation`.
    #[must_use]
    #[cfg(feature = "dim3")]
    pub fn transformed(self, rotation: &Rotation) -> Self {
        Self {
            linvel: *rotation * self.linvel,
            angvel: *rotation * self.angvel,
        }
    }

    /// The approximate kinetic energy of this rigid-body.
    ///
    /// This approximation does not take the rigid-body's mass and angular inertia
    /// into account. Some physics engines call this the "mass-normalized kinetic
    /// energy".
    #[must_use]
    pub fn pseudo_kinetic_energy(&self) -> Real {
        0.5 * (self.linvel.length_squared() + self.angvel.gdot(self.angvel))
    }

    /// The velocity of the given world-space point on this rigid-body.
    #[must_use]
    #[cfg(feature = "dim2")]
    pub fn velocity_at_point(&self, point: Vector, world_com: Vector) -> Vector {
        let dpt = point - world_com;
        self.linvel + self.angvel.gcross(dpt)
    }

    /// The velocity of the given world-space point on this rigid-body.
    #[must_use]
    #[cfg(feature = "dim3")]
    pub fn velocity_at_point(&self, point: Vector, world_com: Vector) -> Vector {
        let dpt = point - world_com;
        self.linvel + self.angvel.gcross(dpt)
    }

    /// Are these velocities exactly equal to zero?
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.linvel == Vector::ZERO && self.angvel == AngVector::default()
    }

    /// The kinetic energy of this rigid-body.
    #[must_use]
    pub fn kinetic_energy(&self, rb_mprops: &RigidBodyMassProps) -> Real {
        let mut energy = (rb_mprops.mass() * self.linvel.length_squared()) / 2.0;

        #[cfg(feature = "dim2")]
        if !num::Zero::is_zero(&rb_mprops.effective_world_inv_inertia) {
            let inertia = 1.0 / rb_mprops.effective_world_inv_inertia;
            energy += inertia * self.angvel * self.angvel / 2.0;
        }

        #[cfg(feature = "dim3")]
        if !rb_mprops.effective_world_inv_inertia.is_zero() {
            let inertia = rb_mprops.effective_world_inv_inertia.inverse_unchecked();
            energy += self.angvel.gdot(inertia * self.angvel) / 2.0;
        }

        energy
    }

    /// Applies an impulse at the center-of-mass of this rigid-body.
    /// The impulse is applied right away, changing the linear velocity.
    /// This does nothing on non-dynamic bodies.
    pub fn apply_impulse(&mut self, rb_mprops: &RigidBodyMassProps, impulse: Vector) {
        self.linvel += impulse * rb_mprops.effective_inv_mass;
    }

    /// Applies an angular impulse at the center-of-mass of this rigid-body.
    /// The impulse is applied right away, changing the angular velocity.
    /// This does nothing on non-dynamic bodies.
    #[cfg(feature = "dim2")]
    pub fn apply_torque_impulse(&mut self, rb_mprops: &RigidBodyMassProps, torque_impulse: Real) {
        self.angvel += rb_mprops.effective_world_inv_inertia * torque_impulse;
    }

    /// Applies an angular impulse at the center-of-mass of this rigid-body.
    /// The impulse is applied right away, changing the angular velocity.
    /// This does nothing on non-dynamic bodies.
    #[cfg(feature = "dim3")]
    pub fn apply_torque_impulse(&mut self, rb_mprops: &RigidBodyMassProps, torque_impulse: Vector) {
        self.angvel += rb_mprops.effective_world_inv_inertia * torque_impulse;
    }

    /// Applies an impulse at the given world-space point of this rigid-body.
    /// The impulse is applied right away, changing the linear and/or angular velocities.
    /// This does nothing on non-dynamic bodies.
    #[cfg(feature = "dim2")]
    pub fn apply_impulse_at_point(
        &mut self,
        rb_mprops: &RigidBodyMassProps,
        impulse: Vector,
        point: Vector,
    ) {
        let torque_impulse = (point - rb_mprops.world_com).perp_dot(impulse);
        self.apply_impulse(rb_mprops, impulse);
        self.apply_torque_impulse(rb_mprops, torque_impulse);
    }

    /// Applies an impulse at the given world-space point of this rigid-body.
    /// The impulse is applied right away, changing the linear and/or angular velocities.
    /// This does nothing on non-dynamic bodies.
    #[cfg(feature = "dim3")]
    pub fn apply_impulse_at_point(
        &mut self,
        rb_mprops: &RigidBodyMassProps,
        impulse: Vector,
        point: Vector,
    ) {
        let torque_impulse = (point - rb_mprops.world_com).cross(impulse);
        self.apply_impulse(rb_mprops, impulse);
        self.apply_torque_impulse(rb_mprops, torque_impulse);
    }
}

impl<T: ScalarType> RigidBodyVelocity<T> {
    /// Returns the update velocities after applying the given damping.
    #[must_use]
    pub fn apply_damping(&self, dt: T, damping: &RigidBodyDamping<T>) -> Self {
        let one = T::one();
        RigidBodyVelocity {
            linvel: self.linvel * (one / (one + dt * damping.linear_damping)),
            angvel: self.angvel * (one / (one + dt * damping.angular_damping)),
        }
    }

    /// Integrate the velocities in `self` to compute obtain new positions when moving from the given
    /// initial position `init_pos`.
    #[must_use]
    #[inline]
    #[allow(clippy::let_and_return)] // Keeping `result` binding for potential renormalization
    pub fn integrate(&self, dt: T, init_pos: &T::Pose, local_com: &T::Vector) -> T::Pose {
        let com = *init_pos * *local_com;
        let result = init_pos
            .append_translation(-com)
            .append_rotation(self.angvel * dt)
            .append_translation(com + self.linvel * dt);
        // TODO: is renormalization really useful?
        // result.rotation.renormalize_fast();
        result
    }
}

impl RigidBodyVelocity<Real> {
    /// Same as [`Self::integrate`] but with the angular part linearized and the local
    /// center-of-mass assumed to be zero.
    #[inline]
    #[cfg(feature = "dim2")]
    pub(crate) fn integrate_linearized(
        &self,
        dt: Real,
        translation: &mut Vector,
        rotation: &mut Rotation,
    ) {
        let dang = self.angvel * dt;
        let new_cos = rotation.re - dang * rotation.im;
        let new_sin = rotation.im + dang * rotation.re;
        *rotation = Rot2::from_cos_sin_unchecked(new_cos, new_sin);
        // NOTE: don't use renormalize_fast since the linearization might cause more drift.
        rotation.normalize_mut();
        *translation += self.linvel * dt;
    }

    /// Same as [`Self::integrate`] but with the angular part linearized and the local
    /// center-of-mass assumed to be zero.
    #[inline]
    #[cfg(feature = "dim3")]
    pub(crate) fn integrate_linearized(
        &self,
        dt: Real,
        translation: &mut Vector,
        rotation: &mut Rotation,
    ) {
        // Rotations linearization is inspired from
        // https://ahrs.readthedocs.io/en/latest/filters/angular.html (not using the matrix form).
        let hang = self.angvel * (dt * 0.5);
        // Quaternion identity + `hang` seen as a quaternion.
        let id_plus_hang = Rotation::from_xyzw(hang.x, hang.y, hang.z, 1.0);
        *rotation = id_plus_hang * *rotation;
        *rotation = rotation.normalize();
        *translation += self.linvel * dt;
    }
}

impl std::ops::Mul<Real> for RigidBodyVelocity<Real> {
    type Output = Self;

    fn mul(self, rhs: Real) -> Self {
        RigidBodyVelocity {
            linvel: self.linvel * rhs,
            angvel: self.angvel * rhs,
        }
    }
}

impl std::ops::Add<RigidBodyVelocity<Real>> for RigidBodyVelocity<Real> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        RigidBodyVelocity {
            linvel: self.linvel + rhs.linvel,
            angvel: self.angvel + rhs.angvel,
        }
    }
}

impl std::ops::AddAssign<RigidBodyVelocity<Real>> for RigidBodyVelocity<Real> {
    fn add_assign(&mut self, rhs: Self) {
        self.linvel += rhs.linvel;
        self.angvel += rhs.angvel;
    }
}

impl std::ops::Sub<RigidBodyVelocity<Real>> for RigidBodyVelocity<Real> {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        RigidBodyVelocity {
            linvel: self.linvel - rhs.linvel,
            angvel: self.angvel - rhs.angvel,
        }
    }
}

impl std::ops::SubAssign<RigidBodyVelocity<Real>> for RigidBodyVelocity<Real> {
    fn sub_assign(&mut self, rhs: Self) {
        self.linvel -= rhs.linvel;
        self.angvel -= rhs.angvel;
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Copy, PartialEq)]
/// Damping factors to progressively slow down a rigid-body.
pub struct RigidBodyDamping<T> {
    /// Damping factor for gradually slowing down the translational motion of the rigid-body.
    pub linear_damping: T,
    /// Damping factor for gradually slowing down the angular motion of the rigid-body.
    pub angular_damping: T,
}

impl<T: SimdRealCopy> Default for RigidBodyDamping<T> {
    fn default() -> Self {
        Self {
            linear_damping: T::zero(),
            angular_damping: T::zero(),
        }
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Copy, PartialEq)]
/// The user-defined external forces applied to this rigid-body.
pub struct RigidBodyForces {
    /// Accumulation of external forces (only for dynamic bodies).
    pub force: Vector,
    /// Accumulation of external torques (only for dynamic bodies).
    pub torque: AngVector,
    /// Gravity is multiplied by this scaling factor before it's
    /// applied to this rigid-body.
    pub gravity_scale: Real,
    /// Forces applied by the user.
    pub user_force: Vector,
    /// Torque applied by the user.
    pub user_torque: AngVector,
    /// Are gyroscopic forces enabled for this rigid-body?
    #[cfg(feature = "dim3")]
    pub gyroscopic_forces_enabled: bool,
}

impl Default for RigidBodyForces {
    fn default() -> Self {
        #[cfg(feature = "dim2")]
        return Self {
            force: Vector::ZERO,
            torque: 0.0,
            gravity_scale: 1.0,
            user_force: Vector::ZERO,
            user_torque: 0.0,
        };

        #[cfg(feature = "dim3")]
        return Self {
            force: Vector::ZERO,
            torque: AngVector::ZERO,
            gravity_scale: 1.0,
            user_force: Vector::ZERO,
            user_torque: AngVector::ZERO,
            gyroscopic_forces_enabled: false,
        };
    }
}

impl RigidBodyForces {
    /// Integrate these forces to compute new velocities.
    #[must_use]
    pub fn integrate(
        &self,
        dt: Real,
        init_vels: &RigidBodyVelocity<Real>,
        mprops: &RigidBodyMassProps,
    ) -> RigidBodyVelocity<Real> {
        let linear_acc = self.force * mprops.effective_inv_mass;
        let angular_acc = mprops.effective_world_inv_inertia * self.torque;

        RigidBodyVelocity {
            linvel: init_vels.linvel + linear_acc * dt,
            angvel: init_vels.angvel + angular_acc * dt,
        }
    }

    /// Adds to `self` the gravitational force that would result in a gravitational acceleration
    /// equal to `gravity`.
    pub fn compute_effective_force_and_torque(&mut self, gravity: Vector, mass: Vector) {
        self.force = self.user_force + gravity * mass * self.gravity_scale;
        self.torque = self.user_torque;
    }

    /// Applies a force at the given world-space point of the rigid-body with the given mass properties.
    pub fn apply_force_at_point(
        &mut self,
        rb_mprops: &RigidBodyMassProps,
        force: Vector,
        point: Vector,
    ) {
        self.user_force += force;
        self.user_torque += (point - rb_mprops.world_com).gcross(force);
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Copy, PartialEq)]
/// Information used for Continuous-Collision-Detection.
pub struct RigidBodyCcd {
    /// The distance used by the CCD solver to decide if a movement would
    /// result in a tunnelling problem.
    pub ccd_thickness: Real,
    /// The max distance between this rigid-body's center of mass and its
    /// furthest collider point.
    pub ccd_max_dist: Real,
    /// Is CCD active for this rigid-body?
    ///
    /// If `self.ccd_enabled` is `true`, then this is automatically set to
    /// `true` when the CCD solver detects that the rigid-body is moving fast
    /// enough to potential cause a tunneling problem.
    pub ccd_active: bool,
    /// Is CCD enabled for this rigid-body?
    pub ccd_enabled: bool,
    /// The soft-CCD prediction distance for this rigid-body.
    pub soft_ccd_prediction: Real,
}

impl Default for RigidBodyCcd {
    fn default() -> Self {
        Self {
            ccd_thickness: Real::MAX,
            ccd_max_dist: 0.0,
            ccd_active: false,
            ccd_enabled: false,
            soft_ccd_prediction: 0.0,
        }
    }
}

impl RigidBodyCcd {
    /// The maximum velocity any point of any collider attached to this rigid-body
    /// moving with the given velocity can have.
    pub fn max_point_velocity(&self, vels: &RigidBodyVelocity<Real>) -> Real {
        #[cfg(feature = "dim2")]
        return vels.linvel.length() + vels.angvel.abs() * self.ccd_max_dist;
        #[cfg(feature = "dim3")]
        return vels.linvel.length() + vels.angvel.length() * self.ccd_max_dist;
    }

    /// Is this rigid-body moving fast enough so that it may cause a tunneling problem?
    pub fn is_moving_fast(
        &self,
        dt: Real,
        vels: &RigidBodyVelocity<Real>,
        forces: Option<&RigidBodyForces>,
    ) -> bool {
        // NOTE: for the threshold we don't use the exact CCD thickness. Theoretically, we
        //       should use `self.rb_ccd.ccd_thickness - smallest_contact_dist` where `smallest_contact_dist`
        //       is the deepest contact (the contact with the largest penetration depth, i.e., the
        //       negative `dist` with the largest absolute value.
        //       However, getting this penetration depth assumes querying the contact graph from
        //       the narrow-phase, which can be pretty expensive. So we use the CCD thickness
        //       divided by 10 right now. We will see in practice if this value is OK or if we
        //       should use a smaller (to be less conservative) or larger divisor (to be more conservative).
        let threshold = self.ccd_thickness / 10.0;

        if let Some(forces) = forces {
            let linear_part = (vels.linvel + forces.force * dt).length();
            #[cfg(feature = "dim2")]
            let angular_part = (vels.angvel + forces.torque * dt).abs() * self.ccd_max_dist;
            #[cfg(feature = "dim3")]
            let angular_part = (vels.angvel + forces.torque * dt).length() * self.ccd_max_dist;
            let vel_with_forces = linear_part + angular_part;
            vel_with_forces > threshold
        } else {
            self.max_point_velocity(vels) * dt > threshold
        }
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug, Copy, PartialEq, Eq, Hash)]
/// Internal identifiers used by the physics engine.
pub struct RigidBodyIds {
    pub(crate) active_island_id: usize,
    pub(crate) active_set_id: usize,
    pub(crate) active_set_timestamp: u32,
}

impl Default for RigidBodyIds {
    fn default() -> Self {
        Self {
            active_island_id: usize::MAX,
            active_set_id: usize::MAX,
            active_set_timestamp: 0,
        }
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Default, Clone, Debug, PartialEq, Eq)]
/// The set of colliders attached to this rigid-bodies.
///
/// This should not be modified manually unless you really know what
/// you are doing (for example if you are trying to integrate Rapier
/// to a game engine using its component-based interface).
pub struct RigidBodyColliders(pub Vec<ColliderHandle>);

impl RigidBodyColliders {
    /// Detach a collider from this rigid-body.
    pub fn detach_collider(
        &mut self,
        rb_changes: &mut RigidBodyChanges,
        co_handle: ColliderHandle,
    ) {
        if let Some(i) = self.0.iter().position(|e| *e == co_handle) {
            rb_changes.set(RigidBodyChanges::COLLIDERS, true);
            self.0.swap_remove(i);
        }
    }

    /// Attach a collider to this rigid-body.
    pub fn attach_collider(
        &mut self,
        rb_type: RigidBodyType,
        rb_changes: &mut RigidBodyChanges,
        rb_ccd: &mut RigidBodyCcd,
        rb_mprops: &mut RigidBodyMassProps,
        rb_pos: &RigidBodyPosition,
        co_handle: ColliderHandle,
        co_pos: &mut ColliderPosition,
        co_parent: &ColliderParent,
        co_shape: &ColliderShape,
        co_mprops: &ColliderMassProps,
    ) {
        rb_changes.set(RigidBodyChanges::COLLIDERS, true);

        co_pos.0 = rb_pos.position * co_parent.pos_wrt_parent;
        rb_ccd.ccd_thickness = rb_ccd.ccd_thickness.min(co_shape.ccd_thickness());

        let shape_bsphere = co_shape.compute_bounding_sphere(&co_parent.pos_wrt_parent);
        rb_ccd.ccd_max_dist = rb_ccd
            .ccd_max_dist
            .max(shape_bsphere.center.length() + shape_bsphere.radius);

        let mass_properties = co_mprops
            .mass_properties(&**co_shape)
            .transform_by(&co_parent.pos_wrt_parent);
        self.0.push(co_handle);
        rb_mprops.local_mprops += mass_properties;
        rb_mprops.update_world_mass_properties(rb_type, &rb_pos.position);
    }

    /// Update the positions of all the colliders attached to this rigid-body.
    pub(crate) fn update_positions(
        &self,
        colliders: &mut ColliderSet,
        modified_colliders: &mut ModifiedColliders,
        parent_pos: &Pose,
    ) {
        for handle in &self.0 {
            // NOTE: the ColliderParent component must exist if we enter this method.
            // NOTE: currently, we are propagating the position even if the collider is disabled.
            //       Is that the best behavior?
            let co = colliders.index_mut_internal(*handle);
            let new_pos = parent_pos * co.parent.as_ref().unwrap().pos_wrt_parent;

            // Set the modification flag so we can benefit from the modification-tracking
            // when updating the narrow-phase/broad-phase afterwards.
            modified_colliders.push_once(*handle, co);

            co.changes |= ColliderChanges::POSITION;
            co.pos = ColliderPosition(new_pos);
        }
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Default, Clone, Debug, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// The dominance groups of a rigid-body.
pub struct RigidBodyDominance(pub i8);

impl RigidBodyDominance {
    /// The actual dominance group of this rigid-body, after taking into account its type.
    pub fn effective_group(&self, status: &RigidBodyType) -> i16 {
        if status.is_dynamic_or_kinematic() {
            self.0 as i16
        } else {
            i8::MAX as i16 + 1
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub(crate) enum SleepRootState {
    /// This sleep root has already been traversed. No need to traverse
    /// again until the rigid-body either gets awaken by an event.
    Traversed,
    /// This sleep root has not been traversed yet.
    TraversalPending,
    /// This body can become a sleep root once it falls asleep.
    #[default]
    Unknown,
}

/// Controls when a body goes to sleep (becomes inactive to save CPU).
///
/// ## Sleeping System
///
/// Bodies automatically sleep when they're at rest, dramatically improving performance
/// in scenes with many inactive objects. Sleeping bodies are:
/// - Excluded from simulation (no collision detection, no velocity integration)
/// - Automatically woken when disturbed (hit by moving object, connected via joint)
/// - Woken manually with `body.wake_up()` or `islands.wake_up()`
///
/// ## How sleeping works
///
/// A body sleeps after its linear AND angular velocities stay below thresholds for
/// `time_until_sleep` seconds (default: 2 seconds). Set thresholds to negative to disable sleeping.
///
/// ## When to disable sleeping
///
/// Most bodies should sleep! Only disable if the body needs to stay active despite being still:
/// - Bodies you frequently query for raycasts/contacts
/// - Bodies with time-based behaviors while stationary
///
/// Use `RigidBodyBuilder::can_sleep(false)` or `RigidBodyActivation::cannot_sleep()`.
#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub struct RigidBodyActivation {
    /// Linear velocity threshold for sleeping (scaled by `length_unit`).
    ///
    /// If negative, body never sleeps. Default: 0.4 (in length units/second).
    pub normalized_linear_threshold: Real,

    /// Angular velocity threshold for sleeping (radians/second).
    ///
    /// If negative, body never sleeps. Default: 0.5 rad/s.
    pub angular_threshold: Real,

    /// How long the body must be still before sleeping (seconds).
    ///
    /// Default: 2.0 seconds. Must be below both velocity thresholds for this duration.
    pub time_until_sleep: Real,

    /// Internal timer tracking how long body has been still.
    pub time_since_can_sleep: Real,

    /// Is this body currently sleeping?
    pub sleeping: bool,

    pub(crate) sleep_root_state: SleepRootState,
}

impl Default for RigidBodyActivation {
    fn default() -> Self {
        Self::active()
    }
}

impl RigidBodyActivation {
    /// The default linear velocity below which a body can be put to sleep.
    pub fn default_normalized_linear_threshold() -> Real {
        0.4
    }

    /// The default angular velocity below which a body can be put to sleep.
    pub fn default_angular_threshold() -> Real {
        0.5
    }

    /// The amount of time the rigid-body must remain below it’s linear and angular velocity
    /// threshold before falling to sleep.
    pub fn default_time_until_sleep() -> Real {
        2.0
    }

    /// Create a new rb_activation status initialised with the default rb_activation threshold and is active.
    pub fn active() -> Self {
        RigidBodyActivation {
            normalized_linear_threshold: Self::default_normalized_linear_threshold(),
            angular_threshold: Self::default_angular_threshold(),
            time_until_sleep: Self::default_time_until_sleep(),
            time_since_can_sleep: 0.0,
            sleeping: false,
            sleep_root_state: SleepRootState::Unknown,
        }
    }

    /// Create a new rb_activation status initialised with the default rb_activation threshold and is inactive.
    pub fn inactive() -> Self {
        RigidBodyActivation {
            normalized_linear_threshold: Self::default_normalized_linear_threshold(),
            angular_threshold: Self::default_angular_threshold(),
            time_until_sleep: Self::default_time_until_sleep(),
            time_since_can_sleep: Self::default_time_until_sleep(),
            sleeping: true,
            sleep_root_state: SleepRootState::Unknown,
        }
    }

    /// Create a new activation status that prevents the rigid-body from sleeping.
    pub fn cannot_sleep() -> Self {
        RigidBodyActivation {
            normalized_linear_threshold: -1.0,
            angular_threshold: -1.0,
            ..Self::active()
        }
    }

    /// Returns `true` if the body is not asleep.
    #[inline]
    pub fn is_active(&self) -> bool {
        !self.sleeping
    }

    /// Wakes up this rigid-body.
    #[inline]
    pub fn wake_up(&mut self, strong: bool) {
        self.sleeping = false;

        // Make this body eligible as a sleep root again.
        if self.sleep_root_state != SleepRootState::TraversalPending {
            self.sleep_root_state = SleepRootState::Unknown;
        }

        if strong {
            self.time_since_can_sleep = 0.0;
        }
    }

    /// Put this rigid-body to sleep.
    #[inline]
    pub fn sleep(&mut self) {
        self.sleeping = true;
        self.time_since_can_sleep = self.time_until_sleep;
    }

    /// Does this body have a sufficiently low kinetic energy for a long enough
    /// duration to be eligible for sleeping?
    pub fn is_eligible_for_sleep(&self) -> bool {
        self.time_since_can_sleep >= self.time_until_sleep
    }

    pub(crate) fn update_energy(
        &mut self,
        body_type: RigidBodyType,
        length_unit: Real,
        sq_linvel: Real,
        sq_angvel: Real,
        dt: Real,
    ) {
        let can_sleep = match body_type {
            RigidBodyType::Dynamic => {
                let linear_threshold = self.normalized_linear_threshold * length_unit;
                sq_linvel < linear_threshold * linear_threshold.abs()
                    && sq_angvel < self.angular_threshold * self.angular_threshold.abs()
            }
            RigidBodyType::KinematicPositionBased | RigidBodyType::KinematicVelocityBased => {
                // Platforms only sleep if both velocities are exactly zero. If it’s not exactly
                // zero, then the user really wants them to move.
                sq_linvel == 0.0 && sq_angvel == 0.0
            }
            RigidBodyType::Fixed => true,
        };

        if can_sleep {
            self.time_since_can_sleep += dt;
        } else {
            self.time_since_can_sleep = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Real;

    #[test]
    fn test_interpolate_velocity() {
        // Interpolate and then integrate the velocity to see if
        // the end positions match.
        #[cfg(feature = "f32")]
        let mut rng = oorandom::Rand32::new(0);
        #[cfg(feature = "f64")]
        let mut rng = oorandom::Rand64::new(0);

        for i in -10..=10 {
            let mult = i as Real;
            let (local_com, curr_pos, next_pos);
            #[cfg(feature = "dim2")]
            {
                local_com = Vector::new(rng.rand_float(), rng.rand_float());
                curr_pos = Pose::new(
                    Vector::new(rng.rand_float(), rng.rand_float()) * mult,
                    rng.rand_float(),
                );
                next_pos = Pose::new(
                    Vector::new(rng.rand_float(), rng.rand_float()) * mult,
                    rng.rand_float(),
                );
            }
            #[cfg(feature = "dim3")]
            {
                local_com = Vector::new(rng.rand_float(), rng.rand_float(), rng.rand_float());
                curr_pos = Pose::new(
                    Vector::new(rng.rand_float(), rng.rand_float(), rng.rand_float()) * mult,
                    Vector::new(rng.rand_float(), rng.rand_float(), rng.rand_float()),
                );
                next_pos = Pose::new(
                    Vector::new(rng.rand_float(), rng.rand_float(), rng.rand_float()) * mult,
                    Vector::new(rng.rand_float(), rng.rand_float(), rng.rand_float()),
                );
            }

            let dt = 0.016;
            let rb_pos = RigidBodyPosition {
                position: curr_pos,
                next_position: next_pos,
            };
            let vel = rb_pos.interpolate_velocity(1.0 / dt, local_com);
            let interp_pos = vel.integrate(dt, &curr_pos, &local_com);
            approx::assert_relative_eq!(interp_pos, next_pos, epsilon = 1.0e-5);
        }
    }
}
