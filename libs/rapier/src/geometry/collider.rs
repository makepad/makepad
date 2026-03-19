use crate::dynamics::{CoefficientCombineRule, MassProperties, RigidBodyHandle, RigidBodySet};
#[cfg(feature = "dim3")]
use crate::geometry::HeightFieldFlags;
use crate::geometry::{
    ActiveCollisionTypes, ColliderChanges, ColliderFlags, ColliderMassProps, ColliderMaterial,
    ColliderParent, ColliderPosition, ColliderShape, ColliderType, InteractionGroups,
    MeshConverter, MeshConverterError, SharedShape,
};
use crate::math::{AngVector, DIM, IVector, Pose, Real, Rotation, Vector, rotation_from_angle};
use crate::parry::transformation::vhacd::VHACDParameters;
use crate::pipeline::{ActiveEvents, ActiveHooks};
use crate::prelude::{ColliderEnabled, IntegrationParameters};
use na::Unit;
use parry::bounding_volume::{Aabb, BoundingVolume};
use parry::shape::{Shape, TriMeshBuilderError, TriMeshFlags};
use parry::transformation::voxelization::FillMode;
#[cfg(feature = "dim3")]
use parry::utils::Array2;

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
/// The collision shape attached to a rigid body that defines what it can collide with.
///
/// Think of a collider as the "hitbox" or "collision shape" for your physics object. While a
/// [`RigidBody`](crate::dynamics::RigidBody) handles the physics (mass, velocity, forces),
/// the collider defines what shape the object has for collision detection.
///
/// ## Key concepts
///
/// - **Shape**: The geometric form (box, sphere, capsule, mesh, etc.)
/// - **Material**: Physical properties like friction (slipperiness) and restitution (bounciness)
/// - **Sensor vs. Solid**: Sensors detect overlaps but don't create physical collisions
/// - **Mass properties**: Automatically computed from the shape's volume and density
///
/// ## Creating colliders
///
/// Always use [`ColliderBuilder`] to create colliders:
///
/// ```ignore
/// let collider = ColliderBuilder::cuboid(1.0, 0.5, 1.0)  // 2x1x2 box
///     .friction(0.7)
///     .restitution(0.3);
/// colliders.insert_with_parent(collider, body_handle, &mut bodies);
/// ```
///
/// ## Attaching to bodies
///
/// Colliders are usually attached to rigid bodies. One body can have multiple colliders
/// to create compound shapes (like a character with separate colliders for head, torso, limbs).
pub struct Collider {
    pub(crate) coll_type: ColliderType,
    pub(crate) shape: ColliderShape,
    pub(crate) mprops: ColliderMassProps,
    pub(crate) changes: ColliderChanges,
    pub(crate) parent: Option<ColliderParent>,
    pub(crate) pos: ColliderPosition,
    pub(crate) material: ColliderMaterial,
    pub(crate) flags: ColliderFlags,
    contact_skin: Real,
    contact_force_event_threshold: Real,
    /// User-defined data associated to this collider.
    pub user_data: u128,
}

impl Collider {
    pub(crate) fn reset_internal_references(&mut self) {
        self.changes = ColliderChanges::all();
    }

    pub(crate) fn effective_contact_force_event_threshold(&self) -> Real {
        if self
            .flags
            .active_events
            .contains(ActiveEvents::CONTACT_FORCE_EVENTS)
        {
            self.contact_force_event_threshold
        } else {
            Real::MAX
        }
    }

    /// The rigid body this collider is attached to, if any.
    ///
    /// Returns `None` for standalone colliders (not attached to any body).
    pub fn parent(&self) -> Option<RigidBodyHandle> {
        self.parent.map(|parent| parent.handle)
    }

    /// Checks if this collider is a sensor (detects overlaps without physical collision).
    ///
    /// Sensors are like "trigger zones" - they detect when other colliders enter/exit them
    /// but don't create physical contact forces. Use for:
    /// - Trigger zones (checkpoint areas, damage regions)
    /// - Proximity detection
    /// - Collectible items
    /// - Area-of-effect detection
    pub fn is_sensor(&self) -> bool {
        self.coll_type.is_sensor()
    }

    /// Copy all the characteristics from `other` to `self`.
    ///
    /// If you have a mutable reference to a collider `collider: &mut Collider`, attempting to
    /// assign it a whole new collider instance, e.g., `*collider = ColliderBuilder::ball(0.5).build()`,
    /// will crash due to some internal indices being overwritten. Instead, use
    /// `collider.copy_from(&ColliderBuilder::ball(0.5).build())`.
    ///
    /// This method will allow you to set most characteristics of this collider from another
    /// collider instance without causing any breakage.
    ///
    /// This method **cannot** be used for reparenting a collider. Therefore, the parent of the
    /// `other` (if any), as well as its relative position to that parent will not be copied into
    /// `self`.
    ///
    /// The pose of `other` will only copied into `self` if `self` doesn’t have a parent (if it has
    /// a parent, its position is directly controlled by the parent rigid-body).
    pub fn copy_from(&mut self, other: &Collider) {
        // NOTE: we deconstruct the collider struct to be sure we don’t forget to
        //       add some copies here if we add more field to Collider in the future.
        let Collider {
            coll_type,
            shape,
            mprops,
            changes: _changes, // Will be set to ALL.
            parent: _parent,   // This function cannot be used to reparent the collider.
            pos,
            material,
            flags,
            contact_force_event_threshold,
            user_data,
            contact_skin,
        } = other;

        if self.parent.is_none() {
            self.pos = *pos;
        }

        self.coll_type = *coll_type;
        self.shape = shape.clone();
        self.mprops = mprops.clone();
        self.material = *material;
        self.contact_force_event_threshold = *contact_force_event_threshold;
        self.user_data = *user_data;
        self.flags = *flags;
        self.changes = ColliderChanges::all();
        self.contact_skin = *contact_skin;
    }

    /// Which physics hooks are enabled for this collider.
    ///
    /// Hooks allow custom filtering and modification of collisions. See [`PhysicsHooks`](crate::pipeline::PhysicsHooks).
    pub fn active_hooks(&self) -> ActiveHooks {
        self.flags.active_hooks
    }

    /// Enables/disables physics hooks for this collider.
    ///
    /// Use to opt colliders into custom collision filtering logic.
    pub fn set_active_hooks(&mut self, active_hooks: ActiveHooks) {
        self.flags.active_hooks = active_hooks;
    }

    /// Which events are enabled for this collider.
    ///
    /// Controls whether you receive collision/contact force events. See [`ActiveEvents`](crate::pipeline::ActiveEvents).
    pub fn active_events(&self) -> ActiveEvents {
        self.flags.active_events
    }

    /// Enables/disables event generation for this collider.
    ///
    /// Set to `ActiveEvents::COLLISION_EVENTS` to receive started/stopped collision notifications.
    /// Set to `ActiveEvents::CONTACT_FORCE_EVENTS` to receive force threshold events.
    pub fn set_active_events(&mut self, active_events: ActiveEvents) {
        self.flags.active_events = active_events;
    }

    /// The collision types enabled for this collider.
    pub fn active_collision_types(&self) -> ActiveCollisionTypes {
        self.flags.active_collision_types
    }

    /// Sets the collision types enabled for this collider.
    pub fn set_active_collision_types(&mut self, active_collision_types: ActiveCollisionTypes) {
        self.flags.active_collision_types = active_collision_types;
    }

    /// The contact skin of this collider.
    ///
    /// See the documentation of [`ColliderBuilder::contact_skin`] for details.
    pub fn contact_skin(&self) -> Real {
        self.contact_skin
    }

    /// Sets the contact skin of this collider.
    ///
    /// See the documentation of [`ColliderBuilder::contact_skin`] for details.
    pub fn set_contact_skin(&mut self, skin_thickness: Real) {
        self.contact_skin = skin_thickness;
    }

    /// The friction coefficient of this collider (how "slippery" it is).
    ///
    /// - `0.0` = perfectly slippery (ice)
    /// - `1.0` = high friction (rubber on concrete)
    /// - Typical values: 0.3-0.8
    pub fn friction(&self) -> Real {
        self.material.friction
    }

    /// Sets the friction coefficient (slipperiness).
    ///
    /// Controls how much this surface resists sliding. Higher values = more grip.
    /// Works with other collider's friction via the combine rule.
    pub fn set_friction(&mut self, coefficient: Real) {
        self.material.friction = coefficient
    }

    /// The combine rule used by this collider to combine its friction
    /// coefficient with the friction coefficient of the other collider it
    /// is in contact with.
    pub fn friction_combine_rule(&self) -> CoefficientCombineRule {
        self.material.friction_combine_rule
    }

    /// Sets the combine rule used by this collider to combine its friction
    /// coefficient with the friction coefficient of the other collider it
    /// is in contact with.
    pub fn set_friction_combine_rule(&mut self, rule: CoefficientCombineRule) {
        self.material.friction_combine_rule = rule;
    }

    /// The restitution coefficient of this collider (how "bouncy" it is).
    ///
    /// - `0.0` = no bounce (clay, soft material)
    /// - `1.0` = perfect bounce (ideal elastic collision)
    /// - `>1.0` = super bouncy (gains energy, unrealistic but fun!)
    /// - Typical values: 0.0-0.8
    pub fn restitution(&self) -> Real {
        self.material.restitution
    }

    /// Sets the restitution coefficient (bounciness).
    ///
    /// Controls how much velocity is preserved after impact. Higher values = more bounce.
    /// Works with other collider's restitution via the combine rule.
    pub fn set_restitution(&mut self, coefficient: Real) {
        self.material.restitution = coefficient
    }

    /// The combine rule used by this collider to combine its restitution
    /// coefficient with the restitution coefficient of the other collider it
    /// is in contact with.
    pub fn restitution_combine_rule(&self) -> CoefficientCombineRule {
        self.material.restitution_combine_rule
    }

    /// Sets the combine rule used by this collider to combine its restitution
    /// coefficient with the restitution coefficient of the other collider it
    /// is in contact with.
    pub fn set_restitution_combine_rule(&mut self, rule: CoefficientCombineRule) {
        self.material.restitution_combine_rule = rule;
    }

    /// Sets the total force magnitude beyond which a contact force event can be emitted.
    pub fn set_contact_force_event_threshold(&mut self, threshold: Real) {
        self.contact_force_event_threshold = threshold;
    }

    /// Converts this collider to/from a sensor.
    ///
    /// Sensors detect overlaps but don't create physical contact forces.
    /// Use `true` for trigger zones, `false` for solid collision shapes.
    pub fn set_sensor(&mut self, is_sensor: bool) {
        if is_sensor != self.is_sensor() {
            self.changes.insert(ColliderChanges::TYPE);
            self.coll_type = if is_sensor {
                ColliderType::Sensor
            } else {
                ColliderType::Solid
            };
        }
    }

    /// Returns `true` if this collider is active in the simulation.
    ///
    /// Disabled colliders are excluded from collision detection and physics.
    pub fn is_enabled(&self) -> bool {
        matches!(self.flags.enabled, ColliderEnabled::Enabled)
    }

    /// Enables or disables this collider.
    ///
    /// When disabled, the collider is excluded from all collision detection and physics.
    /// Useful for temporarily "turning off" colliders without removing them.
    pub fn set_enabled(&mut self, enabled: bool) {
        match self.flags.enabled {
            ColliderEnabled::Enabled | ColliderEnabled::DisabledByParent => {
                if !enabled {
                    self.changes.insert(ColliderChanges::ENABLED_OR_DISABLED);
                    self.flags.enabled = ColliderEnabled::Disabled;
                }
            }
            ColliderEnabled::Disabled => {
                if enabled {
                    self.changes.insert(ColliderChanges::ENABLED_OR_DISABLED);
                    self.flags.enabled = ColliderEnabled::Enabled;
                }
            }
        }
    }

    /// Sets the collider's position (for standalone colliders).
    ///
    /// For attached colliders, modify the parent body's position instead.
    /// This directly sets world-space position.
    pub fn set_translation(&mut self, translation: Vector) {
        self.changes.insert(ColliderChanges::POSITION);
        self.pos.0.translation = translation;
    }

    /// Sets the collider's rotation (for standalone colliders).
    ///
    /// For attached colliders, modify the parent body's rotation instead.
    pub fn set_rotation(&mut self, rotation: Rotation) {
        self.changes.insert(ColliderChanges::POSITION);
        self.pos.0.rotation = rotation;
    }

    /// Sets the collider's full pose (for standalone colliders).
    ///
    /// For attached colliders, modify the parent body instead.
    pub fn set_position(&mut self, position: Pose) {
        self.changes.insert(ColliderChanges::POSITION);
        self.pos.0 = position;
    }

    /// The current world-space position of this collider.
    ///
    /// For attached colliders, this is automatically updated when the parent body moves.
    /// For standalone colliders, this is the position you set directly.
    pub fn position(&self) -> &Pose {
        &self.pos
    }

    /// The current position vector of this collider (world coordinates).
    pub fn translation(&self) -> Vector {
        self.pos.0.translation
    }

    /// The current rotation/orientation of this collider.
    pub fn rotation(&self) -> Rotation {
        self.pos.0.rotation
    }

    /// The collider's position relative to its parent body (local coordinates).
    ///
    /// Returns `None` for standalone colliders. This is the offset from the parent body's origin.
    pub fn position_wrt_parent(&self) -> Option<&Pose> {
        self.parent.as_ref().map(|p| &p.pos_wrt_parent)
    }

    /// Changes this collider's position offset from its parent body.
    ///
    /// Useful for adjusting where a collider sits on a body without moving the whole body.
    /// Does nothing if the collider has no parent.
    pub fn set_translation_wrt_parent(&mut self, translation: Vector) {
        if let Some(parent) = self.parent.as_mut() {
            self.changes.insert(ColliderChanges::PARENT);
            parent.pos_wrt_parent.translation = translation;
        }
    }

    /// Changes this collider's rotation offset from its parent body.
    ///
    /// Rotates the collider relative to its parent. Does nothing if no parent.
    pub fn set_rotation_wrt_parent(&mut self, rotation: AngVector) {
        if let Some(parent) = self.parent.as_mut() {
            self.changes.insert(ColliderChanges::PARENT);
            parent.pos_wrt_parent.rotation = rotation_from_angle(rotation);
        }
    }

    /// Changes this collider's full pose (position + rotation) relative to its parent.
    ///
    /// Does nothing if the collider is not attached to a rigid-body.
    pub fn set_position_wrt_parent(&mut self, pos_wrt_parent: Pose) {
        if let Some(parent) = self.parent.as_mut() {
            self.changes.insert(ColliderChanges::PARENT);
            parent.pos_wrt_parent = pos_wrt_parent;
        }
    }

    /// The collision groups controlling what this collider can interact with.
    ///
    /// See [`InteractionGroups`] for details on collision filtering.
    pub fn collision_groups(&self) -> InteractionGroups {
        self.flags.collision_groups
    }

    /// Changes which collision groups this collider belongs to and can interact with.
    ///
    /// Use to control collision filtering (like changing layers).
    pub fn set_collision_groups(&mut self, groups: InteractionGroups) {
        if self.flags.collision_groups != groups {
            self.changes.insert(ColliderChanges::GROUPS);
            self.flags.collision_groups = groups;
        }
    }

    /// The solver groups for this collider (advanced collision filtering).
    ///
    /// Most users should use `collision_groups()` instead.
    pub fn solver_groups(&self) -> InteractionGroups {
        self.flags.solver_groups
    }

    /// Changes the solver groups (advanced contact resolution filtering).
    pub fn set_solver_groups(&mut self, groups: InteractionGroups) {
        if self.flags.solver_groups != groups {
            self.changes.insert(ColliderChanges::GROUPS);
            self.flags.solver_groups = groups;
        }
    }

    /// Returns the material properties (friction and restitution) of this collider.
    pub fn material(&self) -> &ColliderMaterial {
        &self.material
    }

    /// Returns the volume (3D) or area (2D) of this collider's shape.
    ///
    /// Used internally for mass calculations when density is set.
    pub fn volume(&self) -> Real {
        self.shape.mass_properties(1.0).mass()
    }

    /// The density of this collider (mass per unit volume).
    ///
    /// Used to automatically compute mass from the collider's volume.
    /// Returns an approximate density if mass was set directly instead.
    pub fn density(&self) -> Real {
        match &self.mprops {
            ColliderMassProps::Density(density) => *density,
            ColliderMassProps::Mass(mass) => {
                let inv_volume = self.shape.mass_properties(1.0).inv_mass;
                mass * inv_volume
            }
            ColliderMassProps::MassProperties(mprops) => {
                let inv_volume = self.shape.mass_properties(1.0).inv_mass;
                mprops.mass() * inv_volume
            }
        }
    }

    /// The mass contributed by this collider to its parent body.
    ///
    /// Either set directly or computed from density × volume.
    pub fn mass(&self) -> Real {
        match &self.mprops {
            ColliderMassProps::Density(density) => self.shape.mass_properties(*density).mass(),
            ColliderMassProps::Mass(mass) => *mass,
            ColliderMassProps::MassProperties(mprops) => mprops.mass(),
        }
    }

    /// Sets the uniform density of this collider.
    ///
    /// This will override any previous mass-properties set by [`Self::set_density`],
    /// [`Self::set_mass`], [`Self::set_mass_properties`], [`ColliderBuilder::density`],
    /// [`ColliderBuilder::mass`], or [`ColliderBuilder::mass_properties`]
    /// for this collider.
    ///
    /// The mass and angular inertia of this collider will be computed automatically based on its
    /// shape.
    pub fn set_density(&mut self, density: Real) {
        self.do_set_mass_properties(ColliderMassProps::Density(density));
    }

    /// Sets the mass of this collider.
    ///
    /// This will override any previous mass-properties set by [`Self::set_density`],
    /// [`Self::set_mass`], [`Self::set_mass_properties`], [`ColliderBuilder::density`],
    /// [`ColliderBuilder::mass`], or [`ColliderBuilder::mass_properties`]
    /// for this collider.
    ///
    /// The angular inertia of this collider will be computed automatically based on its shape
    /// and this mass value.
    pub fn set_mass(&mut self, mass: Real) {
        self.do_set_mass_properties(ColliderMassProps::Mass(mass));
    }

    /// Sets the mass properties of this collider.
    ///
    /// This will override any previous mass-properties set by [`Self::set_density`],
    /// [`Self::set_mass`], [`Self::set_mass_properties`], [`ColliderBuilder::density`],
    /// [`ColliderBuilder::mass`], or [`ColliderBuilder::mass_properties`]
    /// for this collider.
    pub fn set_mass_properties(&mut self, mass_properties: MassProperties) {
        self.do_set_mass_properties(ColliderMassProps::MassProperties(Box::new(mass_properties)))
    }

    fn do_set_mass_properties(&mut self, mprops: ColliderMassProps) {
        if mprops != self.mprops {
            self.changes |= ColliderChanges::LOCAL_MASS_PROPERTIES;
            self.mprops = mprops;
        }
    }

    /// The geometric shape of this collider (ball, cuboid, mesh, etc.).
    ///
    /// Returns a reference to the underlying shape object for reading properties
    /// or performing geometric queries.
    pub fn shape(&self) -> &dyn Shape {
        self.shape.as_ref()
    }

    /// A mutable reference to the geometric shape of this collider.
    ///
    /// If that shape is shared by multiple colliders, it will be
    /// cloned first so that `self` contains a unique copy of that
    /// shape that you can modify.
    pub fn shape_mut(&mut self) -> &mut dyn Shape {
        self.changes.insert(ColliderChanges::SHAPE);
        self.shape.make_mut()
    }

    /// Sets the shape of this collider.
    pub fn set_shape(&mut self, shape: SharedShape) {
        self.changes.insert(ColliderChanges::SHAPE);
        self.shape = shape;
    }

    /// Returns the shape as a `SharedShape` (reference-counted shape).
    ///
    /// Use `shape()` for the trait object, this for the concrete type.
    pub fn shared_shape(&self) -> &SharedShape {
        &self.shape
    }

    /// Computes the axis-aligned bounding box (AABB) of this collider.
    ///
    /// The AABB is the smallest box (aligned with world axes) that contains the shape.
    /// Doesn't include contact skin.
    pub fn compute_aabb(&self) -> Aabb {
        self.shape.compute_aabb(&self.pos)
    }

    /// Computes the AABB including contact skin and prediction distance.
    ///
    /// This is the AABB used for collision detection (slightly larger than the visual shape).
    pub fn compute_collision_aabb(&self, prediction: Real) -> Aabb {
        self.shape
            .compute_aabb(&self.pos)
            .loosened(self.contact_skin + prediction)
    }

    /// Computes the AABB swept from current position to `next_position`.
    ///
    /// Returns a box that contains the shape at both positions plus everything in between.
    /// Used for continuous collision detection.
    pub fn compute_swept_aabb(&self, next_position: &Pose) -> Aabb {
        self.shape.compute_swept_aabb(&self.pos, next_position)
    }

    // TODO: we have a lot of different AABB computation functions
    //       We should group them somehow.
    /// Computes the collider’s AABB for usage in a broad-phase.
    ///
    /// It takes into account soft-ccd, the contact skin, and the contact prediction.
    pub fn compute_broad_phase_aabb(
        &self,
        params: &IntegrationParameters,
        bodies: &RigidBodySet,
    ) -> Aabb {
        // Take soft-ccd into account by growing the aabb.
        let next_pose = self.parent.and_then(|p| {
            let parent = bodies.get(p.handle)?;
            (parent.soft_ccd_prediction() > 0.0).then(|| {
                parent.predict_position_using_velocity_and_forces_with_max_dist(
                    params.dt,
                    parent.soft_ccd_prediction(),
                ) * p.pos_wrt_parent
            })
        });

        let prediction_distance = params.prediction_distance();
        let mut aabb = self.compute_collision_aabb(prediction_distance / 2.0);
        if let Some(next_pose) = next_pose {
            let next_aabb = self
                .shape
                .compute_aabb(&next_pose)
                .loosened(self.contact_skin() + prediction_distance / 2.0);
            aabb.merge(&next_aabb);
        }

        aabb
    }

    /// Computes the full mass properties (mass, center of mass, angular inertia).
    ///
    /// Returns properties in the collider's local coordinate system.
    pub fn mass_properties(&self) -> MassProperties {
        self.mprops.mass_properties(&*self.shape)
    }

    /// Returns the force threshold for contact force events.
    ///
    /// When contact forces exceed this value, a `ContactForceEvent` is generated.
    /// See `set_contact_force_event_threshold()` for details.
    pub fn contact_force_event_threshold(&self) -> Real {
        self.contact_force_event_threshold
    }
}

/// A builder for creating colliders with custom shapes and properties.
///
/// This builder lets you create collision shapes and configure their physical properties
/// (friction, bounciness, density, etc.) before adding them to your world.
///
/// # Common shapes
///
/// - [`ball(radius)`](Self::ball) - Sphere (3D) or circle (2D)
/// - [`cuboid(hx, hy, hz)`](Self::cuboid) - Box with half-extents
/// - [`capsule_y(half_height, radius)`](Self::capsule_y) - Pill shape (great for characters)
/// - [`trimesh(vertices, indices)`](Self::trimesh) - Triangle mesh for complex geometry
/// - [`heightfield(...)`](Self::heightfield) - Terrain from height data
///
/// # Example
///
/// ```ignore
/// // Create a bouncy ball
/// let collider = ColliderBuilder::ball(0.5)
///     .restitution(0.9)       // Very bouncy
///     .friction(0.1)          // Low friction (slippery)
///     .density(2.0);           // Heavy material
/// colliders.insert_with_parent(collider, body_handle, &mut bodies);
/// ```
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[must_use = "Builder functions return the updated builder"]
pub struct ColliderBuilder {
    /// The shape of the collider to be built.
    pub shape: SharedShape,
    /// Controls the way the collider’s mass-properties are computed.
    pub mass_properties: ColliderMassProps,
    /// The friction coefficient of the collider to be built.
    pub friction: Real,
    /// The rule used to combine two friction coefficients.
    pub friction_combine_rule: CoefficientCombineRule,
    /// The restitution coefficient of the collider to be built.
    pub restitution: Real,
    /// The rule used to combine two restitution coefficients.
    pub restitution_combine_rule: CoefficientCombineRule,
    /// The position of this collider.
    pub position: Pose,
    /// Is this collider a sensor?
    pub is_sensor: bool,
    /// Contact pairs enabled for this collider.
    pub active_collision_types: ActiveCollisionTypes,
    /// Physics hooks enabled for this collider.
    pub active_hooks: ActiveHooks,
    /// Events enabled for this collider.
    pub active_events: ActiveEvents,
    /// The user-data of the collider being built.
    pub user_data: u128,
    /// The collision groups for the collider being built.
    pub collision_groups: InteractionGroups,
    /// The solver groups for the collider being built.
    pub solver_groups: InteractionGroups,
    /// Will the collider being built be enabled?
    pub enabled: bool,
    /// The total force magnitude beyond which a contact force event can be emitted.
    pub contact_force_event_threshold: Real,
    /// An extra thickness around the collider shape to keep them further apart when colliding.
    pub contact_skin: Real,
}

impl Default for ColliderBuilder {
    fn default() -> Self {
        Self::ball(0.5)
    }
}

impl ColliderBuilder {
    /// Initialize a new collider builder with the given shape.
    pub fn new(shape: SharedShape) -> Self {
        Self {
            shape,
            mass_properties: ColliderMassProps::default(),
            friction: Self::default_friction(),
            restitution: 0.0,
            position: Pose::IDENTITY,
            is_sensor: false,
            user_data: 0,
            collision_groups: InteractionGroups::all(),
            solver_groups: InteractionGroups::all(),
            friction_combine_rule: CoefficientCombineRule::Average,
            restitution_combine_rule: CoefficientCombineRule::Average,
            active_collision_types: ActiveCollisionTypes::default(),
            active_hooks: ActiveHooks::empty(),
            active_events: ActiveEvents::empty(),
            enabled: true,
            contact_force_event_threshold: 0.0,
            contact_skin: 0.0,
        }
    }

    /// Initialize a new collider builder with a compound shape.
    pub fn compound(shapes: Vec<(Pose, SharedShape)>) -> Self {
        Self::new(SharedShape::compound(shapes))
    }

    /// Creates a sphere (3D) or circle (2D) collider.
    ///
    /// The simplest and fastest collision shape. Use for:
    /// - Balls and spheres
    /// - Approximate round objects
    /// - Projectiles
    /// - Particles
    ///
    /// # Parameters
    /// * `radius` - The sphere's radius
    pub fn ball(radius: Real) -> Self {
        Self::new(SharedShape::ball(radius))
    }

    /// Initialize a new collider build with a half-space shape defined by the outward normal
    /// of its planar boundary.
    pub fn halfspace(outward_normal: Unit<Vector>) -> Self {
        Self::new(SharedShape::halfspace(outward_normal.into_inner()))
    }

    /// Initializes a shape made of voxels.
    ///
    /// Each voxel has the size `voxel_size` and grid coordinate given by `voxels`.
    /// The `primitive_geometry` controls the behavior of collision detection at voxels boundaries.
    ///
    /// For initializing a voxels shape from points in space, see [`Self::voxels_from_points`].
    /// For initializing a voxels shape from a mesh to voxelize, see [`Self::voxelized_mesh`].
    pub fn voxels(voxel_size: Vector, voxels: &[IVector]) -> Self {
        Self::new(SharedShape::voxels(voxel_size, voxels))
    }

    /// Initializes a collider made of voxels.
    ///
    /// Each voxel has the size `voxel_size` and contains at least one point from `centers`.
    /// The `primitive_geometry` controls the behavior of collision detection at voxels boundaries.
    pub fn voxels_from_points(voxel_size: Vector, points: &[Vector]) -> Self {
        Self::new(SharedShape::voxels_from_points(voxel_size, points))
    }

    /// Initializes a voxels obtained from the decomposition of the given trimesh (in 3D)
    /// or polyline (in 2D) into voxelized convex parts.
    pub fn voxelized_mesh(
        vertices: &[Vector],
        indices: &[[u32; DIM]],
        voxel_size: Real,
        fill_mode: FillMode,
    ) -> Self {
        Self::new(SharedShape::voxelized_mesh(
            vertices, indices, voxel_size, fill_mode,
        ))
    }

    /// Initialize a new collider builder with a cylindrical shape defined by its half-height
    /// (along the Y axis) and its radius.
    #[cfg(feature = "dim3")]
    pub fn cylinder(half_height: Real, radius: Real) -> Self {
        Self::new(SharedShape::cylinder(half_height, radius))
    }

    /// Initialize a new collider builder with a rounded cylindrical shape defined by its half-height
    /// (along the Y axis), its radius, and its roundedness (the radius of the sphere used for
    /// dilating the cylinder).
    #[cfg(feature = "dim3")]
    pub fn round_cylinder(half_height: Real, radius: Real, border_radius: Real) -> Self {
        Self::new(SharedShape::round_cylinder(
            half_height,
            radius,
            border_radius,
        ))
    }

    /// Initialize a new collider builder with a cone shape defined by its half-height
    /// (along the Y axis) and its basis radius.
    #[cfg(feature = "dim3")]
    pub fn cone(half_height: Real, radius: Real) -> Self {
        Self::new(SharedShape::cone(half_height, radius))
    }

    /// Initialize a new collider builder with a rounded cone shape defined by its half-height
    /// (along the Y axis), its radius, and its roundedness (the radius of the sphere used for
    /// dilating the cylinder).
    #[cfg(feature = "dim3")]
    pub fn round_cone(half_height: Real, radius: Real, border_radius: Real) -> Self {
        Self::new(SharedShape::round_cone(half_height, radius, border_radius))
    }

    /// Initialize a new collider builder with a cuboid shape defined by its half-extents.
    #[cfg(feature = "dim2")]
    pub fn cuboid(hx: Real, hy: Real) -> Self {
        Self::new(SharedShape::cuboid(hx, hy))
    }

    /// Initialize a new collider builder with a round cuboid shape defined by its half-extents
    /// and border radius.
    #[cfg(feature = "dim2")]
    pub fn round_cuboid(hx: Real, hy: Real, border_radius: Real) -> Self {
        Self::new(SharedShape::round_cuboid(hx, hy, border_radius))
    }

    /// Initialize a new collider builder with a capsule defined from its endpoints.
    ///
    /// See also [`ColliderBuilder::capsule_x`], [`ColliderBuilder::capsule_y`],
    /// (and `ColliderBuilder::capsule_z` in 3D only)
    /// for a simpler way to build capsules with common
    /// orientations.
    pub fn capsule_from_endpoints(a: Vector, b: Vector, radius: Real) -> Self {
        Self::new(SharedShape::capsule(a, b, radius))
    }

    /// Initialize a new collider builder with a capsule shape aligned with the `x` axis.
    pub fn capsule_x(half_height: Real, radius: Real) -> Self {
        Self::new(SharedShape::capsule_x(half_height, radius))
    }

    /// Creates a capsule (pill-shaped) collider aligned with the Y axis.
    ///
    /// Capsules are cylinders with hemispherical caps. Excellent for characters because:
    /// - Smooth collision (no getting stuck on edges)
    /// - Good for upright objects (characters, trees)
    /// - Fast collision detection
    ///
    /// # Parameters
    /// * `half_height` - Half the height of the cylindrical part (not including caps)
    /// * `radius` - Radius of the cylinder and caps
    ///
    /// **Example**: `capsule_y(1.0, 0.5)` creates a 3.0 tall capsule (1.0×2 cylinder + 0.5×2 caps)
    pub fn capsule_y(half_height: Real, radius: Real) -> Self {
        Self::new(SharedShape::capsule_y(half_height, radius))
    }

    /// Initialize a new collider builder with a capsule shape aligned with the `z` axis.
    #[cfg(feature = "dim3")]
    pub fn capsule_z(half_height: Real, radius: Real) -> Self {
        Self::new(SharedShape::capsule_z(half_height, radius))
    }

    /// Creates a box collider defined by its half-extents (half-widths).
    ///
    /// Very fast collision detection. Use for:
    /// - Boxes and crates
    /// - Buildings and rooms
    /// - Most rectangular objects
    ///
    /// # Parameters (3D)
    /// * `hx`, `hy`, `hz` - Half-extents (half the width) along each axis
    ///
    /// **Example**: `cuboid(1.0, 0.5, 2.0)` creates a box with full size 2×1×4
    #[cfg(feature = "dim3")]
    pub fn cuboid(hx: Real, hy: Real, hz: Real) -> Self {
        Self::new(SharedShape::cuboid(hx, hy, hz))
    }

    /// Initialize a new collider builder with a round cuboid shape defined by its half-extents
    /// and border radius.
    #[cfg(feature = "dim3")]
    pub fn round_cuboid(hx: Real, hy: Real, hz: Real, border_radius: Real) -> Self {
        Self::new(SharedShape::round_cuboid(hx, hy, hz, border_radius))
    }

    /// Creates a line segment collider between two points.
    ///
    /// Useful for thin barriers, edges, or 2D line-based collision.
    /// Has no thickness - purely a mathematical line.
    pub fn segment(a: Vector, b: Vector) -> Self {
        Self::new(SharedShape::segment(a, b))
    }

    /// Creates a single triangle collider.
    ///
    /// Use for simple 3-sided shapes or as building blocks for more complex geometry.
    pub fn triangle(a: Vector, b: Vector, c: Vector) -> Self {
        Self::new(SharedShape::triangle(a, b, c))
    }

    /// Initializes a collider builder with a triangle shape with round corners.
    pub fn round_triangle(a: Vector, b: Vector, c: Vector, border_radius: Real) -> Self {
        Self::new(SharedShape::round_triangle(a, b, c, border_radius))
    }

    /// Initializes a collider builder with a polyline shape defined by its vertex and index buffers.
    pub fn polyline(vertices: Vec<Vector>, indices: Option<Vec<[u32; 2]>>) -> Self {
        Self::new(SharedShape::polyline(vertices, indices))
    }

    /// Creates a triangle mesh collider from vertices and triangle indices.
    ///
    /// Use for complex, arbitrary shapes like:
    /// - Level geometry and terrain
    /// - Imported 3D models
    /// - Custom irregular shapes
    ///
    /// **Performance note**: Triangle meshes are slower than primitive shapes (balls, boxes, capsules).
    /// Consider using compound shapes or simpler approximations when possible.
    ///
    /// # Parameters
    /// * `vertices` - Array of 3D points
    /// * `indices` - Array of triangles, each is 3 indices into the vertex array
    ///
    /// # Example
    /// ```ignore
    /// use rapier3d::prelude::*;
    /// use nalgebra::Point3;
    ///
    /// let vertices = vec![
    ///     Point3::new(0.0, 0.0, 0.0),
    ///     Point3::new(1.0, 0.0, 0.0),
    ///     Point3::new(0.0, 1.0, 0.0),
    /// ];
    /// let triangle: [u32; 3] = [0, 1, 2];
    /// let indices = vec![triangle];  // One triangle
    /// let collider = ColliderBuilder::trimesh(vertices, indices)?;
    /// ```
    pub fn trimesh(
        vertices: Vec<Vector>,
        indices: Vec<[u32; 3]>,
    ) -> Result<Self, TriMeshBuilderError> {
        Ok(Self::new(SharedShape::trimesh(vertices, indices)?))
    }

    /// Initializes a collider builder with a triangle mesh shape defined by its vertex and index buffers and
    /// flags controlling its pre-processing.
    pub fn trimesh_with_flags(
        vertices: Vec<Vector>,
        indices: Vec<[u32; 3]>,
        flags: TriMeshFlags,
    ) -> Result<Self, TriMeshBuilderError> {
        Ok(Self::new(SharedShape::trimesh_with_flags(
            vertices, indices, flags,
        )?))
    }

    /// Initializes a collider builder with a shape converted from the given triangle mesh index
    /// and vertex buffer.
    ///
    /// All the conversion variants could be achieved with other constructors of [`ColliderBuilder`]
    /// but having this specified by an enum can occasionally be easier or more flexible (determined
    /// at runtime).
    pub fn converted_trimesh(
        vertices: Vec<Vector>,
        indices: Vec<[u32; 3]>,
        converter: MeshConverter,
    ) -> Result<Self, MeshConverterError> {
        let (shape, pose) = converter.convert(vertices, indices)?;
        Ok(Self::new(shape).position(pose))
    }

    /// Creates a compound collider by decomposing a mesh/polyline into convex pieces.
    ///
    /// Concave shapes (like an 'L' or 'C') are automatically broken into multiple convex
    /// parts for efficient collision detection. This is often faster than using a trimesh.
    ///
    /// Uses the V-HACD algorithm. Good for imported models that aren't already convex.
    pub fn convex_decomposition(vertices: &[Vector], indices: &[[u32; DIM]]) -> Self {
        Self::new(SharedShape::convex_decomposition(vertices, indices))
    }

    /// Initializes a collider builder with a compound shape obtained from the decomposition of
    /// the given trimesh (in 3D) or polyline (in 2D) into convex parts dilated with round corners.
    pub fn round_convex_decomposition(
        vertices: &[Vector],
        indices: &[[u32; DIM]],
        border_radius: Real,
    ) -> Self {
        Self::new(SharedShape::round_convex_decomposition(
            vertices,
            indices,
            border_radius,
        ))
    }

    /// Initializes a collider builder with a compound shape obtained from the decomposition of
    /// the given trimesh (in 3D) or polyline (in 2D) into convex parts.
    pub fn convex_decomposition_with_params(
        vertices: &[Vector],
        indices: &[[u32; DIM]],
        params: &VHACDParameters,
    ) -> Self {
        Self::new(SharedShape::convex_decomposition_with_params(
            vertices, indices, params,
        ))
    }

    /// Initializes a collider builder with a compound shape obtained from the decomposition of
    /// the given trimesh (in 3D) or polyline (in 2D) into convex parts dilated with round corners.
    pub fn round_convex_decomposition_with_params(
        vertices: &[Vector],
        indices: &[[u32; DIM]],
        params: &VHACDParameters,
        border_radius: Real,
    ) -> Self {
        Self::new(SharedShape::round_convex_decomposition_with_params(
            vertices,
            indices,
            params,
            border_radius,
        ))
    }

    /// Creates the smallest convex shape that contains all the given points.
    ///
    /// Computes the "shrink-wrap" around a point cloud. Useful for:
    /// - Creating collision shapes from vertex data
    /// - Approximating complex shapes with a simpler convex one
    ///
    /// Returns `None` if the points don't form a valid convex shape.
    ///
    /// **Performance**: Convex shapes are much faster than triangle meshes!
    pub fn convex_hull(points: &[Vector]) -> Option<Self> {
        SharedShape::convex_hull(points).map(Self::new)
    }

    /// Initializes a new collider builder with a round 2D convex polygon or 3D convex polyhedron
    /// obtained after computing the convex-hull of the given points. The shape is dilated
    /// by a sphere of radius `border_radius`.
    pub fn round_convex_hull(points: &[Vector], border_radius: Real) -> Option<Self> {
        SharedShape::round_convex_hull(points, border_radius).map(Self::new)
    }

    /// Creates a new collider builder that is a convex polygon formed by the
    /// given polyline assumed to be convex (no convex-hull will be automatically
    /// computed).
    #[cfg(feature = "dim2")]
    pub fn convex_polyline(points: Vec<Vector>) -> Option<Self> {
        SharedShape::convex_polyline(points).map(Self::new)
    }

    /// Creates a new collider builder that is a round convex polygon formed by the
    /// given polyline assumed to be convex (no convex-hull will be automatically
    /// computed). The polygon shape is dilated by a sphere of radius `border_radius`.
    #[cfg(feature = "dim2")]
    pub fn round_convex_polyline(points: Vec<Vector>, border_radius: Real) -> Option<Self> {
        SharedShape::round_convex_polyline(points, border_radius).map(Self::new)
    }

    /// Creates a new collider builder that is a convex polyhedron formed by the
    /// given triangle-mesh assumed to be convex (no convex-hull will be automatically
    /// computed).
    #[cfg(feature = "dim3")]
    pub fn convex_mesh(points: Vec<Vector>, indices: &[[u32; 3]]) -> Option<Self> {
        SharedShape::convex_mesh(points, indices).map(Self::new)
    }

    /// Creates a new collider builder that is a round convex polyhedron formed by the
    /// given triangle-mesh assumed to be convex (no convex-hull will be automatically
    /// computed). The triangle mesh shape is dilated by a sphere of radius `border_radius`.
    #[cfg(feature = "dim3")]
    pub fn round_convex_mesh(
        points: Vec<Vector>,
        indices: &[[u32; 3]],
        border_radius: Real,
    ) -> Option<Self> {
        SharedShape::round_convex_mesh(points, indices, border_radius).map(Self::new)
    }

    /// Initializes a collider builder with a heightfield shape defined by its set of height and a scale
    /// factor along each coordinate axis.
    #[cfg(feature = "dim2")]
    pub fn heightfield(heights: Vec<Real>, scale: Vector) -> Self {
        Self::new(SharedShape::heightfield(heights, scale))
    }

    /// Creates a terrain/landscape collider from a 2D grid of height values.
    ///
    /// Perfect for outdoor terrain in 3D games. The heightfield is a grid where each cell
    /// stores a height value, creating a landscape surface.
    ///
    /// Use for:
    /// - Terrain and landscapes
    /// - Hills and valleys
    /// - Ground surfaces in open worlds
    ///
    /// # Parameters
    /// * `heights` - 2D matrix of height values (Y coordinates)
    /// * `scale` - Size of each grid cell in X and Z directions
    ///
    /// **Performance**: Much faster than triangle meshes for terrain!
    #[cfg(feature = "dim3")]
    pub fn heightfield(heights: Array2<Real>, scale: Vector) -> Self {
        Self::new(SharedShape::heightfield(heights, scale))
    }

    /// Initializes a collider builder with a heightfield shape defined by its set of height and a scale
    /// factor along each coordinate axis.
    #[cfg(feature = "dim3")]
    pub fn heightfield_with_flags(
        heights: Array2<Real>,
        scale: Vector,
        flags: HeightFieldFlags,
    ) -> Self {
        Self::new(SharedShape::heightfield_with_flags(heights, scale, flags))
    }

    /// Returns the default friction value used when not specified (0.5).
    pub fn default_friction() -> Real {
        0.5
    }

    /// Returns the default density value used when not specified (1.0).
    pub fn default_density() -> Real {
        1.0
    }

    /// Stores custom user data with this collider (128-bit integer).
    ///
    /// Use to associate game data (entity ID, type, etc.) with physics objects.
    ///
    /// # Example
    /// ```ignore
    /// let collider = ColliderBuilder::ball(0.5)
    ///     .user_data(entity_id as u128)
    ///     .build();
    /// ```
    pub fn user_data(mut self, data: u128) -> Self {
        self.user_data = data;
        self
    }

    /// Sets which collision groups this collider belongs to and can interact with.
    ///
    /// Use this to control what can collide with what (like collision layers).
    /// See [`InteractionGroups`] for examples.
    ///
    /// # Example
    /// ```ignore
    /// // Player bullet: in group 1, only hits group 2 (enemies)
    /// let groups = InteractionGroups::new(Group::GROUP_1, Group::GROUP_2);
    /// let bullet = ColliderBuilder::ball(0.1)
    ///     .collision_groups(groups)
    ///     .build();
    /// ```
    pub fn collision_groups(mut self, groups: InteractionGroups) -> Self {
        self.collision_groups = groups;
        self
    }

    /// Sets solver groups (advanced collision filtering for contact resolution).
    ///
    /// Similar to collision_groups but specifically for the contact solver.
    /// Most users should use `collision_groups()` instead - this is for advanced scenarios
    /// where you want collisions detected but not resolved (e.g., one-way platforms).
    pub fn solver_groups(mut self, groups: InteractionGroups) -> Self {
        self.solver_groups = groups;
        self
    }

    /// Makes this collider a sensor (trigger zone) instead of a solid collision shape.
    ///
    /// Sensors detect overlaps but don't create physical collisions. Use for:
    /// - Trigger zones (checkpoints, danger areas)
    /// - Collectible item detection
    /// - Proximity sensors
    /// - Win/lose conditions
    ///
    /// You'll receive collision events when objects enter/exit the sensor.
    ///
    /// # Example
    /// ```ignore
    /// let trigger = ColliderBuilder::cuboid(5.0, 5.0, 5.0)
    ///     .sensor(true)
    ///     .build();
    /// ```
    pub fn sensor(mut self, is_sensor: bool) -> Self {
        self.is_sensor = is_sensor;
        self
    }

    /// Enables custom physics hooks for this collider (advanced).
    ///
    /// See [`ActiveHooks`](crate::pipeline::ActiveHooks) for details on custom collision filtering.
    pub fn active_hooks(mut self, active_hooks: ActiveHooks) -> Self {
        self.active_hooks = active_hooks;
        self
    }

    /// Enables event generation for this collider.
    ///
    /// Set to `ActiveEvents::COLLISION_EVENTS` for start/stop notifications.
    /// Set to `ActiveEvents::CONTACT_FORCE_EVENTS` for force threshold events.
    ///
    /// # Example
    /// ```ignore
    /// let sensor = ColliderBuilder::ball(1.0)
    ///     .sensor(true)
    ///     .active_events(ActiveEvents::COLLISION_EVENTS)
    ///     .build();
    /// ```
    pub fn active_events(mut self, active_events: ActiveEvents) -> Self {
        self.active_events = active_events;
        self
    }

    /// Sets which body type combinations can collide with this collider.
    ///
    /// See [`ActiveCollisionTypes`] for details. Most users don't need to change this.
    pub fn active_collision_types(mut self, active_collision_types: ActiveCollisionTypes) -> Self {
        self.active_collision_types = active_collision_types;
        self
    }

    /// Sets the friction coefficient (slipperiness) for this collider.
    ///
    /// - `0.0` = ice (very slippery)
    /// - `0.5` = wood on wood
    /// - `1.0` = rubber (high grip)
    ///
    /// Default is `0.5`.
    pub fn friction(mut self, friction: Real) -> Self {
        self.friction = friction;
        self
    }

    /// Sets how friction coefficients are combined when two colliders touch.
    ///
    /// Options: Average, Min, Max, Multiply. Default is Average.
    /// Most games can ignore this and use the default.
    pub fn friction_combine_rule(mut self, rule: CoefficientCombineRule) -> Self {
        self.friction_combine_rule = rule;
        self
    }

    /// Sets the restitution coefficient (bounciness) for this collider.
    ///
    /// - `0.0` = no bounce (clay, soft)
    /// - `0.5` = moderate bounce
    /// - `1.0` = perfect elastic bounce
    /// - `>1.0` = super bouncy (gains energy!)
    ///
    /// Default is `0.0`.
    pub fn restitution(mut self, restitution: Real) -> Self {
        self.restitution = restitution;
        self
    }

    /// Sets the rule to be used to combine two restitution coefficients in a contact.
    pub fn restitution_combine_rule(mut self, rule: CoefficientCombineRule) -> Self {
        self.restitution_combine_rule = rule;
        self
    }

    /// Sets the density (mass per unit volume) of this collider.
    ///
    /// Mass will be computed as: `density × volume`. Common densities:
    /// - `1000.0` = water
    /// - `2700.0` = aluminum
    /// - `7850.0` = steel
    ///
    /// ⚠️ Use either `density()` OR `mass()`, not both (last call wins).
    ///
    /// # Example
    /// ```ignore
    /// let steel_ball = ColliderBuilder::ball(0.5).density(7850.0).build();
    /// ```
    pub fn density(mut self, density: Real) -> Self {
        self.mass_properties = ColliderMassProps::Density(density);
        self
    }

    /// Sets the total mass of this collider directly.
    ///
    /// Angular inertia is computed automatically from the shape and mass.
    ///
    /// ⚠️ Use either `mass()` OR `density()`, not both (last call wins).
    ///
    /// # Example
    /// ```ignore
    /// // 10kg ball regardless of its radius
    /// let collider = ColliderBuilder::ball(0.5).mass(10.0).build();
    /// ```
    pub fn mass(mut self, mass: Real) -> Self {
        self.mass_properties = ColliderMassProps::Mass(mass);
        self
    }

    /// Sets the mass properties of the collider this builder will build.
    ///
    /// This will be overridden by a call to [`Self::density`] or [`Self::mass`] so it only
    /// makes sense to call either [`Self::density`] or [`Self::mass`] or [`Self::mass_properties`].
    pub fn mass_properties(mut self, mass_properties: MassProperties) -> Self {
        self.mass_properties = ColliderMassProps::MassProperties(Box::new(mass_properties));
        self
    }

    /// Sets the force threshold for triggering contact force events.
    ///
    /// When total contact force exceeds this value, a `ContactForceEvent` is generated
    /// (if `ActiveEvents::CONTACT_FORCE_EVENTS` is enabled).
    ///
    /// Use for detecting hard impacts, breaking objects, or damage systems.
    ///
    /// # Example
    /// ```ignore
    /// let glass = ColliderBuilder::cuboid(1.0, 1.0, 0.1)
    ///     .active_events(ActiveEvents::CONTACT_FORCE_EVENTS)
    ///     .contact_force_event_threshold(1000.0)  // Break at 1000N
    ///     .build();
    /// ```
    pub fn contact_force_event_threshold(mut self, threshold: Real) -> Self {
        self.contact_force_event_threshold = threshold;
        self
    }

    /// Sets where the collider sits relative to its parent body.
    ///
    /// For attached colliders, this is the offset from the body's origin.
    /// For standalone colliders, this is the world position.
    ///
    /// # Example
    /// ```ignore
    /// // Collider offset 2 units to the right of the body
    /// let collider = ColliderBuilder::ball(0.5)
    ///     .translation(vector![2.0, 0.0, 0.0])
    ///     .build();
    /// ```
    pub fn translation(mut self, translation: Vector) -> Self {
        self.position.translation = translation;
        self
    }

    /// Sets the collider's rotation relative to its parent body.
    ///
    /// For attached colliders, this rotates the collider relative to the body.
    /// For standalone colliders, this is the world rotation.
    pub fn rotation(mut self, angle: AngVector) -> Self {
        self.position.rotation = rotation_from_angle(angle);
        self
    }

    /// Sets the collider's full pose (position + rotation) relative to its parent.
    ///
    /// For attached colliders, this is relative to the parent body.
    /// For standalone colliders, this is the world pose.
    pub fn position(mut self, pos: Pose) -> Self {
        self.position = pos;
        self
    }

    /// Sets the initial position (translation and orientation) of the collider to be created,
    /// relative to the rigid-body it is attached to.
    #[deprecated(note = "Use `.position` instead.")]
    pub fn position_wrt_parent(mut self, pos: Pose) -> Self {
        self.position = pos;
        self
    }

    /// Set the position of this collider in the local-space of the rigid-body it is attached to.
    #[deprecated(note = "Use `.position` instead.")]
    pub fn delta(mut self, delta: Pose) -> Self {
        self.position = delta;
        self
    }

    /// Sets the contact skin of the collider.
    ///
    /// The contact skin acts as if the collider was enlarged with a skin of width `skin_thickness`
    /// around it, keeping objects further apart when colliding.
    ///
    /// A non-zero contact skin can increase performance, and in some cases, stability. However
    /// it creates a small gap between colliding object (equal to the sum of their skin). If the
    /// skin is sufficiently small, this might not be visually significant or can be hidden by the
    /// rendering assets.
    pub fn contact_skin(mut self, skin_thickness: Real) -> Self {
        self.contact_skin = skin_thickness;
        self
    }

    /// Sets whether this collider starts enabled or disabled.
    ///
    /// Default is `true` (enabled). Set to `false` to create a disabled collider.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Finalizes the collider and returns it, ready to be added to the world.
    ///
    /// # Example
    /// ```ignore
    /// let collider = ColliderBuilder::ball(0.5)
    ///     .friction(0.7)
    ///     .build();
    /// colliders.insert_with_parent(collider, body_handle, &mut bodies);
    /// ```
    pub fn build(&self) -> Collider {
        let shape = self.shape.clone();
        let material = ColliderMaterial {
            friction: self.friction,
            restitution: self.restitution,
            friction_combine_rule: self.friction_combine_rule,
            restitution_combine_rule: self.restitution_combine_rule,
        };
        let flags = ColliderFlags {
            collision_groups: self.collision_groups,
            solver_groups: self.solver_groups,
            active_collision_types: self.active_collision_types,
            active_hooks: self.active_hooks,
            active_events: self.active_events,
            enabled: if self.enabled {
                ColliderEnabled::Enabled
            } else {
                ColliderEnabled::Disabled
            },
        };
        let changes = ColliderChanges::all();
        let pos = ColliderPosition(self.position);
        let coll_type = if self.is_sensor {
            ColliderType::Sensor
        } else {
            ColliderType::Solid
        };

        Collider {
            shape,
            mprops: self.mass_properties.clone(),
            material,
            parent: None,
            changes,
            pos,
            flags,
            coll_type,
            contact_force_event_threshold: self.contact_force_event_threshold,
            contact_skin: self.contact_skin,
            user_data: self.user_data,
        }
    }
}

impl From<ColliderBuilder> for Collider {
    fn from(val: ColliderBuilder) -> Collider {
        val.build()
    }
}
