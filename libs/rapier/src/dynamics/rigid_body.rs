#[cfg(doc)]
use super::IntegrationParameters;
use crate::dynamics::{
    LockedAxes, MassProperties, RigidBodyActivation, RigidBodyAdditionalMassProps, RigidBodyCcd,
    RigidBodyChanges, RigidBodyColliders, RigidBodyDamping, RigidBodyDominance, RigidBodyForces,
    RigidBodyIds, RigidBodyMassProps, RigidBodyPosition, RigidBodyType, RigidBodyVelocity,
};
use crate::geometry::{
    ColliderHandle, ColliderMassProps, ColliderParent, ColliderPosition, ColliderSet, ColliderShape,
};
use crate::math::{AngVector, Pose, Real, Rotation, Vector, rotation_from_angle};
use crate::utils::CrossProduct;

#[cfg(feature = "dim2")]
use crate::num::Zero;

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
/// A physical object that can move, rotate, and collide with other objects in your simulation.
///
/// Rigid bodies are the fundamental moving objects in physics simulations. Think of them as
/// the "physical representation" of your game objects - a character, a crate, a vehicle, etc.
///
/// ## Body types
///
/// - **Dynamic**: Affected by forces, gravity, and collisions. Use for objects that should move realistically (falling boxes, projectiles, etc.)
/// - **Fixed**: Never moves. Use for static geometry like walls, floors, and terrain
/// - **Kinematic**: Moved by setting velocity or position directly, not by forces. Use for moving platforms, doors, or player-controlled characters
///
/// ## Creating bodies
///
/// Always use [`RigidBodyBuilder`] to create new rigid bodies:
///
/// ```
/// # use rapier3d::prelude::*;
/// # let mut bodies = RigidBodySet::new();
/// let body = RigidBodyBuilder::dynamic()
///     .translation(Vector::new(0.0, 10.0, 0.0))
///     .build();
/// let handle = bodies.insert(body);
/// ```
#[derive(Debug, Clone)]
// #[repr(C)]
// #[repr(align(64))]
pub struct RigidBody {
    pub(crate) ids: RigidBodyIds,
    pub(crate) pos: RigidBodyPosition,
    pub(crate) damping: RigidBodyDamping<Real>,
    pub(crate) vels: RigidBodyVelocity<Real>,
    pub(crate) forces: RigidBodyForces,
    pub(crate) mprops: RigidBodyMassProps,

    pub(crate) ccd_vels: RigidBodyVelocity<Real>,
    pub(crate) ccd: RigidBodyCcd,
    pub(crate) colliders: RigidBodyColliders,
    /// Whether or not this rigid-body is sleeping.
    pub(crate) activation: RigidBodyActivation,
    pub(crate) changes: RigidBodyChanges,
    /// The status of the body, governing how it is affected by external forces.
    pub(crate) body_type: RigidBodyType,
    /// The dominance group this rigid-body is part of.
    pub(crate) dominance: RigidBodyDominance,
    pub(crate) enabled: bool,
    pub(crate) additional_solver_iterations: usize,
    /// User-defined data associated to this rigid-body.
    pub user_data: u128,
}

impl Default for RigidBody {
    fn default() -> Self {
        Self::new()
    }
}

impl RigidBody {
    fn new() -> Self {
        Self {
            pos: RigidBodyPosition::default(),
            mprops: RigidBodyMassProps::default(),
            ccd_vels: RigidBodyVelocity::default(),
            vels: RigidBodyVelocity::default(),
            damping: RigidBodyDamping::default(),
            forces: RigidBodyForces::default(),
            ccd: RigidBodyCcd::default(),
            ids: RigidBodyIds::default(),
            colliders: RigidBodyColliders::default(),
            activation: RigidBodyActivation::active(),
            changes: RigidBodyChanges::all(),
            body_type: RigidBodyType::Dynamic,
            dominance: RigidBodyDominance::default(),
            enabled: true,
            user_data: 0,
            additional_solver_iterations: 0,
        }
    }

    pub(crate) fn reset_internal_references(&mut self) {
        self.colliders.0 = Vec::new();
        self.ids = Default::default();
    }

    /// Copy all the characteristics from `other` to `self`.
    ///
    /// If you have a mutable reference to a rigid-body `rigid_body: &mut RigidBody`, attempting to
    /// assign it a whole new rigid-body instance, e.g., `*rigid_body = RigidBodyBuilder::dynamic().build()`,
    /// will crash due to some internal indices being overwritten. Instead, use
    /// `rigid_body.copy_from(&RigidBodyBuilder::dynamic().build())`.
    ///
    /// This method will allow you to set most characteristics of this rigid-body from another
    /// rigid-body instance without causing any breakage.
    ///
    /// This method **cannot** be used for editing the list of colliders attached to this rigid-body.
    /// Therefore, the list of colliders attached to `self` won’t be replaced by the one attached
    /// to `other`.
    ///
    /// The pose of `other` will only copied into `self` if `self` doesn’t have a parent (if it has
    /// a parent, its position is directly controlled by the parent rigid-body).
    pub fn copy_from(&mut self, other: &RigidBody) {
        // NOTE: we deconstruct the rigid-body struct to be sure we don’t forget to
        //       add some copies here if we add more field to RigidBody in the future.
        let RigidBody {
            pos,
            mprops,
            ccd_vels: integrated_vels,
            vels,
            damping,
            forces,
            ccd,
            ids: _ids,             // Internal ids must not be overwritten.
            colliders: _colliders, // This function cannot be used to edit collider sets.
            activation,
            changes: _changes, // Will be set to ALL.
            body_type,
            dominance,
            enabled,
            additional_solver_iterations,
            user_data,
        } = other;

        self.pos = *pos;
        self.mprops = mprops.clone();
        self.ccd_vels = *integrated_vels;
        self.vels = *vels;
        self.damping = *damping;
        self.forces = *forces;
        self.ccd = *ccd;
        self.activation = *activation;
        self.body_type = *body_type;
        self.dominance = *dominance;
        self.enabled = *enabled;
        self.additional_solver_iterations = *additional_solver_iterations;
        self.user_data = *user_data;

        self.changes = RigidBodyChanges::all();
    }

    /// Set the additional number of solver iterations run for this rigid-body and
    /// everything interacting with it.
    ///
    /// See [`Self::set_additional_solver_iterations`] for additional information.
    pub fn additional_solver_iterations(&self) -> usize {
        self.additional_solver_iterations
    }

    /// Set the additional number of solver iterations run for this rigid-body and
    /// everything interacting with it.
    ///
    /// Increasing this number will help improve simulation accuracy on this rigid-body
    /// and every rigid-body interacting directly or indirectly with it (through joints
    /// or contacts). This implies a performance hit.
    ///
    /// The default value is 0, meaning exactly [`IntegrationParameters::num_solver_iterations`] will
    /// be used as number of solver iterations for this body.
    pub fn set_additional_solver_iterations(&mut self, additional_iterations: usize) {
        self.additional_solver_iterations = additional_iterations;
    }

    /// The activation status of this rigid-body.
    pub fn activation(&self) -> &RigidBodyActivation {
        &self.activation
    }

    /// Mutable reference to the activation status of this rigid-body.
    pub fn activation_mut(&mut self) -> &mut RigidBodyActivation {
        self.changes |= RigidBodyChanges::SLEEP;
        &mut self.activation
    }

    /// Is this rigid-body enabled?
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Sets whether this rigid-body is enabled or not.
    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled != self.enabled {
            if enabled {
                // NOTE: this is probably overkill, but it makes sure we don’t
                // forget anything that needs to be updated because the rigid-body
                // was basically interpreted as if it was removed while it was
                // disabled.
                self.changes = RigidBodyChanges::all();
            } else {
                self.changes |= RigidBodyChanges::ENABLED_OR_DISABLED;
            }

            self.enabled = enabled;
        }
    }

    /// The linear damping coefficient (velocity reduction over time).
    ///
    /// Damping gradually slows down moving objects. `0.0` = no damping (infinite momentum),
    /// higher values = faster slowdown. Use for air resistance, friction, etc.
    #[inline]
    pub fn linear_damping(&self) -> Real {
        self.damping.linear_damping
    }

    /// Sets how quickly linear velocity decreases over time.
    ///
    /// - `0.0` = no slowdown (space/frictionless)
    /// - `0.1` = gradual slowdown (air resistance)
    /// - `1.0+` = rapid slowdown (thick fluid)
    #[inline]
    pub fn set_linear_damping(&mut self, damping: Real) {
        self.damping.linear_damping = damping;
    }

    /// The angular damping coefficient (rotation slowdown over time).
    ///
    /// Like linear damping but for rotation. Higher values make spinning objects stop faster.
    #[inline]
    pub fn angular_damping(&self) -> Real {
        self.damping.angular_damping
    }

    /// Sets how quickly angular velocity decreases over time.
    ///
    /// Controls how fast spinning objects slow down.
    #[inline]
    pub fn set_angular_damping(&mut self, damping: Real) {
        self.damping.angular_damping = damping
    }

    /// The type of this rigid-body.
    pub fn body_type(&self) -> RigidBodyType {
        self.body_type
    }

    /// Sets the type of this rigid-body.
    pub fn set_body_type(&mut self, status: RigidBodyType, wake_up: bool) {
        if status != self.body_type {
            self.changes.insert(RigidBodyChanges::TYPE);
            self.body_type = status;

            if status == RigidBodyType::Fixed {
                self.vels = RigidBodyVelocity::zero();
            }

            if self.is_dynamic_or_kinematic() && wake_up {
                self.wake_up(true);
            }
        }
    }

    /// The center of mass position in world coordinates.
    ///
    /// This is the "balance point" where the body's mass is centered. Forces applied here
    /// produce no rotation, only translation.
    #[inline]
    pub fn center_of_mass(&self) -> Vector {
        self.mprops.world_com
    }

    /// The center of mass in the body's local coordinate system.
    ///
    /// This is relative to the body's position, computed from attached colliders.
    #[inline]
    pub fn local_center_of_mass(&self) -> Vector {
        self.mprops.local_mprops.local_com
    }

    /// The mass-properties of this rigid-body.
    #[inline]
    pub fn mass_properties(&self) -> &RigidBodyMassProps {
        &self.mprops
    }

    /// The dominance group of this rigid-body.
    ///
    /// This method always returns `i8::MAX + 1` for non-dynamic
    /// rigid-bodies.
    #[inline]
    pub fn effective_dominance_group(&self) -> i16 {
        self.dominance.effective_group(&self.body_type)
    }

    /// Sets the axes along which this rigid-body cannot translate or rotate.
    #[inline]
    pub fn set_locked_axes(&mut self, locked_axes: LockedAxes, wake_up: bool) {
        if locked_axes != self.mprops.flags {
            if self.is_dynamic_or_kinematic() && wake_up {
                self.wake_up(true);
            }

            self.mprops.flags = locked_axes;
            self.update_world_mass_properties();
        }
    }

    /// The axes along which this rigid-body cannot translate or rotate.
    #[inline]
    pub fn locked_axes(&self) -> LockedAxes {
        self.mprops.flags
    }

    /// Locks or unlocks all rotational movement for this body.
    ///
    /// When locked, the body cannot rotate at all (useful for keeping objects upright).
    /// Use for characters that shouldn't tip over, or objects that should only slide.
    #[inline]
    pub fn lock_rotations(&mut self, locked: bool, wake_up: bool) {
        if locked != self.mprops.flags.contains(LockedAxes::ROTATION_LOCKED) {
            if self.is_dynamic_or_kinematic() && wake_up {
                self.wake_up(true);
            }

            self.mprops.flags.set(LockedAxes::ROTATION_LOCKED_X, locked);
            self.mprops.flags.set(LockedAxes::ROTATION_LOCKED_Y, locked);
            self.mprops.flags.set(LockedAxes::ROTATION_LOCKED_Z, locked);
            self.update_world_mass_properties();
        }
    }

    #[inline]
    /// Locks or unlocks rotations of this rigid-body along each cartesian axes.
    pub fn set_enabled_rotations(
        &mut self,
        allow_rotations_x: bool,
        allow_rotations_y: bool,
        allow_rotations_z: bool,
        wake_up: bool,
    ) {
        if self.mprops.flags.contains(LockedAxes::ROTATION_LOCKED_X) == allow_rotations_x
            || self.mprops.flags.contains(LockedAxes::ROTATION_LOCKED_Y) == allow_rotations_y
            || self.mprops.flags.contains(LockedAxes::ROTATION_LOCKED_Z) == allow_rotations_z
        {
            if self.is_dynamic_or_kinematic() && wake_up {
                self.wake_up(true);
            }

            self.mprops
                .flags
                .set(LockedAxes::ROTATION_LOCKED_X, !allow_rotations_x);
            self.mprops
                .flags
                .set(LockedAxes::ROTATION_LOCKED_Y, !allow_rotations_y);
            self.mprops
                .flags
                .set(LockedAxes::ROTATION_LOCKED_Z, !allow_rotations_z);
            self.update_world_mass_properties();
        }
    }

    /// Locks or unlocks rotations of this rigid-body along each cartesian axes.
    #[deprecated(note = "Use `set_enabled_rotations` instead")]
    pub fn restrict_rotations(
        &mut self,
        allow_rotations_x: bool,
        allow_rotations_y: bool,
        allow_rotations_z: bool,
        wake_up: bool,
    ) {
        self.set_enabled_rotations(
            allow_rotations_x,
            allow_rotations_y,
            allow_rotations_z,
            wake_up,
        );
    }

    /// Locks or unlocks all translational movement for this body.
    ///
    /// When locked, the body cannot move from its position (but can still rotate).
    /// Use for rotating platforms, turrets, or objects fixed in space.
    #[inline]
    pub fn lock_translations(&mut self, locked: bool, wake_up: bool) {
        if locked != self.mprops.flags.contains(LockedAxes::TRANSLATION_LOCKED) {
            if self.is_dynamic_or_kinematic() && wake_up {
                self.wake_up(true);
            }

            self.mprops
                .flags
                .set(LockedAxes::TRANSLATION_LOCKED, locked);
            self.update_world_mass_properties();
        }
    }

    #[inline]
    /// Locks or unlocks rotations of this rigid-body along each cartesian axes.
    pub fn set_enabled_translations(
        &mut self,
        allow_translation_x: bool,
        allow_translation_y: bool,
        #[cfg(feature = "dim3")] allow_translation_z: bool,
        wake_up: bool,
    ) {
        #[cfg(feature = "dim2")]
        if self.mprops.flags.contains(LockedAxes::TRANSLATION_LOCKED_X) != allow_translation_x
            && self.mprops.flags.contains(LockedAxes::TRANSLATION_LOCKED_Y) != allow_translation_y
        {
            // Nothing to change.
            return;
        }
        #[cfg(feature = "dim3")]
        if self.mprops.flags.contains(LockedAxes::TRANSLATION_LOCKED_X) != allow_translation_x
            && self.mprops.flags.contains(LockedAxes::TRANSLATION_LOCKED_Y) != allow_translation_y
            && self.mprops.flags.contains(LockedAxes::TRANSLATION_LOCKED_Z) != allow_translation_z
        {
            // Nothing to change.
            return;
        }

        if self.is_dynamic_or_kinematic() && wake_up {
            self.wake_up(true);
        }

        self.mprops
            .flags
            .set(LockedAxes::TRANSLATION_LOCKED_X, !allow_translation_x);
        self.mprops
            .flags
            .set(LockedAxes::TRANSLATION_LOCKED_Y, !allow_translation_y);
        #[cfg(feature = "dim3")]
        self.mprops
            .flags
            .set(LockedAxes::TRANSLATION_LOCKED_Z, !allow_translation_z);
        self.update_world_mass_properties();
    }

    #[inline]
    #[deprecated(note = "Use `set_enabled_translations` instead")]
    /// Locks or unlocks rotations of this rigid-body along each cartesian axes.
    pub fn restrict_translations(
        &mut self,
        allow_translation_x: bool,
        allow_translation_y: bool,
        #[cfg(feature = "dim3")] allow_translation_z: bool,
        wake_up: bool,
    ) {
        self.set_enabled_translations(
            allow_translation_x,
            allow_translation_y,
            #[cfg(feature = "dim3")]
            allow_translation_z,
            wake_up,
        )
    }

    /// Are the translations of this rigid-body locked?
    #[cfg(feature = "dim2")]
    pub fn is_translation_locked(&self) -> bool {
        self.mprops
            .flags
            .contains(LockedAxes::TRANSLATION_LOCKED_X | LockedAxes::TRANSLATION_LOCKED_Y)
    }

    /// Are the translations of this rigid-body locked?
    #[cfg(feature = "dim3")]
    pub fn is_translation_locked(&self) -> bool {
        self.mprops.flags.contains(LockedAxes::TRANSLATION_LOCKED)
    }

    /// Are the rotations of this rigid-body locked?
    #[cfg(feature = "dim2")]
    pub fn is_rotation_locked(&self) -> bool {
        self.mprops.flags.contains(LockedAxes::ROTATION_LOCKED_Z)
    }

    /// Returns `true` for each rotational degrees of freedom locked on this rigid-body.
    #[cfg(feature = "dim3")]
    pub fn is_rotation_locked(&self) -> [bool; 3] {
        [
            self.mprops.flags.contains(LockedAxes::ROTATION_LOCKED_X),
            self.mprops.flags.contains(LockedAxes::ROTATION_LOCKED_Y),
            self.mprops.flags.contains(LockedAxes::ROTATION_LOCKED_Z),
        ]
    }

    /// Enables or disables Continuous Collision Detection for this body.
    ///
    /// CCD prevents fast-moving objects from tunneling through thin walls, but costs more CPU.
    /// Enable for bullets, fast projectiles, or any object that must never pass through geometry.
    pub fn enable_ccd(&mut self, enabled: bool) {
        self.ccd.ccd_enabled = enabled;
    }

    /// Checks if CCD is enabled for this body.
    ///
    /// Returns `true` if CCD is turned on (not whether it's currently active this frame).
    pub fn is_ccd_enabled(&self) -> bool {
        self.ccd.ccd_enabled
    }

    /// Sets the maximum prediction distance Soft Continuous Collision-Detection.
    ///
    /// When set to 0, soft-CCD is disabled. Soft-CCD helps prevent tunneling especially of
    /// slow-but-thin to moderately fast objects. The soft CCD prediction distance indicates how
    /// far in the object’s path the CCD algorithm is allowed to inspect. Large values can impact
    /// performance badly by increasing the work needed from the broad-phase.
    ///
    /// It is a generally cheaper variant of regular CCD (that can be enabled with
    /// [`RigidBody::enable_ccd`] since it relies on predictive constraints instead of
    /// shape-cast and substeps.
    pub fn set_soft_ccd_prediction(&mut self, prediction_distance: Real) {
        self.ccd.soft_ccd_prediction = prediction_distance;
    }

    /// The soft-CCD prediction distance for this rigid-body.
    ///
    /// See the documentation of [`RigidBody::set_soft_ccd_prediction`] for additional details on
    /// soft-CCD.
    pub fn soft_ccd_prediction(&self) -> Real {
        self.ccd.soft_ccd_prediction
    }

    // This is different from `is_ccd_enabled`. This checks that CCD
    // is active for this rigid-body, i.e., if it was seen to move fast
    // enough to justify a CCD run.
    /// Is CCD active for this rigid-body?
    ///
    /// The CCD is considered active if the rigid-body is moving at
    /// a velocity greater than an automatically-computed threshold.
    ///
    /// This is not the same as `self.is_ccd_enabled` which only
    /// checks if CCD is enabled to run for this rigid-body or if
    /// it is completely disabled (independently from its velocity).
    pub fn is_ccd_active(&self) -> bool {
        self.ccd.ccd_active
    }

    /// Recalculates mass, center of mass, and inertia from attached colliders.
    ///
    /// Normally automatic, but call this if you modify collider shapes/masses at runtime.
    /// Only needed after directly modifying colliders without going through the builder.
    pub fn recompute_mass_properties_from_colliders(&mut self, colliders: &ColliderSet) {
        self.mprops.recompute_mass_properties_from_colliders(
            colliders,
            &self.colliders,
            self.body_type,
            &self.pos.position,
        );
    }

    /// Adds extra mass on top of collider-computed mass.
    ///
    /// Total mass = collider masses + this additional mass. Use when you want to make
    /// a body heavier without changing collider densities.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let body = bodies.insert(RigidBodyBuilder::dynamic());
    /// // Add 50kg to make this body heavier
    /// bodies[body].set_additional_mass(50.0, true);
    /// ```
    ///
    /// Angular inertia is automatically scaled to match the mass increase.
    /// Updated automatically at next physics step or call `recompute_mass_properties_from_colliders()`.
    #[inline]
    pub fn set_additional_mass(&mut self, additional_mass: Real, wake_up: bool) {
        self.do_set_additional_mass_properties(
            RigidBodyAdditionalMassProps::Mass(additional_mass),
            wake_up,
        )
    }

    /// Sets the rigid-body's additional mass-properties.
    ///
    /// This is only the "additional" mass-properties because the total mass-properties of the
    /// rigid-body is equal to the sum of this additional mass-properties and the mass computed from
    /// the colliders (with non-zero densities) attached to this rigid-body.
    ///
    /// That total mass-properties (which include the attached colliders’ contributions)
    /// will be updated at the name physics step, or can be updated manually with
    /// [`Self::recompute_mass_properties_from_colliders`].
    ///
    /// This will override any previous mass-properties set by [`Self::set_additional_mass`],
    /// [`Self::set_additional_mass_properties`], [`RigidBodyBuilder::additional_mass`], or
    /// [`RigidBodyBuilder::additional_mass_properties`] for this rigid-body.
    ///
    /// If `wake_up` is `true` then the rigid-body will be woken up if it was
    /// put to sleep because it did not move for a while.
    #[inline]
    pub fn set_additional_mass_properties(&mut self, props: MassProperties, wake_up: bool) {
        self.do_set_additional_mass_properties(
            RigidBodyAdditionalMassProps::MassProps(props),
            wake_up,
        )
    }

    fn do_set_additional_mass_properties(
        &mut self,
        props: RigidBodyAdditionalMassProps,
        wake_up: bool,
    ) {
        let new_mprops = Some(Box::new(props));

        if self.mprops.additional_local_mprops != new_mprops {
            self.changes.insert(RigidBodyChanges::LOCAL_MASS_PROPERTIES);
            self.mprops.additional_local_mprops = new_mprops;

            if self.is_dynamic_or_kinematic() && wake_up {
                self.wake_up(true);
            }
        }
    }

    /// Returns handles of all colliders attached to this body.
    ///
    /// Use to iterate over a body's collision shapes or to modify them.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut colliders = ColliderSet::new();
    /// # let body = bodies.insert(RigidBodyBuilder::dynamic());
    /// # colliders.insert_with_parent(ColliderBuilder::ball(0.5), body, &mut bodies);
    /// for collider_handle in bodies[body].colliders() {
    ///     if let Some(collider) = colliders.get_mut(*collider_handle) {
    ///         collider.set_friction(0.5);
    ///     }
    /// }
    /// ```
    pub fn colliders(&self) -> &[ColliderHandle] {
        &self.colliders.0[..]
    }

    /// Checks if this is a dynamic body (moves via forces and collisions).
    ///
    /// Dynamic bodies are fully simulated and respond to gravity, forces, and collisions.
    pub fn is_dynamic(&self) -> bool {
        self.body_type == RigidBodyType::Dynamic
    }

    /// Checks if this is a kinematic body (moves via direct velocity/position control).
    ///
    /// Kinematic bodies move by setting velocity directly, not by applying forces.
    pub fn is_kinematic(&self) -> bool {
        self.body_type.is_kinematic()
    }

    /// Is this rigid-body a dynamic rigid-body or a kinematic rigid-body?
    ///
    /// This method is mostly convenient internally where kinematic and dynamic rigid-body
    /// are subject to the same behavior.
    pub fn is_dynamic_or_kinematic(&self) -> bool {
        self.body_type.is_dynamic_or_kinematic()
    }

    /// The offset index in the solver’s active set, or `u32::MAX` if
    /// the rigid-body isn’t dynamic or kinematic.
    // TODO: is this really necessary? Could we just always assign u32::MAX
    //       to all the fixed bodies active set offsets?
    pub fn effective_active_set_offset(&self) -> u32 {
        if self.is_dynamic_or_kinematic() {
            self.ids.active_set_id as u32
        } else {
            u32::MAX
        }
    }

    /// Checks if this is a fixed body (never moves, infinite mass).
    ///
    /// Fixed bodies are static geometry: walls, floors, terrain. They never move
    /// and are not affected by any forces or collisions.
    pub fn is_fixed(&self) -> bool {
        self.body_type == RigidBodyType::Fixed
    }

    /// The mass of this rigid body in kilograms.
    ///
    /// Returns zero for fixed bodies (which technically have infinite mass).
    /// Mass is computed from attached colliders' shapes and densities.
    pub fn mass(&self) -> Real {
        self.mprops.local_mprops.mass()
    }

    /// The predicted position of this rigid-body.
    ///
    /// If this rigid-body is kinematic this value is set by the `set_next_kinematic_position`
    /// method and is used for estimating the kinematic body velocity at the next timestep.
    /// For non-kinematic bodies, this value is currently unspecified.
    pub fn next_position(&self) -> &Pose {
        &self.pos.next_position
    }

    /// The gravity scale multiplier for this body.
    ///
    /// - `1.0` (default) = normal gravity
    /// - `0.0` = no gravity (floating)
    /// - `2.0` = double gravity (heavy/fast falling)
    /// - Negative values = reverse gravity (objects fall upward!)
    pub fn gravity_scale(&self) -> Real {
        self.forces.gravity_scale
    }

    /// Sets how much gravity affects this body (multiplier).
    ///
    /// # Examples
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let body = bodies.insert(RigidBodyBuilder::dynamic());
    /// bodies[body].set_gravity_scale(0.0, true);  // Zero-G (space)
    /// bodies[body].set_gravity_scale(0.1, true);  // Moon gravity
    /// bodies[body].set_gravity_scale(2.0, true);  // Extra heavy
    /// ```
    pub fn set_gravity_scale(&mut self, scale: Real, wake_up: bool) {
        if self.forces.gravity_scale != scale {
            if wake_up && self.activation.sleeping {
                self.changes.insert(RigidBodyChanges::SLEEP);
                self.activation.sleeping = false;
            }

            self.forces.gravity_scale = scale;
        }
    }

    /// The dominance group of this rigid-body.
    pub fn dominance_group(&self) -> i8 {
        self.dominance.0
    }

    /// The dominance group of this rigid-body.
    pub fn set_dominance_group(&mut self, dominance: i8) {
        if self.dominance.0 != dominance {
            self.changes.insert(RigidBodyChanges::DOMINANCE);
            self.dominance.0 = dominance
        }
    }

    /// Adds a collider to this rigid-body.
    pub(crate) fn add_collider_internal(
        &mut self,
        co_handle: ColliderHandle,
        co_parent: &ColliderParent,
        co_pos: &mut ColliderPosition,
        co_shape: &ColliderShape,
        co_mprops: &ColliderMassProps,
    ) {
        self.colliders.attach_collider(
            self.body_type,
            &mut self.changes,
            &mut self.ccd,
            &mut self.mprops,
            &self.pos,
            co_handle,
            co_pos,
            co_parent,
            co_shape,
            co_mprops,
        )
    }

    /// Removes a collider from this rigid-body.
    pub(crate) fn remove_collider_internal(&mut self, handle: ColliderHandle) {
        if let Some(i) = self.colliders.0.iter().position(|e| *e == handle) {
            self.changes.set(RigidBodyChanges::COLLIDERS, true);
            self.colliders.0.swap_remove(i);
        }
    }

    /// Forces this body to sleep immediately (stop simulating it).
    ///
    /// Sleeping bodies are excluded from physics simulation until disturbed. Use to manually
    /// deactivate bodies you know won't move for a while.
    ///
    /// The body will auto-wake if:
    /// - Hit by a moving object
    /// - Connected via joint to a moving body
    /// - Manually woken with `wake_up()`
    pub fn sleep(&mut self) {
        self.activation.sleep();
        self.vels = RigidBodyVelocity::zero();
    }

    /// Wakes up this body if it's sleeping, making it active in the simulation.
    ///
    /// # Parameters
    /// * `strong` - If `true`, guarantees the body stays awake for multiple frames.
    ///   If `false`, it might sleep again immediately if conditions are met.
    ///
    /// Use after manually moving a sleeping body or to keep it active temporarily.
    pub fn wake_up(&mut self, strong: bool) {
        if self.activation.sleeping {
            self.changes.insert(RigidBodyChanges::SLEEP);
        }

        self.activation.wake_up(strong);
    }

    /// Is this rigid body sleeping?
    pub fn is_sleeping(&self) -> bool {
        // TODO: should we:
        // - return false for fixed bodies.
        // - return true for non-sleeping dynamic bodies.
        // - return true only for kinematic bodies with non-zero velocity?
        self.activation.sleeping
    }

    /// Returns `true` if the body has non-zero linear or angular velocity.
    ///
    /// Useful for checking if an object is actually moving vs sitting still.
    pub fn is_moving(&self) -> bool {
        #[cfg(feature = "dim2")]
        let angvel_is_nonzero = self.vels.angvel != 0.0;
        #[cfg(feature = "dim3")]
        let angvel_is_nonzero = self.vels.angvel != Default::default();
        self.vels.linvel != Default::default() || angvel_is_nonzero
    }

    /// Returns both linear and angular velocity as a combined structure.
    ///
    /// Most users should use `linvel()` and `angvel()` separately instead.
    pub fn vels(&self) -> &RigidBodyVelocity<Real> {
        &self.vels
    }

    /// The current linear velocity (speed and direction of movement).
    ///
    /// This is how fast the body is moving in units per second. Use with [`set_linvel()`](Self::set_linvel)
    /// to directly control the body's movement speed.
    pub fn linvel(&self) -> Vector {
        self.vels.linvel
    }

    /// The current angular velocity (rotation speed) in 2D.
    ///
    /// Returns radians per second. Positive = counter-clockwise, negative = clockwise.
    #[cfg(feature = "dim2")]
    pub fn angvel(&self) -> Real {
        self.vels.angvel
    }

    /// The current angular velocity (rotation speed) in 3D.
    ///
    /// Returns a vector in radians per second around each axis (X, Y, Z).
    #[cfg(feature = "dim3")]
    pub fn angvel(&self) -> AngVector {
        self.vels.angvel
    }

    /// Set both the angular and linear velocity of this rigid-body.
    ///
    /// If `wake_up` is `true` then the rigid-body will be woken up if it was
    /// put to sleep because it did not move for a while.
    pub fn set_vels(&mut self, vels: RigidBodyVelocity<Real>, wake_up: bool) {
        self.set_linvel(vels.linvel, wake_up);
        #[cfg(feature = "dim2")]
        self.set_angvel(vels.angvel, wake_up);
        #[cfg(feature = "dim3")]
        self.set_angvel(vels.angvel, wake_up);
    }

    /// Sets how fast this body is moving (linear velocity).
    ///
    /// This directly sets the body's velocity without applying forces. Use for:
    /// - Player character movement
    /// - Kinematic object control
    /// - Instantly changing an object's speed
    ///
    /// For physics-based movement, consider using [`apply_impulse()`](Self::apply_impulse) or
    /// [`add_force()`](Self::add_force) instead for more realistic behavior.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let body = bodies.insert(RigidBodyBuilder::dynamic());
    /// // Make the body move to the right at 5 units/second
    /// bodies[body].set_linvel(Vector::new(5.0, 0.0, 0.0), true);
    /// ```
    pub fn set_linvel(&mut self, linvel: Vector, wake_up: bool) {
        if self.vels.linvel != linvel {
            match self.body_type {
                RigidBodyType::Dynamic | RigidBodyType::KinematicVelocityBased => {
                    self.vels.linvel = linvel;
                    if wake_up {
                        self.wake_up(true)
                    }
                }
                RigidBodyType::Fixed | RigidBodyType::KinematicPositionBased => {}
            }
        }
    }

    /// The angular velocity of this rigid-body.
    ///
    /// If `wake_up` is `true` then the rigid-body will be woken up if it was
    /// put to sleep because it did not move for a while.
    #[cfg(feature = "dim2")]
    pub fn set_angvel(&mut self, angvel: Real, wake_up: bool) {
        if self.vels.angvel != angvel {
            match self.body_type {
                RigidBodyType::Dynamic | RigidBodyType::KinematicVelocityBased => {
                    self.vels.angvel = angvel;
                    if wake_up {
                        self.wake_up(true)
                    }
                }
                RigidBodyType::Fixed | RigidBodyType::KinematicPositionBased => {}
            }
        }
    }

    /// The angular velocity of this rigid-body.
    ///
    /// If `wake_up` is `true` then the rigid-body will be woken up if it was
    /// put to sleep because it did not move for a while.
    #[cfg(feature = "dim3")]
    pub fn set_angvel(&mut self, angvel: AngVector, wake_up: bool) {
        if self.vels.angvel != angvel {
            match self.body_type {
                RigidBodyType::Dynamic | RigidBodyType::KinematicVelocityBased => {
                    self.vels.angvel = angvel;
                    if wake_up {
                        self.wake_up(true)
                    }
                }
                RigidBodyType::Fixed | RigidBodyType::KinematicPositionBased => {}
            }
        }
    }

    /// The current position (translation + rotation) of this rigid body in world space.
    ///
    /// Returns an `SimdPose` which combines both translation and rotation.
    /// For just the position vector, use [`translation()`](Self::translation) instead.
    #[inline]
    pub fn position(&self) -> &Pose {
        &self.pos.position
    }

    /// The current position vector of this rigid body (world coordinates).
    ///
    /// This is just the XYZ location, without rotation. For the full pose (position + rotation),
    /// use [`position()`](Self::position).
    #[inline]
    pub fn translation(&self) -> Vector {
        self.pos.position.translation
    }

    /// Teleports this rigid body to a new position (world coordinates).
    ///
    /// ⚠️ **Warning**: This instantly moves the body, ignoring physics! The body will "teleport"
    /// without checking for collisions in between. Use this for:
    /// - Respawning objects
    /// - Level transitions
    /// - Resetting positions
    ///
    /// For smooth physics-based movement, use velocities or forces instead.
    ///
    /// # Parameters
    /// * `wake_up` - If `true`, prevents the body from immediately going back to sleep
    #[inline]
    pub fn set_translation(&mut self, translation: Vector, wake_up: bool) {
        if self.pos.position.translation != translation
            || self.pos.next_position.translation != translation
        {
            self.changes.insert(RigidBodyChanges::POSITION);
            self.pos.position.translation = translation;
            self.pos.next_position.translation = translation;

            // Update the world mass-properties so torque application remains valid.
            self.update_world_mass_properties();

            // TODO: Do we really need to check that the body isn't dynamic?
            if wake_up && self.is_dynamic_or_kinematic() {
                self.wake_up(true)
            }
        }
    }

    /// The current rotation/orientation of this rigid body.
    #[inline]
    pub fn rotation(&self) -> &Rotation {
        &self.pos.position.rotation
    }

    /// Instantly rotates this rigid body to a new orientation.
    ///
    /// ⚠️ **Warning**: This teleports the rotation, ignoring physics! See [`set_translation()`](Self::set_translation) for details.
    #[inline]
    pub fn set_rotation(&mut self, rotation: Rotation, wake_up: bool) {
        if self.pos.position.rotation != rotation || self.pos.next_position.rotation != rotation {
            self.changes.insert(RigidBodyChanges::POSITION);
            self.pos.position.rotation = rotation;
            self.pos.next_position.rotation = rotation;

            // Update the world mass-properties so torque application remains valid.
            self.update_world_mass_properties();

            // TODO: Do we really need to check that the body isn't dynamic?
            if wake_up && self.is_dynamic_or_kinematic() {
                self.wake_up(true)
            }
        }
    }

    /// Teleports this body to a new position and rotation (ignoring physics).
    ///
    /// ⚠️ **Warning**: Instantly moves the body without checking for collisions!
    /// For position-based kinematic bodies, this also resets their interpolated velocity to zero.
    ///
    /// Use for respawning, level transitions, or resetting positions.
    pub fn set_position(&mut self, pos: Pose, wake_up: bool) {
        if self.pos.position != pos || self.pos.next_position != pos {
            self.changes.insert(RigidBodyChanges::POSITION);
            self.pos.position = pos;
            self.pos.next_position = pos;

            // Update the world mass-properties so torque application remains valid.
            self.update_world_mass_properties();

            // TODO: Do we really need to check that the body isn't dynamic?
            if wake_up && self.is_dynamic_or_kinematic() {
                self.wake_up(true)
            }
        }
    }

    /// For position-based kinematic bodies: sets where the body should rotate to by next frame.
    ///
    /// Only works for `KinematicPositionBased` bodies. Rapier computes the angular velocity
    /// needed to reach this rotation smoothly.
    pub fn set_next_kinematic_rotation(&mut self, rotation: Rotation) {
        if self.is_kinematic() {
            self.pos.next_position.rotation = rotation;

            if self.pos.position.rotation != rotation {
                self.wake_up(true);
            }
        }
    }

    /// For position-based kinematic bodies: sets where the body should move to by next frame.
    ///
    /// Only works for `KinematicPositionBased` bodies. Rapier computes the velocity
    /// needed to reach this position smoothly.
    pub fn set_next_kinematic_translation(&mut self, translation: Vector) {
        if self.is_kinematic() {
            self.pos.next_position.translation = translation;

            if self.pos.position.translation != translation {
                self.wake_up(true);
            }
        }
    }

    /// For position-based kinematic bodies: sets the target pose (position + rotation) for next frame.
    ///
    /// Only works for `KinematicPositionBased` bodies. Combines translation and rotation control.
    pub fn set_next_kinematic_position(&mut self, pos: Pose) {
        if self.is_kinematic() {
            self.pos.next_position = pos;

            if self.pos.position != pos {
                self.wake_up(true);
            }
        }
    }

    /// Predicts the next position of this rigid-body, by integrating its velocity and forces
    /// by a time of `dt`.
    pub(crate) fn predict_position_using_velocity_and_forces_with_max_dist(
        &self,
        dt: Real,
        max_dist: Real,
    ) -> Pose {
        let new_vels = self.forces.integrate(dt, &self.vels, &self.mprops);
        // Compute the clamped dt such that the body doesn't travel more than `max_dist`.
        let linvel_norm = new_vels.linvel.length();
        let clamped_linvel = linvel_norm.min(max_dist * crate::utils::inv(dt));
        let clamped_dt = dt * clamped_linvel * crate::utils::inv(linvel_norm);
        new_vels.integrate(
            clamped_dt,
            &self.pos.position,
            &self.mprops.local_mprops.local_com,
        )
    }

    /// Calculates where this body will be after `dt` seconds, considering current velocity AND forces.
    ///
    /// Useful for predicting future positions or implementing custom integration.
    /// Accounts for gravity and applied forces.
    pub fn predict_position_using_velocity_and_forces(&self, dt: Real) -> Pose {
        self.pos
            .integrate_forces_and_velocities(dt, &self.forces, &self.vels, &self.mprops)
    }

    /// Calculates where this body will be after `dt` seconds, considering only current velocity (not forces).
    ///
    /// Like `predict_position_using_velocity_and_forces()` but ignores applied forces.
    /// Useful when you only care about inertial motion without acceleration.
    pub fn predict_position_using_velocity(&self, dt: Real) -> Pose {
        self.vels
            .integrate(dt, &self.pos.position, &self.mprops.local_mprops.local_com)
    }

    pub(crate) fn update_world_mass_properties(&mut self) {
        self.mprops
            .update_world_mass_properties(self.body_type, &self.pos.position);
    }
}

/// ## Applying forces and torques
impl RigidBody {
    /// Clears all forces that were added with `add_force()`.
    ///
    /// Forces are automatically cleared each physics step, so you rarely need this.
    /// Useful if you want to cancel forces mid-frame.
    pub fn reset_forces(&mut self, wake_up: bool) {
        if self.forces.user_force != Vector::ZERO {
            self.forces.user_force = Vector::ZERO;

            if wake_up {
                self.wake_up(true);
            }
        }
    }

    /// Clears all torques that were added with `add_torque()`.
    ///
    /// Torques are automatically cleared each physics step. Rarely needed.
    #[cfg(feature = "dim2")]
    pub fn reset_torques(&mut self, wake_up: bool) {
        if self.forces.user_torque != 0.0 {
            self.forces.user_torque = 0.0;

            if wake_up {
                self.wake_up(true);
            }
        }
    }

    /// Clears all torques that were added with `add_torque()`.
    ///
    /// Torques are automatically cleared each physics step. Rarely needed.
    #[cfg(feature = "dim3")]
    pub fn reset_torques(&mut self, wake_up: bool) {
        if self.forces.user_torque != AngVector::ZERO {
            self.forces.user_torque = AngVector::ZERO;

            if wake_up {
                self.wake_up(true);
            }
        }
    }

    /// Applies a continuous force to this body (like thrust, wind, or magnets).
    ///
    /// Unlike [`apply_impulse()`](Self::apply_impulse) which is instant, forces are applied
    /// continuously over time and accumulate until the next physics step. Use for:
    /// - Rocket/jet thrust
    /// - Wind or water currents
    /// - Magnetic/gravity fields
    /// - Continuous pushing/pulling
    ///
    /// Forces are automatically cleared after each physics step, so you typically call this
    /// every frame if you want continuous force application.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let body = bodies.insert(RigidBodyBuilder::dynamic());
    /// // Apply thrust every frame
    /// bodies[body].add_force(Vector::new(0.0, 100.0, 0.0), true);
    /// ```
    ///
    /// Only affects dynamic bodies (does nothing for kinematic/fixed bodies).
    pub fn add_force(&mut self, force: Vector, wake_up: bool) {
        if force != Vector::ZERO && self.body_type == RigidBodyType::Dynamic {
            self.forces.user_force += force;

            if wake_up {
                self.wake_up(true);
            }
        }
    }

    /// Applies a continuous rotational force (torque) to spin this body.
    ///
    /// Like `add_force()` but for rotation. Accumulates until next physics step.
    /// In 2D: positive = counter-clockwise, negative = clockwise.
    ///
    /// Only affects dynamic bodies.
    #[cfg(feature = "dim2")]
    pub fn add_torque(&mut self, torque: Real, wake_up: bool) {
        if !torque.is_zero() && self.body_type == RigidBodyType::Dynamic {
            self.forces.user_torque += torque;

            if wake_up {
                self.wake_up(true);
            }
        }
    }

    /// Applies a continuous rotational force (torque) to spin this body.
    ///
    /// Like `add_force()` but for rotation. In 3D, the torque vector direction
    /// determines the rotation axis (right-hand rule).
    ///
    /// Only affects dynamic bodies.
    #[cfg(feature = "dim3")]
    pub fn add_torque(&mut self, torque: Vector, wake_up: bool) {
        if torque != Vector::ZERO && self.body_type == RigidBodyType::Dynamic {
            self.forces.user_torque += torque;

            if wake_up {
                self.wake_up(true);
            }
        }
    }

    /// Applies force at a specific point on the body (creates both force and torque).
    ///
    /// When you push an object off-center, it both moves AND spins. This method handles both effects.
    /// The force creates linear acceleration, and the offset from center-of-mass creates torque.
    ///
    /// Use for: Forces applied at contact points, explosions at specific locations, pushing objects.
    ///
    /// # Parameters
    /// * `force` - The force vector to apply
    /// * `point` - Where to apply the force (world coordinates)
    ///
    /// Only affects dynamic bodies.
    pub fn add_force_at_point(&mut self, force: Vector, point: Vector, wake_up: bool) {
        if force != Vector::ZERO && self.body_type == RigidBodyType::Dynamic {
            self.forces.user_force += force;
            self.forces.user_torque += (point - self.mprops.world_com).gcross(force);

            if wake_up {
                self.wake_up(true);
            }
        }
    }
}

/// ## Applying impulses and angular impulses
impl RigidBody {
    /// Instantly changes the velocity by applying an impulse (like a kick or explosion).
    ///
    /// An impulse is an instant change in momentum. Think of it as a "one-time push" that
    /// immediately affects velocity. Use for:
    /// - Jumping (apply upward impulse)
    /// - Explosions pushing objects away
    /// - Getting hit by something
    /// - Launching projectiles
    ///
    /// The effect depends on the body's mass - heavier objects will be affected less by the same impulse.
    ///
    /// **For continuous forces** (like rocket thrust or wind), use [`add_force()`](Self::add_force) instead.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let body = bodies.insert(RigidBodyBuilder::dynamic());
    /// // Make a character jump
    /// bodies[body].apply_impulse(Vector::new(0.0, 300.0, 0.0), true);
    /// ```
    ///
    /// Only affects dynamic bodies (does nothing for kinematic/fixed bodies).
    pub fn apply_impulse(&mut self, impulse: Vector, wake_up: bool) {
        if impulse != Vector::ZERO && self.body_type == RigidBodyType::Dynamic {
            self.vels.linvel += impulse * self.mprops.effective_inv_mass;

            if wake_up {
                self.wake_up(true);
            }
        }
    }

    /// Applies an angular impulse at the center-of-mass of this rigid-body.
    /// The impulse is applied right away, changing the angular velocity.
    /// This does nothing on non-dynamic bodies.
    #[cfg(feature = "dim2")]
    pub fn apply_torque_impulse(&mut self, torque_impulse: Real, wake_up: bool) {
        if !torque_impulse.is_zero() && self.body_type == RigidBodyType::Dynamic {
            self.vels.angvel += self.mprops.effective_world_inv_inertia * torque_impulse;

            if wake_up {
                self.wake_up(true);
            }
        }
    }

    /// Instantly changes rotation speed by applying angular impulse (like a sudden spin).
    ///
    /// In 3D, the impulse vector direction determines the spin axis (right-hand rule).
    /// Like `apply_impulse()` but for rotation. Only affects dynamic bodies.
    #[cfg(feature = "dim3")]
    pub fn apply_torque_impulse(&mut self, torque_impulse: Vector, wake_up: bool) {
        if torque_impulse != Vector::ZERO && self.body_type == RigidBodyType::Dynamic {
            self.vels.angvel += self.mprops.effective_world_inv_inertia * torque_impulse;

            if wake_up {
                self.wake_up(true);
            }
        }
    }

    /// Applies impulse at a specific point on the body (creates both linear and angular effects).
    ///
    /// Like `add_force_at_point()` but instant instead of continuous. When you hit an object
    /// off-center, it both flies away AND spins - this method handles both.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let body = bodies.insert(RigidBodyBuilder::dynamic());
    /// // Hit the top-left corner of a box
    /// bodies[body].apply_impulse_at_point(
    ///     Vector::new(100.0, 0.0, 0.0),
    ///     Vector::new(-0.5, 0.5, 0.0),  // Top-left of a 1x1 box
    ///     true
    /// );
    /// // Box will move right AND spin
    /// ```
    ///
    /// Only affects dynamic bodies.
    pub fn apply_impulse_at_point(&mut self, impulse: Vector, point: Vector, wake_up: bool) {
        let torque_impulse = (point - self.mprops.world_com).gcross(impulse);
        self.apply_impulse(impulse, wake_up);
        self.apply_torque_impulse(torque_impulse, wake_up);
    }

    /// Returns the total force currently queued to be applied this frame.
    ///
    /// This is the sum of all `add_force()` calls since the last physics step.
    /// Returns zero for non-dynamic bodies.
    pub fn user_force(&self) -> Vector {
        if self.body_type == RigidBodyType::Dynamic {
            self.forces.user_force
        } else {
            Vector::ZERO
        }
    }

    /// Returns the total torque currently queued to be applied this frame.
    ///
    /// This is the sum of all `add_torque()` calls since the last physics step.
    /// Returns zero for non-dynamic bodies.
    pub fn user_torque(&self) -> AngVector {
        if self.body_type == RigidBodyType::Dynamic {
            self.forces.user_torque
        } else {
            #[cfg(feature = "dim2")]
            {
                0.0
            }
            #[cfg(feature = "dim3")]
            {
                AngVector::ZERO
            }
        }
    }

    /// Checks if gyroscopic forces are enabled (3D only).
    ///
    /// Gyroscopic forces cause spinning objects to resist changes in rotation axis
    /// (like how spinning tops stay upright). Adds slight CPU cost.
    #[cfg(feature = "dim3")]
    pub fn gyroscopic_forces_enabled(&self) -> bool {
        self.forces.gyroscopic_forces_enabled
    }

    /// Enables/disables gyroscopic forces for more realistic spinning behavior.
    ///
    /// When enabled, rapidly spinning objects resist rotation axis changes (like gyroscopes).
    /// Examples: spinning tops, flywheels, rotating spacecraft.
    ///
    /// **Default**: Disabled (costs performance, rarely needed in games).
    #[cfg(feature = "dim3")]
    pub fn enable_gyroscopic_forces(&mut self, enabled: bool) {
        self.forces.gyroscopic_forces_enabled = enabled;
    }
}

impl RigidBody {
    /// Calculates the velocity at a specific point on this body.
    ///
    /// Due to rotation, different points on a rigid body move at different speeds.
    /// This computes the linear velocity at any world-space point.
    ///
    /// Useful for: impact calculations, particle effects, sound volume based on impact speed.
    pub fn velocity_at_point(&self, point: Vector) -> Vector {
        self.vels.velocity_at_point(point, self.mprops.world_com)
    }

    /// Calculates the kinetic energy of this body (energy from motion).
    ///
    /// Returns `0.5 * mass * velocity² + 0.5 * inertia * angular_velocity²`
    /// Useful for physics-based gameplay (energy tracking, damage based on impact energy).
    pub fn kinetic_energy(&self) -> Real {
        self.vels.kinetic_energy(&self.mprops)
    }

    /// Calculates the gravitational potential energy of this body.
    ///
    /// Returns `mass * gravity * height`. Useful for energy conservation checks.
    pub fn gravitational_potential_energy(&self, dt: Real, gravity: Vector) -> Real {
        let world_com = self.mprops.local_mprops.world_com(&self.pos.position);

        // Project position back along velocity vector one half-step (leap-frog)
        // to sync up the potential energy with the kinetic energy:
        let world_com = world_com - self.vels.linvel * (dt / 2.0);

        -self.mass() * self.forces.gravity_scale * gravity.dot(world_com)
    }

    /// Computes the angular velocity of this rigid-body after application of gyroscopic forces.
    #[cfg(feature = "dim3")]
    pub fn angvel_with_gyroscopic_forces(&self, dt: Real) -> AngVector {
        // NOTE: integrating the gyroscopic forces implicitly are both slower and
        //       very dissipative. Instead, we only keep the explicit term and
        //       ensure angular momentum is preserved (similar to Jolt).
        let w = self.pos.position.rotation.inverse() * self.angvel();
        let i = self.mprops.local_mprops.principal_inertia();
        let ii = self.mprops.local_mprops.inv_principal_inertia;
        let curr_momentum = i * w;
        let explicit_gyro_momentum = -w.cross(curr_momentum) * dt;
        let total_momentum = curr_momentum + explicit_gyro_momentum;
        let total_momentum_sqnorm = total_momentum.length_squared();

        if total_momentum_sqnorm != 0.0 {
            let capped_momentum =
                total_momentum * (curr_momentum.length_squared() / total_momentum_sqnorm).sqrt();
            self.pos.position.rotation * (ii * capped_momentum)
        } else {
            self.angvel()
        }
    }
}

/// A builder for creating rigid bodies with custom properties.
///
/// This builder lets you configure all properties of a rigid body before adding it to your world.
/// Start with one of the type constructors ([`dynamic()`](Self::dynamic), [`fixed()`](Self::fixed),
///  [`kinematic_position_based()`](Self::kinematic_position_based), or
/// [`kinematic_velocity_based()`](Self::kinematic_velocity_based)), then chain property setters,
/// and finally call [`build()`](Self::build).
///
/// # Example
///
/// ```
/// # use rapier3d::prelude::*;
/// let body = RigidBodyBuilder::dynamic()
///     .translation(Vector::new(0.0, 5.0, 0.0))  // Start 5 units above ground
///     .linvel(Vector::new(1.0, 0.0, 0.0))       // Initial velocity to the right
///     .can_sleep(false)                          // Keep always active
///     .build();
/// ```
#[derive(Clone, Debug, PartialEq)]
#[must_use = "Builder functions return the updated builder"]
pub struct RigidBodyBuilder {
    /// The initial position of the rigid-body to be built.
    pub position: Pose,
    /// The linear velocity of the rigid-body to be built.
    pub linvel: Vector,
    /// The angular velocity of the rigid-body to be built.
    pub angvel: AngVector,
    /// The scale factor applied to the gravity affecting the rigid-body to be built, `1.0` by default.
    pub gravity_scale: Real,
    /// Damping factor for gradually slowing down the translational motion of the rigid-body, `0.0` by default.
    pub linear_damping: Real,
    /// Damping factor for gradually slowing down the angular motion of the rigid-body, `0.0` by default.
    pub angular_damping: Real,
    /// The type of rigid-body being constructed.
    pub body_type: RigidBodyType,
    mprops_flags: LockedAxes,
    /// The additional mass-properties of the rigid-body being built. See [`RigidBodyBuilder::additional_mass_properties`] for more information.
    additional_mass_properties: RigidBodyAdditionalMassProps,
    /// Whether the rigid-body to be created can sleep if it reaches a dynamic equilibrium.
    pub can_sleep: bool,
    /// Whether the rigid-body is to be created asleep.
    pub sleeping: bool,
    /// Whether Continuous Collision-Detection is enabled for the rigid-body to be built.
    ///
    /// CCD prevents tunneling, but may still allow limited interpenetration of colliders.
    pub ccd_enabled: bool,
    /// The maximum prediction distance Soft Continuous Collision-Detection.
    ///
    /// When set to 0, soft CCD is disabled. Soft-CCD helps prevent tunneling especially of
    /// slow-but-thin to moderately fast objects. The soft CCD prediction distance indicates how
    /// far in the object’s path the CCD algorithm is allowed to inspect. Large values can impact
    /// performance badly by increasing the work needed from the broad-phase.
    ///
    /// It is a generally cheaper variant of regular CCD (that can be enabled with
    /// [`RigidBodyBuilder::ccd_enabled`] since it relies on predictive constraints instead of
    /// shape-cast and substeps.
    pub soft_ccd_prediction: Real,
    /// The dominance group of the rigid-body to be built.
    pub dominance_group: i8,
    /// Will the rigid-body being built be enabled?
    pub enabled: bool,
    /// An arbitrary user-defined 128-bit integer associated to the rigid-bodies built by this builder.
    pub user_data: u128,
    /// The additional number of solver iterations run for this rigid-body and
    /// everything interacting with it.
    ///
    /// See [`RigidBody::set_additional_solver_iterations`] for additional information.
    pub additional_solver_iterations: usize,
    /// Are gyroscopic forces enabled for this rigid-body?
    pub gyroscopic_forces_enabled: bool,
}

impl Default for RigidBodyBuilder {
    fn default() -> Self {
        Self::dynamic()
    }
}

impl RigidBodyBuilder {
    /// Initialize a new builder for a rigid body which is either fixed, dynamic, or kinematic.
    pub fn new(body_type: RigidBodyType) -> Self {
        #[cfg(feature = "dim2")]
        let angvel = 0.0;
        #[cfg(feature = "dim3")]
        let angvel = AngVector::ZERO;

        Self {
            position: Pose::IDENTITY,
            linvel: Vector::ZERO,
            angvel,
            gravity_scale: 1.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            body_type,
            mprops_flags: LockedAxes::empty(),
            additional_mass_properties: RigidBodyAdditionalMassProps::default(),
            can_sleep: true,
            sleeping: false,
            ccd_enabled: false,
            soft_ccd_prediction: 0.0,
            dominance_group: 0,
            enabled: true,
            user_data: 0,
            additional_solver_iterations: 0,
            gyroscopic_forces_enabled: false,
        }
    }

    /// Initializes the builder of a new fixed rigid body.
    #[deprecated(note = "use `RigidBodyBuilder::fixed()` instead")]
    pub fn new_static() -> Self {
        Self::fixed()
    }
    /// Initializes the builder of a new velocity-based kinematic rigid body.
    #[deprecated(note = "use `RigidBodyBuilder::kinematic_velocity_based()` instead")]
    pub fn new_kinematic_velocity_based() -> Self {
        Self::kinematic_velocity_based()
    }
    /// Initializes the builder of a new position-based kinematic rigid body.
    #[deprecated(note = "use `RigidBodyBuilder::kinematic_position_based()` instead")]
    pub fn new_kinematic_position_based() -> Self {
        Self::kinematic_position_based()
    }

    /// Creates a builder for a **fixed** (static) rigid body.
    ///
    /// Fixed bodies never move and are not affected by any forces. Use them for:
    /// - Walls, floors, and ceilings
    /// - Static terrain and level geometry
    /// - Any object that should never move in your simulation
    ///
    /// Fixed bodies have infinite mass and never sleep.
    pub fn fixed() -> Self {
        Self::new(RigidBodyType::Fixed)
    }

    /// Creates a builder for a **velocity-based kinematic** rigid body.
    ///
    /// Kinematic bodies are moved by directly setting their velocity (not by applying forces).
    /// They can push dynamic bodies but are not affected by them. Use for:
    /// - Moving platforms and elevators
    /// - Doors and sliding panels
    /// - Any object you want to control directly while still affecting other physics objects
    ///
    /// Set velocity with [`RigidBody::set_linvel`] and [`RigidBody::set_angvel`].
    pub fn kinematic_velocity_based() -> Self {
        Self::new(RigidBodyType::KinematicVelocityBased)
    }

    /// Creates a builder for a **position-based kinematic** rigid body.
    ///
    /// Similar to velocity-based kinematic, but you control it by setting its next position
    /// directly rather than setting velocity. Rapier will automatically compute the velocity
    /// needed to reach that position. Use for objects animated by external systems.
    pub fn kinematic_position_based() -> Self {
        Self::new(RigidBodyType::KinematicPositionBased)
    }

    /// Creates a builder for a **dynamic** rigid body.
    ///
    /// Dynamic bodies are fully simulated - they respond to gravity, forces, collisions, and
    /// constraints. This is the most common type for interactive objects. Use for:
    /// - Physics objects that should fall and bounce (boxes, spheres, ragdolls)
    /// - Projectiles and debris
    /// - Vehicles and moving characters (when not using kinematic control)
    /// - Any object that should behave realistically under physics
    ///
    /// Dynamic bodies can sleep (become inactive) when at rest to save performance.
    pub fn dynamic() -> Self {
        Self::new(RigidBodyType::Dynamic)
    }

    /// Sets the additional number of solver iterations run for this rigid-body and
    /// everything interacting with it.
    ///
    /// See [`RigidBody::set_additional_solver_iterations`] for additional information.
    pub fn additional_solver_iterations(mut self, additional_iterations: usize) -> Self {
        self.additional_solver_iterations = additional_iterations;
        self
    }

    /// Sets the scale applied to the gravity force affecting the rigid-body to be created.
    pub fn gravity_scale(mut self, scale_factor: Real) -> Self {
        self.gravity_scale = scale_factor;
        self
    }

    /// Sets the dominance group (advanced collision priority system).
    ///
    /// Higher dominance groups can push lower ones but not vice versa.
    /// Rarely needed - most games don't use this. Default is 0 (all equal priority).
    ///
    /// Use case: Heavy objects that should always push lighter ones in contacts.
    pub fn dominance_group(mut self, group: i8) -> Self {
        self.dominance_group = group;
        self
    }

    /// Sets the initial position (XYZ coordinates) where this body will be created.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// let body = RigidBodyBuilder::dynamic()
    ///     .translation(Vector::new(10.0, 5.0, -3.0))
    ///     .build();
    /// ```
    pub fn translation(mut self, translation: Vector) -> Self {
        self.position.translation = translation;
        self
    }

    /// Sets the initial rotation/orientation of the body to be created.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// // Rotate 45 degrees around Y axis (in 3D)
    /// let body = RigidBodyBuilder::dynamic()
    ///     .rotation(Vector::new(0.0, std::f32::consts::PI / 4.0, 0.0))
    ///     .build();
    /// ```
    pub fn rotation(mut self, angle: AngVector) -> Self {
        self.position.rotation = rotation_from_angle(angle);
        self
    }

    /// Sets the initial position (translation and orientation) of the rigid-body to be created.
    #[deprecated = "renamed to `RigidBodyBuilder::pose`"]
    pub fn position(mut self, pos: Pose) -> Self {
        self.position = pos;
        self
    }

    /// Sets the initial pose (translation and orientation) of the rigid-body to be created.
    pub fn pose(mut self, pos: Pose) -> Self {
        self.position = pos;
        self
    }

    /// An arbitrary user-defined 128-bit integer associated to the rigid-bodies built by this builder.
    pub fn user_data(mut self, data: u128) -> Self {
        self.user_data = data;
        self
    }

    /// Sets the additional mass-properties of the rigid-body being built.
    ///
    /// This will be overridden by a call to [`Self::additional_mass`] so it only makes sense to call
    /// either [`Self::additional_mass`] or [`Self::additional_mass_properties`].    
    ///
    /// Note that "additional" means that the final mass-properties of the rigid-bodies depends
    /// on the initial mass-properties of the rigid-body (set by this method)
    /// to which is added the contributions of all the colliders with non-zero density
    /// attached to this rigid-body.
    ///
    /// Therefore, if you want your provided mass-properties to be the final
    /// mass-properties of your rigid-body, don't attach colliders to it, or
    /// only attach colliders with densities equal to zero.
    pub fn additional_mass_properties(mut self, mprops: MassProperties) -> Self {
        self.additional_mass_properties = RigidBodyAdditionalMassProps::MassProps(mprops);
        self
    }

    /// Sets the additional mass of the rigid-body being built.
    ///
    /// This will be overridden by a call to [`Self::additional_mass_properties`] so it only makes
    /// sense to call either [`Self::additional_mass`] or [`Self::additional_mass_properties`].    
    ///
    /// This is only the "additional" mass because the total mass of the  rigid-body is
    /// equal to the sum of this additional mass and the mass computed from the colliders
    /// (with non-zero densities) attached to this rigid-body.
    ///
    /// The total angular inertia of the rigid-body will be scaled automatically based on this
    /// additional mass. If this scaling effect isn’t desired, use [`Self::additional_mass_properties`]
    /// instead of this method.
    ///
    /// # Parameters
    /// * `mass`- The mass that will be added to the created rigid-body.
    pub fn additional_mass(mut self, mass: Real) -> Self {
        self.additional_mass_properties = RigidBodyAdditionalMassProps::Mass(mass);
        self
    }

    /// Sets which movement axes are locked (cannot move/rotate).
    ///
    /// See [`LockedAxes`] for examples of constraining movement to specific directions.
    pub fn locked_axes(mut self, locked_axes: LockedAxes) -> Self {
        self.mprops_flags = locked_axes;
        self
    }

    /// Prevents all translational movement (body can still rotate).
    ///
    /// Use for turrets, spinning objects fixed in place, etc.
    pub fn lock_translations(mut self) -> Self {
        self.mprops_flags.set(LockedAxes::TRANSLATION_LOCKED, true);
        self
    }

    /// Locks translation along specific axes.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// // 2D game in 3D: lock Z movement
    /// let body = RigidBodyBuilder::dynamic()
    ///     .enabled_translations(true, true, false)  // X, Y free; Z locked
    ///     .build();
    /// ```
    pub fn enabled_translations(
        mut self,
        allow_translations_x: bool,
        allow_translations_y: bool,
        #[cfg(feature = "dim3")] allow_translations_z: bool,
    ) -> Self {
        self.mprops_flags
            .set(LockedAxes::TRANSLATION_LOCKED_X, !allow_translations_x);
        self.mprops_flags
            .set(LockedAxes::TRANSLATION_LOCKED_Y, !allow_translations_y);
        #[cfg(feature = "dim3")]
        self.mprops_flags
            .set(LockedAxes::TRANSLATION_LOCKED_Z, !allow_translations_z);
        self
    }

    #[deprecated(note = "Use `enabled_translations` instead")]
    /// Only allow translations of this rigid-body around specific coordinate axes.
    pub fn restrict_translations(
        self,
        allow_translations_x: bool,
        allow_translations_y: bool,
        #[cfg(feature = "dim3")] allow_translations_z: bool,
    ) -> Self {
        self.enabled_translations(
            allow_translations_x,
            allow_translations_y,
            #[cfg(feature = "dim3")]
            allow_translations_z,
        )
    }

    /// Prevents all rotational movement (body can still translate).
    ///
    /// Use for characters that shouldn't tip over, objects that should only slide, etc.
    pub fn lock_rotations(mut self) -> Self {
        self.mprops_flags.set(LockedAxes::ROTATION_LOCKED_X, true);
        self.mprops_flags.set(LockedAxes::ROTATION_LOCKED_Y, true);
        self.mprops_flags.set(LockedAxes::ROTATION_LOCKED_Z, true);
        self
    }

    /// Only allow rotations of this rigid-body around specific coordinate axes.
    #[cfg(feature = "dim3")]
    pub fn enabled_rotations(
        mut self,
        allow_rotations_x: bool,
        allow_rotations_y: bool,
        allow_rotations_z: bool,
    ) -> Self {
        self.mprops_flags
            .set(LockedAxes::ROTATION_LOCKED_X, !allow_rotations_x);
        self.mprops_flags
            .set(LockedAxes::ROTATION_LOCKED_Y, !allow_rotations_y);
        self.mprops_flags
            .set(LockedAxes::ROTATION_LOCKED_Z, !allow_rotations_z);
        self
    }

    /// Locks or unlocks rotations of this rigid-body along each cartesian axes.
    #[deprecated(note = "Use `enabled_rotations` instead")]
    #[cfg(feature = "dim3")]
    pub fn restrict_rotations(
        self,
        allow_rotations_x: bool,
        allow_rotations_y: bool,
        allow_rotations_z: bool,
    ) -> Self {
        self.enabled_rotations(allow_rotations_x, allow_rotations_y, allow_rotations_z)
    }

    /// Sets linear damping (how quickly linear velocity decreases over time).
    ///
    /// Models air resistance, drag, etc. Higher values = faster slowdown.
    /// - `0.0` = no drag (space)
    /// - `0.1` = light drag (air)
    /// - `1.0+` = heavy drag (underwater)
    pub fn linear_damping(mut self, factor: Real) -> Self {
        self.linear_damping = factor;
        self
    }

    /// Sets angular damping (how quickly rotation speed decreases over time).
    ///
    /// Models rotational drag. Higher values = spinning stops faster.
    pub fn angular_damping(mut self, factor: Real) -> Self {
        self.angular_damping = factor;
        self
    }

    /// Sets the initial linear velocity (movement speed and direction).
    ///
    /// The body will start moving at this velocity when created.
    pub fn linvel(mut self, linvel: Vector) -> Self {
        self.linvel = linvel;
        self
    }

    /// Sets the initial angular velocity (rotation speed).
    ///
    /// The body will start rotating at this speed when created.
    pub fn angvel(mut self, angvel: AngVector) -> Self {
        self.angvel = angvel;
        self
    }

    /// Sets whether this body can go to sleep when at rest (default: `true`).
    ///
    /// Sleeping bodies are excluded from simulation until disturbed, saving CPU.
    /// Set to `false` if you need the body always active (e.g., for continuous queries).
    pub fn can_sleep(mut self, can_sleep: bool) -> Self {
        self.can_sleep = can_sleep;
        self
    }

    /// Enables Continuous Collision Detection to prevent fast objects from tunneling.
    ///
    /// CCD prevents "tunneling" where fast-moving objects pass through thin walls.
    /// Enable this for:
    /// - Bullets and fast projectiles
    /// - Small objects moving at high speed
    /// - Objects that must never pass through walls
    ///
    /// **Trade-off**: More accurate but more expensive. Most objects don't need CCD.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// // Bullet that should never tunnel through walls
    /// let bullet = RigidBodyBuilder::dynamic()
    ///     .ccd_enabled(true)
    ///     .build();
    /// ```
    pub fn ccd_enabled(mut self, enabled: bool) -> Self {
        self.ccd_enabled = enabled;
        self
    }

    /// Sets the maximum prediction distance Soft Continuous Collision-Detection.
    ///
    /// When set to 0, soft-CCD is disabled. Soft-CCD helps prevent tunneling especially of
    /// slow-but-thin to moderately fast objects. The soft CCD prediction distance indicates how
    /// far in the object’s path the CCD algorithm is allowed to inspect. Large values can impact
    /// performance badly by increasing the work needed from the broad-phase.
    ///
    /// It is a generally cheaper variant of regular CCD (that can be enabled with
    /// [`RigidBodyBuilder::ccd_enabled`] since it relies on predictive constraints instead of
    /// shape-cast and substeps.
    pub fn soft_ccd_prediction(mut self, prediction_distance: Real) -> Self {
        self.soft_ccd_prediction = prediction_distance;
        self
    }

    /// Sets whether the rigid-body is to be created asleep.
    pub fn sleeping(mut self, sleeping: bool) -> Self {
        self.sleeping = sleeping;
        self
    }

    /// Are gyroscopic forces enabled for this rigid-body?
    ///
    /// Enabling gyroscopic forces allows more realistic behaviors like gyroscopic precession,
    /// but result in a slight performance overhead.
    ///
    /// Disabled by default.
    #[cfg(feature = "dim3")]
    pub fn gyroscopic_forces_enabled(mut self, enabled: bool) -> Self {
        self.gyroscopic_forces_enabled = enabled;
        self
    }

    /// Enable or disable the rigid-body after its creation.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Build a new rigid-body with the parameters configured with this builder.
    pub fn build(&self) -> RigidBody {
        let mut rb = RigidBody::new();
        rb.pos.next_position = self.position;
        rb.pos.position = self.position;
        rb.vels.linvel = self.linvel;
        rb.vels.angvel = self.angvel;
        rb.body_type = self.body_type;
        rb.user_data = self.user_data;
        rb.additional_solver_iterations = self.additional_solver_iterations;

        if self.additional_mass_properties
            != RigidBodyAdditionalMassProps::MassProps(MassProperties::default())
            && self.additional_mass_properties != RigidBodyAdditionalMassProps::Mass(0.0)
        {
            rb.mprops.additional_local_mprops = Some(Box::new(self.additional_mass_properties));
        }

        rb.mprops.flags = self.mprops_flags;
        rb.damping.linear_damping = self.linear_damping;
        rb.damping.angular_damping = self.angular_damping;
        rb.forces.gravity_scale = self.gravity_scale;
        #[cfg(feature = "dim3")]
        {
            rb.forces.gyroscopic_forces_enabled = self.gyroscopic_forces_enabled;
        }
        rb.dominance = RigidBodyDominance(self.dominance_group);
        rb.enabled = self.enabled;
        rb.enable_ccd(self.ccd_enabled);
        rb.set_soft_ccd_prediction(self.soft_ccd_prediction);

        if self.can_sleep && self.sleeping {
            rb.sleep();
        }

        if !self.can_sleep {
            rb.activation.normalized_linear_threshold = -1.0;
            rb.activation.angular_threshold = -1.0;
        }

        rb
    }
}

impl From<RigidBodyBuilder> for RigidBody {
    fn from(val: RigidBodyBuilder) -> RigidBody {
        val.build()
    }
}
