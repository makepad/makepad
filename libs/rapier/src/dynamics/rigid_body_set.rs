use crate::data::{Arena, HasModifiedFlag, ModifiedObjects};
use crate::dynamics::{
    ImpulseJointSet, IslandManager, MultibodyJointSet, RigidBody, RigidBodyBuilder,
    RigidBodyChanges, RigidBodyHandle,
};
use crate::geometry::ColliderSet;
use std::ops::{Index, IndexMut};

#[cfg(doc)]
use crate::pipeline::PhysicsPipeline;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
/// A pair of rigid body handles.
pub struct BodyPair {
    /// The first rigid body handle.
    pub body1: RigidBodyHandle,
    /// The second rigid body handle.
    pub body2: RigidBodyHandle,
}

impl BodyPair {
    /// Builds a new pair of rigid-body handles.
    pub fn new(body1: RigidBodyHandle, body2: RigidBodyHandle) -> Self {
        BodyPair { body1, body2 }
    }
}

pub(crate) type ModifiedRigidBodies = ModifiedObjects<RigidBodyHandle, RigidBody>;

impl HasModifiedFlag for RigidBody {
    #[inline]
    fn has_modified_flag(&self) -> bool {
        self.changes.contains(RigidBodyChanges::IN_MODIFIED_SET)
    }

    #[inline]
    fn set_modified_flag(&mut self) {
        self.changes |= RigidBodyChanges::IN_MODIFIED_SET;
    }
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Default, Debug)]
/// The collection that stores all rigid bodies in your physics world.
///
/// This is where you add, remove, and access all your physics objects. Think of it as
/// a "database" of all rigid bodies, where each body gets a unique handle for fast lookup.
///
/// # Why use handles?
///
/// Instead of storing bodies directly, you get back a [`RigidBodyHandle`] when inserting.
/// This handle is lightweight (just an index + generation) and remains valid even if other
/// bodies are removed, protecting you from use-after-free bugs.
///
/// # Example
///
/// ```
/// # use rapier3d::prelude::*;
/// let mut bodies = RigidBodySet::new();
///
/// // Add a dynamic body
/// let handle = bodies.insert(RigidBodyBuilder::dynamic());
///
/// // Access it later
/// if let Some(body) = bodies.get_mut(handle) {
///     body.apply_impulse(Vector::new(0.0, 10.0, 0.0), true);
/// }
/// ```
pub struct RigidBodySet {
    // NOTE: the pub(crate) are needed by the broad phase
    // to avoid borrowing issues. It is also needed for
    // parallelism because the `Receiver` breaks the Sync impl.
    // Could we avoid this?
    pub(crate) bodies: Arena<RigidBody>,
    pub(crate) modified_bodies: ModifiedRigidBodies,
    #[cfg_attr(feature = "serde-serialize", serde(skip))]
    pub(crate) default_fixed: RigidBody,
}

impl RigidBodySet {
    /// Creates a new empty collection of rigid bodies.
    ///
    /// Call this once when setting up your physics world. The collection will
    /// automatically grow as you add more bodies.
    pub fn new() -> Self {
        RigidBodySet {
            bodies: Arena::new(),
            modified_bodies: ModifiedObjects::default(),
            default_fixed: RigidBodyBuilder::fixed().build(),
        }
    }

    /// Creates a new collection with pre-allocated space for the given number of bodies.
    ///
    /// Use this if you know approximately how many bodies you'll need, to avoid
    /// multiple reallocations as the collection grows.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// // You know you'll have ~1000 bodies
    /// let mut bodies = RigidBodySet::with_capacity(1000);
    /// ```
    pub fn with_capacity(capacity: usize) -> Self {
        RigidBodySet {
            bodies: Arena::with_capacity(capacity),
            modified_bodies: ModifiedRigidBodies::with_capacity(capacity),
            default_fixed: RigidBodyBuilder::fixed().build(),
        }
    }

    pub(crate) fn take_modified(&mut self) -> ModifiedRigidBodies {
        std::mem::take(&mut self.modified_bodies)
    }

    /// Returns how many rigid bodies are currently in this collection.
    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    /// Returns `true` if there are no rigid bodies in this collection.
    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    /// Checks if the given handle points to a valid rigid body that still exists.
    ///
    /// Returns `false` if the body was removed or the handle is invalid.
    pub fn contains(&self, handle: RigidBodyHandle) -> bool {
        self.bodies.contains(handle.0)
    }

    /// Adds a rigid body to the world and returns its handle for future access.
    ///
    /// The handle is how you'll refer to this body later (to move it, apply forces, etc.).
    /// Keep the handle somewhere accessible - you can't get the body back without it!
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// let handle = bodies.insert(
    ///     RigidBodyBuilder::dynamic()
    ///         .translation(Vector::new(0.0, 5.0, 0.0))
    ///         .build()
    /// );
    /// // Store `handle` to access this body later
    /// ```
    pub fn insert(&mut self, rb: impl Into<RigidBody>) -> RigidBodyHandle {
        let mut rb = rb.into();
        // Make sure the internal links are reset, they may not be
        // if this rigid-body was obtained by cloning another one.
        rb.reset_internal_references();
        rb.changes.set(RigidBodyChanges::all(), true);

        let handle = RigidBodyHandle(self.bodies.insert(rb));
        // Using push_unchecked because this is a brand new rigid-body with the MODIFIED
        // flags set but isn’t in the modified_bodies yet.
        self.modified_bodies
            .push_unchecked(handle, &mut self.bodies[handle.0]);
        handle
    }

    /// Removes a rigid body from the world along with all its attached colliders and joints.
    ///
    /// This is a complete cleanup operation that removes:
    /// - The rigid body itself
    /// - All colliders attached to it (if `remove_attached_colliders` is `true`)
    /// - All joints connected to this body
    ///
    /// Returns the removed body if it existed, or `None` if the handle was invalid.
    ///
    /// # Parameters
    ///
    /// * `remove_attached_colliders` - If `true`, removes all colliders attached to this body.
    ///   If `false`, the colliders are detached and become independent.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut islands = IslandManager::new();
    /// # let mut colliders = ColliderSet::new();
    /// # let mut impulse_joints = ImpulseJointSet::new();
    /// # let mut multibody_joints = MultibodyJointSet::new();
    /// # let handle = bodies.insert(RigidBodyBuilder::dynamic());
    /// // Remove a body and everything attached to it
    /// if let Some(body) = bodies.remove(
    ///     handle,
    ///     &mut islands,
    ///     &mut colliders,
    ///     &mut impulse_joints,
    ///     &mut multibody_joints,
    ///     true  // Remove colliders too
    /// ) {
    ///     println!("Removed body at {:?}", body.translation());
    /// }
    /// ```
    pub fn remove(
        &mut self,
        handle: RigidBodyHandle,
        islands: &mut IslandManager,
        colliders: &mut ColliderSet,
        impulse_joints: &mut ImpulseJointSet,
        multibody_joints: &mut MultibodyJointSet,
        remove_attached_colliders: bool,
    ) -> Option<RigidBody> {
        let rb = self.bodies.remove(handle.0)?;
        /*
         * Update active sets.
         */
        islands.rigid_body_removed_or_disabled(handle, &rb.ids, self);

        /*
         * Remove colliders attached to this rigid-body.
         */
        if remove_attached_colliders {
            for collider in rb.colliders() {
                colliders.remove(*collider, islands, self, false);
            }
        } else {
            // If we don’t remove the attached colliders, simply detach them.
            let colliders_to_detach = rb.colliders().to_vec();
            for co_handle in colliders_to_detach {
                colliders.set_parent(co_handle, None, self);
            }
        }

        /*
         * Remove impulse_joints attached to this rigid-body.
         */
        impulse_joints.remove_joints_attached_to_rigid_body(handle);
        multibody_joints.remove_joints_attached_to_rigid_body(handle);

        Some(rb)
    }

    /// Gets a rigid body by its index without knowing the generation number.
    ///
    /// ⚠️ **Advanced/unsafe usage** - prefer [`get()`](Self::get) instead!
    ///
    /// This bypasses the generation check that normally protects against the ABA problem
    /// (where an index gets reused after removal). Only use this if you're certain the
    /// body at this index is the one you expect.
    ///
    /// Returns both the body and its current handle (with the correct generation).
    pub fn get_unknown_gen(&self, i: u32) -> Option<(&RigidBody, RigidBodyHandle)> {
        self.bodies
            .get_unknown_gen(i)
            .map(|(b, h)| (b, RigidBodyHandle(h)))
    }

    /// Gets a mutable reference to a rigid body by its index without knowing the generation.
    ///
    /// ⚠️ **Advanced/unsafe usage** - prefer [`get_mut()`](Self::get_mut) instead!
    ///
    /// This bypasses the generation check. See [`get_unknown_gen()`](Self::get_unknown_gen)
    /// for more details on when this is appropriate (rarely).
    #[cfg(not(feature = "dev-remove-slow-accessors"))]
    pub fn get_unknown_gen_mut(&mut self, i: u32) -> Option<(&mut RigidBody, RigidBodyHandle)> {
        let (rb, handle) = self.bodies.get_unknown_gen_mut(i)?;
        let handle = RigidBodyHandle(handle);
        self.modified_bodies.push_once(handle, rb);
        Some((rb, handle))
    }

    /// Gets a read-only reference to the rigid body with the given handle.
    ///
    /// Returns `None` if the handle is invalid or the body was removed.
    ///
    /// Use this to read body properties like position, velocity, mass, etc.
    pub fn get(&self, handle: RigidBodyHandle) -> Option<&RigidBody> {
        self.bodies.get(handle.0)
    }

    /// Gets a mutable reference to the rigid body with the given handle.
    ///
    /// Returns `None` if the handle is invalid or the body was removed.
    ///
    /// Use this to modify body properties, apply forces/impulses, change velocities, etc.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let handle = bodies.insert(RigidBodyBuilder::dynamic());
    /// if let Some(body) = bodies.get_mut(handle) {
    ///     body.set_linvel(Vector::new(1.0, 0.0, 0.0), true);
    ///     body.apply_impulse(Vector::new(0.0, 100.0, 0.0), true);
    /// }
    /// ```
    #[cfg(not(feature = "dev-remove-slow-accessors"))]
    pub fn get_mut(&mut self, handle: RigidBodyHandle) -> Option<&mut RigidBody> {
        let result = self.bodies.get_mut(handle.0)?;
        self.modified_bodies.push_once(handle, result);
        Some(result)
    }

    /// Gets mutable references to two different rigid bodies at once.
    ///
    /// This is useful when you need to modify two bodies simultaneously (e.g., when manually
    /// handling collisions between them). If both handles are the same, only the first value
    /// will be `Some`.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let handle1 = bodies.insert(RigidBodyBuilder::dynamic());
    /// # let handle2 = bodies.insert(RigidBodyBuilder::dynamic());
    /// let (body1, body2) = bodies.get_pair_mut(handle1, handle2);
    /// if let (Some(b1), Some(b2)) = (body1, body2) {
    ///     // Can modify both bodies at once
    ///     b1.apply_impulse(Vector::new(10.0, 0.0, 0.0), true);
    ///     b2.apply_impulse(Vector::new(-10.0, 0.0, 0.0), true);
    /// }
    /// ```
    #[cfg(not(feature = "dev-remove-slow-accessors"))]
    pub fn get_pair_mut(
        &mut self,
        handle1: RigidBodyHandle,
        handle2: RigidBodyHandle,
    ) -> (Option<&mut RigidBody>, Option<&mut RigidBody>) {
        if handle1 == handle2 {
            (self.get_mut(handle1), None)
        } else {
            let (mut rb1, mut rb2) = self.bodies.get2_mut(handle1.0, handle2.0);
            if let Some(rb1) = rb1.as_deref_mut() {
                self.modified_bodies.push_once(handle1, rb1);
            }
            if let Some(rb2) = rb2.as_deref_mut() {
                self.modified_bodies.push_once(handle2, rb2);
            }
            (rb1, rb2)
        }
    }

    pub(crate) fn get_mut_internal(&mut self, handle: RigidBodyHandle) -> Option<&mut RigidBody> {
        self.bodies.get_mut(handle.0)
    }

    pub(crate) fn index_mut_internal(&mut self, handle: RigidBodyHandle) -> &mut RigidBody {
        &mut self.bodies[handle.0]
    }

    // Just a very long name instead of `.get_mut` to make sure
    // this is really the method we wanted to use instead of `get_mut_internal`.
    pub(crate) fn get_mut_internal_with_modification_tracking(
        &mut self,
        handle: RigidBodyHandle,
    ) -> Option<&mut RigidBody> {
        let result = self.bodies.get_mut(handle.0)?;
        self.modified_bodies.push_once(handle, result);
        Some(result)
    }

    /// Iterates over all rigid bodies in this collection.
    ///
    /// Each iteration yields a `(handle, &RigidBody)` pair. Use this to read properties
    /// of all bodies (positions, velocities, etc.) without modifying them.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # bodies.insert(RigidBodyBuilder::dynamic());
    /// for (handle, body) in bodies.iter() {
    ///     println!("Body {:?} is at {:?}", handle, body.translation());
    /// }
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (RigidBodyHandle, &RigidBody)> {
        self.bodies.iter().map(|(h, b)| (RigidBodyHandle(h), b))
    }

    /// Iterates over all rigid bodies with mutable access.
    ///
    /// Each iteration yields a `(handle, &mut RigidBody)` pair. Use this to modify
    /// multiple bodies in one pass.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # bodies.insert(RigidBodyBuilder::dynamic());
    /// // Apply gravity manually to all dynamic bodies
    /// for (handle, body) in bodies.iter_mut() {
    ///     if body.is_dynamic() {
    ///         body.add_force(Vector::new(0.0, -9.81 * body.mass(), 0.0), true);
    ///     }
    /// }
    /// ```
    #[cfg(not(feature = "dev-remove-slow-accessors"))]
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (RigidBodyHandle, &mut RigidBody)> {
        self.modified_bodies.clear();
        let modified_bodies = &mut self.modified_bodies;
        self.bodies.iter_mut().map(move |(h, b)| {
            // NOTE: using `push_unchecked` because we just cleared `modified_bodies`
            //       before iterating.
            modified_bodies.push_unchecked(RigidBodyHandle(h), b);
            (RigidBodyHandle(h), b)
        })
    }

    /// Updates the positions of all colliders attached to bodies that have moved.
    ///
    /// Normally you don't need to call this - it's automatically handled by [`PhysicsPipeline::step`].
    /// Only call this manually if you're:
    /// - Moving bodies yourself outside of `step()`
    /// - Using `QueryPipeline` for raycasts without running physics simulation
    /// - Need collider positions to be immediately up-to-date for some custom logic
    ///
    /// This synchronizes collider world positions based on their parent bodies' positions.
    pub fn propagate_modified_body_positions_to_colliders(&self, colliders: &mut ColliderSet) {
        for body in self.modified_bodies.iter().filter_map(|h| self.get(*h)) {
            if body.changes.contains(RigidBodyChanges::POSITION) {
                for handle in body.colliders() {
                    if let Some(collider) = colliders.get_mut(*handle) {
                        let new_pos = body.position() * collider.position_wrt_parent().unwrap();
                        collider.set_position(new_pos);
                    }
                }
            }
        }
    }
}

impl Index<RigidBodyHandle> for RigidBodySet {
    type Output = RigidBody;

    fn index(&self, index: RigidBodyHandle) -> &RigidBody {
        &self.bodies[index.0]
    }
}

impl Index<crate::data::Index> for RigidBodySet {
    type Output = RigidBody;

    fn index(&self, index: crate::data::Index) -> &RigidBody {
        &self.bodies[index]
    }
}

#[cfg(not(feature = "dev-remove-slow-accessors"))]
impl IndexMut<RigidBodyHandle> for RigidBodySet {
    fn index_mut(&mut self, handle: RigidBodyHandle) -> &mut RigidBody {
        let rb = &mut self.bodies[handle.0];
        self.modified_bodies.push_once(handle, rb);
        rb
    }
}
