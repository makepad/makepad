use crate::dynamics::RigidBodyHandle;
use crate::geometry::{Aabb, Collider, ColliderHandle, PointProjection, Ray, RayIntersection};
use crate::geometry::{BroadPhaseBvh, InteractionGroups};
use crate::math::{Pose, Real, Vector};
use crate::{dynamics::RigidBodySet, geometry::ColliderSet};
use parry::bounding_volume::BoundingVolume;
use parry::partitioning::{Bvh, BvhNode};
use parry::query::details::{NormalConstraints, ShapeCastOptions};
use parry::query::{NonlinearRigidMotion, QueryDispatcher, RayCast, ShapeCastHit};
use parry::shape::{CompositeShape, CompositeShapeRef, FeatureId, Shape, TypedCompositeShape};

/// A query system for performing spatial queries on your physics world (raycasts, shape casts, intersections).
///
/// Think of this as a "search engine" for your physics world. Use it to answer questions like:
/// - "What does this ray hit?"
/// - "What colliders are near this point?"
/// - "If I move this shape, what will it collide with?"
///
/// Get a QueryPipeline from your [`BroadPhaseBvh`] using [`as_query_pipeline()`](BroadPhaseBvh::as_query_pipeline).
///
/// # Example
/// ```
/// # use rapier3d::prelude::*;
/// # let mut bodies = RigidBodySet::new();
/// # let mut colliders = ColliderSet::new();
/// # let broad_phase = BroadPhaseBvh::new();
/// # let narrow_phase = NarrowPhase::new();
/// # let ground = bodies.insert(RigidBodyBuilder::fixed());
/// # colliders.insert_with_parent(ColliderBuilder::cuboid(10.0, 0.1, 10.0), ground, &mut bodies);
/// let query_pipeline = broad_phase.as_query_pipeline(
///     narrow_phase.query_dispatcher(),
///     &bodies,
///     &colliders,
///     QueryFilter::default()
/// );
///
/// // Cast a ray downward
/// let ray = Ray::new(Vector::new(0.0, 10.0, 0.0), Vector::new(0.0, -1.0, 0.0));
/// if let Some((handle, toi)) = query_pipeline.cast_ray(&ray, Real::MAX, false) {
///     println!("Hit collider {:?} at distance {}", handle, toi);
/// }
/// ```
#[derive(Copy, Clone)]
pub struct QueryPipeline<'a> {
    /// The query dispatcher for running geometric queries on leaf geometries.
    pub dispatcher: &'a dyn QueryDispatcher,
    /// A bvh containing collider indices at its leaves.
    pub bvh: &'a Bvh,
    /// Rigid-bodies potentially involved in the scene queries.
    pub bodies: &'a RigidBodySet,
    /// Colliders potentially involved in the scene queries.
    pub colliders: &'a ColliderSet,
    /// The query filters for controlling what colliders should be ignored by the queries.
    pub filter: QueryFilter<'a>,
}

/// Same as [`QueryPipeline`] but holds mutable references to the body and collider sets.
///
/// This structure is generally obtained by calling [`BroadPhaseBvh::as_query_pipeline_mut`].
/// This is useful for argument passing. Call `.as_ref()` for obtaining a `QueryPipeline`
/// to run the scene queries.
pub struct QueryPipelineMut<'a> {
    /// The query dispatcher for running geometric queries on leaf geometries.
    pub dispatcher: &'a dyn QueryDispatcher,
    /// A bvh containing collider indices at its leaves.
    pub bvh: &'a Bvh,
    /// Rigid-bodies potentially involved in the scene queries.
    pub bodies: &'a mut RigidBodySet,
    /// Colliders potentially involved in the scene queries.
    pub colliders: &'a mut ColliderSet,
    /// The query filters for controlling what colliders should be ignored by the queries.
    pub filter: QueryFilter<'a>,
}

impl QueryPipelineMut<'_> {
    /// Downgrades the mutable reference to an immutable reference.
    pub fn as_ref(&self) -> QueryPipeline<'_> {
        QueryPipeline {
            dispatcher: self.dispatcher,
            bvh: self.bvh,
            bodies: &*self.bodies,
            colliders: &*self.colliders,
            filter: self.filter,
        }
    }
}

impl CompositeShape for QueryPipeline<'_> {
    fn map_part_at(
        &self,
        shape_id: u32,
        f: &mut dyn FnMut(Option<&Pose>, &dyn Shape, Option<&dyn NormalConstraints>),
    ) {
        self.map_untyped_part_at(shape_id, f);
    }
    fn bvh(&self) -> &Bvh {
        self.bvh
    }
}

impl TypedCompositeShape for QueryPipeline<'_> {
    type PartNormalConstraints = ();
    type PartShape = dyn Shape;
    fn map_typed_part_at<T>(
        &self,
        shape_id: u32,
        mut f: impl FnMut(Option<&Pose>, &Self::PartShape, Option<&Self::PartNormalConstraints>) -> T,
    ) -> Option<T> {
        let (co, co_handle) = self.colliders.get_unknown_gen(shape_id)?;

        if self.filter.test(self.bodies, co_handle, co) {
            // SAFETY: colliders store owned `'static` shapes.
            let shape: &(dyn Shape + 'static) = unsafe { core::mem::transmute(co.shape()) };
            Some(f(Some(co.position()), shape, None))
        } else {
            None
        }
    }

    fn map_untyped_part_at<T>(
        &self,
        shape_id: u32,
        mut f: impl FnMut(Option<&Pose>, &dyn Shape, Option<&dyn NormalConstraints>) -> T,
    ) -> Option<T> {
        let (co, co_handle) = self.colliders.get_unknown_gen(shape_id)?;

        if self.filter.test(self.bodies, co_handle, co) {
            Some(f(Some(co.position()), co.shape(), None))
        } else {
            None
        }
    }
}

impl BroadPhaseBvh {
    /// Initialize a [`QueryPipeline`] for scene queries from this broad-phase.
    pub fn as_query_pipeline<'a>(
        &'a self,
        dispatcher: &'a dyn QueryDispatcher,
        bodies: &'a RigidBodySet,
        colliders: &'a ColliderSet,
        filter: QueryFilter<'a>,
    ) -> QueryPipeline<'a> {
        QueryPipeline {
            dispatcher,
            bvh: &self.tree,
            bodies,
            colliders,
            filter,
        }
    }

    /// Initialize a [`QueryPipelineMut`] for scene queries from this broad-phase.
    pub fn as_query_pipeline_mut<'a>(
        &'a self,
        dispatcher: &'a dyn QueryDispatcher,
        bodies: &'a mut RigidBodySet,
        colliders: &'a mut ColliderSet,
        filter: QueryFilter<'a>,
    ) -> QueryPipelineMut<'a> {
        QueryPipelineMut {
            dispatcher,
            bvh: &self.tree,
            bodies,
            colliders,
            filter,
        }
    }
}

impl<'a> QueryPipeline<'a> {
    fn id_to_handle<T>(&self, (id, data): (u32, T)) -> Option<(ColliderHandle, T)> {
        self.colliders.get_unknown_gen(id).map(|(_, h)| (h, data))
    }

    /// Replaces [`Self::filter`] with different filtering rules.
    pub fn with_filter(self, filter: QueryFilter<'a>) -> Self {
        Self { filter, ..self }
    }

    /// Casts a ray through the world and returns the first collider it hits.
    ///
    /// This is one of the most common operations - use it for line-of-sight checks,
    /// projectile trajectories, mouse picking, laser beams, etc.
    ///
    /// Returns `Some((handle, distance))` if the ray hits something, where:
    /// - `handle` is which collider was hit
    /// - `distance` is how far along the ray the hit occurred (time-of-impact)
    ///
    /// # Parameters
    /// * `ray` - The ray to cast (origin + direction). Create with `Ray::new(origin, direction)`
    /// * `max_toi` - Maximum distance to check. Use `Real::MAX` for unlimited range
    /// * `solid` - If `true`, detects hits even if the ray starts inside a shape. If `false`,
    ///   the ray "passes through" from the inside until it exits
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut colliders = ColliderSet::new();
    /// # let broad_phase = BroadPhaseBvh::new();
    /// # let narrow_phase = NarrowPhase::new();
    /// # let ground = bodies.insert(RigidBodyBuilder::fixed());
    /// # colliders.insert_with_parent(ColliderBuilder::cuboid(10.0, 0.1, 10.0), ground, &mut bodies);
    /// # let query_pipeline = broad_phase.as_query_pipeline(narrow_phase.query_dispatcher(), &bodies, &colliders, QueryFilter::default());
    /// // Raycast downward from (0, 10, 0)
    /// let ray = Ray::new(Vector::new(0.0, 10.0, 0.0), Vector::new(0.0, -1.0, 0.0));
    /// if let Some((handle, toi)) = query_pipeline.cast_ray(&ray, Real::MAX, true) {
    ///     let hit_point = ray.origin + ray.dir * toi;
    ///     println!("Hit at {:?}, distance = {}", hit_point, toi);
    /// }
    /// ```
    pub fn cast_ray(
        &self,
        ray: &Ray,
        max_toi: Real,
        solid: bool,
    ) -> Option<(ColliderHandle, Real)> {
        CompositeShapeRef(self)
            .cast_local_ray(ray, max_toi, solid)
            .and_then(|hit| self.id_to_handle(hit))
    }

    /// Casts a ray and returns detailed information about the hit (including surface normal).
    ///
    /// Like [`cast_ray()`](Self::cast_ray), but returns more information useful for things like:
    /// - Decals (need surface normal to orient the texture)
    /// - Bullet holes (need to know what part of the mesh was hit)
    /// - Ricochets (need normal to calculate bounce direction)
    ///
    /// Returns `Some((handle, intersection))` where `intersection` contains:
    /// - `toi`: Distance to impact
    /// - `normal`: Surface normal at the hit point
    /// - `feature`: Which geometric feature was hit (vertex, edge, face)
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut colliders = ColliderSet::new();
    /// # let broad_phase = BroadPhaseBvh::new();
    /// # let narrow_phase = NarrowPhase::new();
    /// # let ground = bodies.insert(RigidBodyBuilder::fixed());
    /// # colliders.insert_with_parent(ColliderBuilder::cuboid(10.0, 0.1, 10.0), ground, &mut bodies);
    /// # let query_pipeline = broad_phase.as_query_pipeline(narrow_phase.query_dispatcher(), &bodies, &colliders, QueryFilter::default());
    /// # let ray = Ray::new(Vector::new(0.0, 10.0, 0.0), Vector::new(0.0, -1.0, 0.0));
    /// if let Some((handle, hit)) = query_pipeline.cast_ray_and_get_normal(&ray, 100.0, true) {
    ///     println!("Hit at distance {}, surface normal: {:?}", hit.time_of_impact, hit.normal);
    /// }
    /// ```
    pub fn cast_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_toi: Real,
        solid: bool,
    ) -> Option<(ColliderHandle, RayIntersection)> {
        CompositeShapeRef(self)
            .cast_local_ray_and_get_normal(ray, max_toi, solid)
            .and_then(|hit| self.id_to_handle(hit))
    }

    /// Returns ALL colliders that a ray passes through (not just the first).
    ///
    /// Unlike [`cast_ray()`](Self::cast_ray) which stops at the first hit, this returns
    /// every collider along the ray's path. Useful for:
    /// - Penetrating weapons that go through multiple objects
    /// - Checking what's in a line (e.g., visibility through glass)
    /// - Counting how many objects are between two points
    ///
    /// Returns an iterator of `(handle, collider, intersection)` tuples.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut colliders = ColliderSet::new();
    /// # let broad_phase = BroadPhaseBvh::new();
    /// # let narrow_phase = NarrowPhase::new();
    /// # let ground = bodies.insert(RigidBodyBuilder::fixed());
    /// # colliders.insert_with_parent(ColliderBuilder::cuboid(10.0, 0.1, 10.0), ground, &mut bodies);
    /// # let query_pipeline = broad_phase.as_query_pipeline(narrow_phase.query_dispatcher(), &bodies, &colliders, QueryFilter::default());
    /// # let ray = Ray::new(Vector::new(0.0, 10.0, 0.0), Vector::new(0.0, -1.0, 0.0));
    /// for (handle, collider, hit) in query_pipeline.intersect_ray(ray, 100.0, true) {
    ///     println!("Ray passed through {:?} at distance {}", handle, hit.time_of_impact);
    /// }
    /// ```
    pub fn intersect_ray(
        &'a self,
        ray: Ray,
        max_toi: Real,
        solid: bool,
    ) -> impl Iterator<Item = (ColliderHandle, &'a Collider, RayIntersection)> + 'a {
        // TODO: add this to CompositeShapeRef?
        self.bvh
            .leaves(move |node: &BvhNode| node.aabb().intersects_local_ray(&ray, max_toi))
            .filter_map(move |leaf| {
                let (co, co_handle) = self.colliders.get_unknown_gen(leaf)?;
                if self.filter.test(self.bodies, co_handle, co) {
                    if let Some(intersection) =
                        co.shape
                            .cast_ray_and_get_normal(co.position(), &ray, max_toi, solid)
                    {
                        return Some((co_handle, co, intersection));
                    }
                }

                None
            })
    }

    /// Finds the closest point on any collider to the given point.
    ///
    /// Returns the collider and information about where on its surface the closest point is.
    /// Useful for:
    /// - Finding nearest cover/obstacle
    /// - Snap-to-surface mechanics
    /// - Distance queries
    ///
    /// # Parameters
    /// * `solid` - If `true`, a point inside a shape projects to itself. If `false`, it projects
    ///   to the nearest point on the shape's boundary
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let params = IntegrationParameters::default();
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut colliders = ColliderSet::new();
    /// # let mut broad_phase = BroadPhaseBvh::new();
    /// # let narrow_phase = NarrowPhase::new();
    /// # let ground = bodies.insert(RigidBodyBuilder::fixed());
    /// # let ground_collider = ColliderBuilder::cuboid(10.0, 0.1, 10.0).build();
    /// # let ground_aabb = ground_collider.compute_aabb();
    /// # let collider_handle = colliders.insert_with_parent(ground_collider, ground, &mut bodies);
    /// # broad_phase.set_aabb(&params, collider_handle, ground_aabb);
    /// # let query_pipeline = broad_phase.as_query_pipeline(narrow_phase.query_dispatcher(), &bodies, &colliders, QueryFilter::default());
    /// let point = Vector::new(5.0, 0.0, 0.0);
    /// if let Some((handle, projection)) = query_pipeline.project_point(point, std::f32::MAX, true) {
    ///     println!("Closest collider: {:?}", handle);
    ///     println!("Closest point: {:?}", projection.point);
    ///     println!("Distance: {}", (point - projection.point).length());
    /// }
    /// ```
    pub fn project_point(
        &self,
        point: Vector,
        _max_dist: Real,
        solid: bool,
    ) -> Option<(ColliderHandle, PointProjection)> {
        self.id_to_handle(CompositeShapeRef(self).project_local_point(point, solid))
    }

    /// Returns ALL colliders that contain the given point.
    ///
    /// A point is "inside" a collider if it's within its volume. Useful for:
    /// - Detecting what area/trigger zones a point is in
    /// - Checking if a position is inside geometry
    /// - Finding all overlapping volumes at a location
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut colliders = ColliderSet::new();
    /// # let broad_phase = BroadPhaseBvh::new();
    /// # let narrow_phase = NarrowPhase::new();
    /// # let ground = bodies.insert(RigidBodyBuilder::fixed());
    /// # colliders.insert_with_parent(ColliderBuilder::ball(5.0), ground, &mut bodies);
    /// # let query_pipeline = broad_phase.as_query_pipeline(narrow_phase.query_dispatcher(), &bodies, &colliders, QueryFilter::default());
    /// let point = Vector::new(0.0, 0.0, 0.0);
    /// for (handle, collider) in query_pipeline.intersect_point(point) {
    ///     println!("Point is inside {:?}", handle);
    /// }
    /// ```
    pub fn intersect_point(
        &'a self,
        point: Vector,
    ) -> impl Iterator<Item = (ColliderHandle, &'a Collider)> + 'a {
        // TODO: add to CompositeShapeRef?
        self.bvh
            .leaves(move |node: &BvhNode| node.aabb().contains_local_point(point))
            .filter_map(move |leaf| {
                let (co, co_handle) = self.colliders.get_unknown_gen(leaf)?;
                if self.filter.test(self.bodies, co_handle, co)
                    && co.shape.contains_point(co.position(), point)
                {
                    return Some((co_handle, co));
                }

                None
            })
    }

    /// Find the projection of a point on the closest collider.
    ///
    /// The results include the ID of the feature hit by the point.
    ///
    /// # Parameters
    /// * `point` - The point to project.
    pub fn project_point_and_get_feature(
        &self,
        point: Vector,
    ) -> Option<(ColliderHandle, PointProjection, FeatureId)> {
        let (id, (proj, feat)) = CompositeShapeRef(self).project_local_point_and_get_feature(point);
        let handle = self.colliders.get_unknown_gen(id)?.1;
        Some((handle, proj, feat))
    }

    /// Finds all handles of all the colliders with an [`Aabb`] intersecting the given [`Aabb`].
    ///
    /// Note that the collider AABB taken into account is the one currently stored in the query
    /// pipeline’s BVH. It doesn’t recompute the latest collider AABB.
    pub fn intersect_aabb_conservative(
        &'a self,
        aabb: Aabb,
    ) -> impl Iterator<Item = (ColliderHandle, &'a Collider)> + 'a {
        // TODO: add to ColliderRef?
        self.bvh
            .leaves(move |node: &BvhNode| node.aabb().intersects(&aabb))
            .filter_map(move |leaf| {
                let (co, co_handle) = self.colliders.get_unknown_gen(leaf)?;
                // NOTE: do **not** recompute and check the latest collider AABB.
                //       Checking only against the one in the BVH is useful, e.g., for conservative
                //       scene queries for CCD.
                if self.filter.test(self.bodies, co_handle, co) {
                    return Some((co_handle, co));
                }

                None
            })
    }

    /// Sweeps a shape through the world to find what it would collide with.
    ///
    /// Like raycasting, but instead of a thin ray, you're moving an entire shape (sphere, box, etc.)
    /// through space. This is also called "shape casting" or "sweep testing". Useful for:
    /// - Predicting where a moving object will hit something
    /// - Checking if a movement is valid before executing it
    /// - Thick raycasts (e.g., character controller collision prediction)
    /// - Area-of-effect scanning along a path
    ///
    /// Returns the first collision: `(collider_handle, hit_details)` where hit contains
    /// time-of-impact, witness points, and surface normal.
    ///
    /// # Parameters
    /// * `shape_pos` - Starting position/orientation of the shape
    /// * `shape_vel` - Direction and speed to move the shape (velocity vector)
    /// * `shape` - The shape to sweep (ball, cuboid, capsule, etc.)
    /// * `options` - Maximum distance, collision filtering, etc.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// # use rapier3d::parry::{query::ShapeCastOptions, shape::Ball};
    /// # let mut bodies = RigidBodySet::new();
    /// # let mut colliders = ColliderSet::new();
    /// # let narrow_phase = NarrowPhase::new();
    /// # let broad_phase = BroadPhaseBvh::new();
    /// # let ground = bodies.insert(RigidBodyBuilder::fixed());
    /// # colliders.insert_with_parent(ColliderBuilder::cuboid(10.0, 0.1, 10.0), ground, &mut bodies);
    /// # let query_pipeline = broad_phase.as_query_pipeline(narrow_phase.query_dispatcher(), &bodies, &colliders, QueryFilter::default());
    /// // Sweep a sphere downward
    /// let shape = Ball::new(0.5);
    /// let start_pos = Pose::translation(0.0, 10.0, 0.0);
    /// let velocity = Vector::new(0.0, -1.0, 0.0);
    /// let options = ShapeCastOptions::default();
    ///
    /// if let Some((handle, hit)) = query_pipeline.cast_shape(&start_pos, velocity, &shape, options) {
    ///     println!("Shape would hit {:?} at time {}", handle, hit.time_of_impact);
    /// }
    /// ```
    pub fn cast_shape(
        &self,
        shape_pos: &Pose,
        shape_vel: Vector,
        shape: &dyn Shape,
        options: ShapeCastOptions,
    ) -> Option<(ColliderHandle, ShapeCastHit)> {
        CompositeShapeRef(self)
            .cast_shape(self.dispatcher, shape_pos, shape_vel, shape, options)
            .and_then(|hit| self.id_to_handle(hit))
    }

    /// Casts a shape with an arbitrary continuous motion and retrieve the first collider it hits.
    ///
    /// In the resulting `TOI`, witness and normal 1 refer to the world collider, and are in world
    /// space.
    ///
    /// # Parameters
    /// * `shape_motion` - The motion of the shape.
    /// * `shape` - The shape to cast.
    /// * `start_time` - The starting time of the interval where the motion takes place.
    /// * `end_time` - The end time of the interval where the motion takes place.
    /// * `stop_at_penetration` - If the casted shape starts in a penetration state with any
    ///    collider, two results are possible. If `stop_at_penetration` is `true` then, the
    ///    result will have a `toi` equal to `start_time`. If `stop_at_penetration` is `false`
    ///    then the nonlinear shape-casting will see if further motion with respect to the penetration normal
    ///    would result in tunnelling. If it does not (i.e. we have a separating velocity along
    ///    that normal) then the nonlinear shape-casting will attempt to find another impact,
    ///    at a time `> start_time` that could result in tunnelling.
    pub fn cast_shape_nonlinear(
        &self,
        shape_motion: &NonlinearRigidMotion,
        shape: &dyn Shape,
        start_time: Real,
        end_time: Real,
        stop_at_penetration: bool,
    ) -> Option<(ColliderHandle, ShapeCastHit)> {
        CompositeShapeRef(self)
            .cast_shape_nonlinear(
                self.dispatcher,
                &NonlinearRigidMotion::identity(),
                shape_motion,
                shape,
                start_time,
                end_time,
                stop_at_penetration,
            )
            .and_then(|hit| self.id_to_handle(hit))
    }

    /// Retrieve all the colliders intersecting the given shape.
    ///
    /// # Parameters
    /// * `shapePos` - The pose of the shape to test.
    /// * `shape` - The shape to test.
    pub fn intersect_shape(
        &'a self,
        shape_pos: Pose,
        shape: &'a dyn Shape,
    ) -> impl Iterator<Item = (ColliderHandle, &'a Collider)> + 'a {
        // TODO: add this to CompositeShapeRef?
        let shape_aabb = shape.compute_aabb(&shape_pos);
        self.bvh
            .leaves(move |node: &BvhNode| node.aabb().intersects(&shape_aabb))
            .filter_map(move |leaf| {
                let (co, co_handle) = self.colliders.get_unknown_gen(leaf)?;
                if self.filter.test(self.bodies, co_handle, co) {
                    let pos12 = shape_pos.inv_mul(co.position());
                    if self.dispatcher.intersection_test(&pos12, shape, co.shape()) == Ok(true) {
                        return Some((co_handle, co));
                    }
                }

                None
            })
    }
}

bitflags::bitflags! {
    #[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
    /// Flags for filtering spatial queries by body type or sensor status.
    ///
    /// Use these to quickly exclude categories of colliders from raycasts and other queries.
    ///
    /// # Example
    /// ```
    /// # use rapier3d::prelude::*;
    /// // Raycast that only hits dynamic objects (ignore walls/floors)
    /// let filter = QueryFilter::from(QueryFilterFlags::ONLY_DYNAMIC);
    ///
    /// // Find only trigger zones, not solid geometry
    /// let filter = QueryFilter::from(QueryFilterFlags::EXCLUDE_SOLIDS);
    /// ```
    pub struct QueryFilterFlags: u32 {
        /// Excludes fixed bodies and standalone colliders.
        const EXCLUDE_FIXED = 1 << 0;
        /// Excludes kinematic bodies.
        const EXCLUDE_KINEMATIC = 1 << 1;
        /// Excludes dynamic bodies.
        const EXCLUDE_DYNAMIC = 1 << 2;
        /// Excludes sensors (trigger zones).
        const EXCLUDE_SENSORS = 1 << 3;
        /// Excludes solid colliders (only hit sensors).
        const EXCLUDE_SOLIDS = 1 << 4;
        /// Only includes dynamic bodies.
        const ONLY_DYNAMIC = Self::EXCLUDE_FIXED.bits() | Self::EXCLUDE_KINEMATIC.bits();
        /// Only includes kinematic bodies.
        const ONLY_KINEMATIC = Self::EXCLUDE_DYNAMIC.bits() | Self::EXCLUDE_FIXED.bits();
        /// Only includes fixed bodies (excluding standalone colliders).
        const ONLY_FIXED = Self::EXCLUDE_DYNAMIC.bits() | Self::EXCLUDE_KINEMATIC.bits();
    }
}

impl QueryFilterFlags {
    /// Tests if the given collider should be taken into account by a scene query, based
    /// on the flags on `self`.
    #[inline]
    pub fn test(&self, bodies: &RigidBodySet, collider: &Collider) -> bool {
        if self.is_empty() {
            // No filter.
            return true;
        }

        if (self.contains(QueryFilterFlags::EXCLUDE_SENSORS) && collider.is_sensor())
            || (self.contains(QueryFilterFlags::EXCLUDE_SOLIDS) && !collider.is_sensor())
        {
            return false;
        }

        if self.contains(QueryFilterFlags::EXCLUDE_FIXED) && collider.parent.is_none() {
            return false;
        }

        if let Some(parent) = collider.parent.and_then(|p| bodies.get(p.handle)) {
            let parent_type = parent.body_type();

            if (self.contains(QueryFilterFlags::EXCLUDE_FIXED) && parent_type.is_fixed())
                || (self.contains(QueryFilterFlags::EXCLUDE_KINEMATIC)
                    && parent_type.is_kinematic())
                || (self.contains(QueryFilterFlags::EXCLUDE_DYNAMIC) && parent_type.is_dynamic())
            {
                return false;
            }
        }

        true
    }
}

/// Filtering rules for spatial queries (raycasts, shape casts, etc.).
///
/// Controls which colliders should be included/excluded from query results.
/// By default, all colliders are included.
///
/// # Common filters
///
/// ```
/// # use rapier3d::prelude::*;
/// # let player_collider = ColliderHandle::from_raw_parts(0, 0);
/// # let enemy_groups = InteractionGroups::all();
/// // Only hit dynamic objects (ignore static walls)
/// let filter = QueryFilter::only_dynamic();
///
/// // Hit everything except the player's own collider
/// let filter = QueryFilter::default()
///     .exclude_collider(player_collider);
///
/// // Raycast that only hits enemies (using collision groups)
/// let filter = QueryFilter::default()
///     .groups(enemy_groups);
///
/// // Custom filtering with a closure
/// let filter = QueryFilter::default()
///     .predicate(&|handle, collider| {
///         // Only hit colliders with user_data > 100
///         collider.user_data > 100
///     });
/// ```
#[derive(Copy, Clone, Default)]
pub struct QueryFilter<'a> {
    /// Flags for excluding fixed/kinematic/dynamic bodies or sensors/solids.
    pub flags: QueryFilterFlags,
    /// If set, only colliders with compatible collision groups are included.
    pub groups: Option<InteractionGroups>,
    /// If set, this specific collider is excluded.
    pub exclude_collider: Option<ColliderHandle>,
    /// If set, all colliders attached to this body are excluded.
    pub exclude_rigid_body: Option<RigidBodyHandle>,
    /// Custom filtering function - collider included only if this returns `true`.
    #[allow(clippy::type_complexity)]
    pub predicate: Option<&'a dyn Fn(ColliderHandle, &Collider) -> bool>,
}

impl QueryFilter<'_> {
    /// Applies the filters described by `self` to a collider to determine if it has to be
    /// included in a scene query (`true`) or not (`false`).
    #[inline]
    pub fn test(&self, bodies: &RigidBodySet, handle: ColliderHandle, collider: &Collider) -> bool {
        self.exclude_collider != Some(handle)
            && (self.exclude_rigid_body.is_none() // NOTE: deal with the `None` case separately otherwise the next test is incorrect if the collider’s parent is `None` too.
            || self.exclude_rigid_body != collider.parent.map(|p| p.handle))
            && self
                .groups
                .map(|grps| collider.flags.collision_groups.test(grps))
                .unwrap_or(true)
            && self.flags.test(bodies, collider)
            && self.predicate.map(|f| f(handle, collider)).unwrap_or(true)
    }
}

impl From<QueryFilterFlags> for QueryFilter<'_> {
    fn from(flags: QueryFilterFlags) -> Self {
        Self {
            flags,
            ..QueryFilter::default()
        }
    }
}

impl From<InteractionGroups> for QueryFilter<'_> {
    fn from(groups: InteractionGroups) -> Self {
        Self {
            groups: Some(groups),
            ..QueryFilter::default()
        }
    }
}

impl<'a> QueryFilter<'a> {
    /// A query filter that doesn’t exclude any collider.
    pub fn new() -> Self {
        Self::default()
    }

    /// Exclude from the query any collider attached to a fixed rigid-body and colliders with no rigid-body attached.
    pub fn exclude_fixed() -> Self {
        QueryFilterFlags::EXCLUDE_FIXED.into()
    }

    /// Exclude from the query any collider attached to a kinematic rigid-body.
    pub fn exclude_kinematic() -> Self {
        QueryFilterFlags::EXCLUDE_KINEMATIC.into()
    }

    /// Exclude from the query any collider attached to a dynamic rigid-body.
    pub fn exclude_dynamic() -> Self {
        QueryFilterFlags::EXCLUDE_DYNAMIC.into()
    }

    /// Excludes all colliders not attached to a dynamic rigid-body.
    pub fn only_dynamic() -> Self {
        QueryFilterFlags::ONLY_DYNAMIC.into()
    }

    /// Excludes all colliders not attached to a kinematic rigid-body.
    pub fn only_kinematic() -> Self {
        QueryFilterFlags::ONLY_KINEMATIC.into()
    }

    /// Exclude all colliders attached to a non-fixed rigid-body
    /// (this will not exclude colliders not attached to any rigid-body).
    pub fn only_fixed() -> Self {
        QueryFilterFlags::ONLY_FIXED.into()
    }

    /// Exclude from the query any collider that is a sensor.
    pub fn exclude_sensors(mut self) -> Self {
        self.flags |= QueryFilterFlags::EXCLUDE_SENSORS;
        self
    }

    /// Exclude from the query any collider that is not a sensor.
    pub fn exclude_solids(mut self) -> Self {
        self.flags |= QueryFilterFlags::EXCLUDE_SOLIDS;
        self
    }

    /// Only colliders with collision groups compatible with this one will
    /// be included in the scene query.
    pub fn groups(mut self, groups: InteractionGroups) -> Self {
        self.groups = Some(groups);
        self
    }

    /// Set the collider that will be excluded from the scene query.
    pub fn exclude_collider(mut self, collider: ColliderHandle) -> Self {
        self.exclude_collider = Some(collider);
        self
    }

    /// Set the rigid-body that will be excluded from the scene query.
    pub fn exclude_rigid_body(mut self, rigid_body: RigidBodyHandle) -> Self {
        self.exclude_rigid_body = Some(rigid_body);
        self
    }

    /// Set the predicate to apply a custom collider filtering during the scene query.
    pub fn predicate(mut self, predicate: &'a impl Fn(ColliderHandle, &Collider) -> bool) -> Self {
        self.predicate = Some(predicate);
        self
    }
}
