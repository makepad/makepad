use super::{Bvh, BvhNode};
use crate::bounding_volume::{Aabb, BoundingVolume};
use crate::math::Real;
use crate::math::Vector;
use crate::query::PointProjection;
use crate::query::{PointQuery, Ray};
use crate::shape::FeatureId;

#[cfg(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32"))]
pub(super) struct SimdInvRay {
    // TODO: we need to use `glam` here instead of `wide` because `wide` is lacking
    //       operations for getting the min/max vector element.
    //       We can switch back to `wide` once it's supported.
    pub origin: glamx::Vec3A,
    pub inv_dir: glamx::Vec3A,
}

#[cfg(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32"))]
impl From<Ray> for SimdInvRay {
    fn from(ray: Ray) -> Self {
        let inv_dir = ray.dir.map(|r| {
            if r.abs() < Real::EPSILON {
                r.signum() / Real::EPSILON
            } else {
                1.0 / r
            }
        });
        Self {
            origin: glamx::Vec3A::from([ray.origin.x, ray.origin.y, ray.origin.z]),
            inv_dir: glamx::Vec3A::from([inv_dir.x, inv_dir.y, inv_dir.z]),
        }
    }
}

impl Bvh {
    /// Returns an iterator over all leaves whose AABBs intersect the given AABB.
    ///
    /// This efficiently finds all objects in the scene that potentially overlap with a
    /// query region. It's commonly used for:
    /// - Finding objects in a specific area
    /// - Broad-phase collision detection (find candidate pairs)
    /// - Spatial queries (what's near this position?)
    /// - Frustum culling (what's in the camera's view?)
    ///
    /// The iterator returns leaf indices corresponding to objects that were added to the BVH.
    ///
    /// # Arguments
    ///
    /// * `aabb` - The axis-aligned bounding box to test for intersections
    ///
    /// # Returns
    ///
    /// An iterator yielding `u32` leaf indices for all leaves whose AABBs intersect the
    /// query AABB. The iteration order is depth-first through the tree.
    ///
    /// # Performance
    ///
    /// - **Time**: O(log n) average case with good spatial locality
    /// - **Worst case**: O(n) if the query AABB is very large
    /// - Efficiently prunes entire subtrees that don't intersect
    /// - Zero allocations
    ///
    /// # Examples
    ///
    /// ## Find objects in a region
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// // Create a scene with objects
    /// let objects = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(5.0, 0.0, 0.0), Vector::new(6.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(10.0, 0.0, 0.0), Vector::new(11.0, 1.0, 1.0)),
    /// ];
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &objects);
    ///
    /// // Query: which objects are near the origin?
    /// let query_region = Aabb::new(
    ///     Vector::new(-1.0, -1.0, -1.0),
    ///     Vector::new(2.0, 2.0, 2.0)
    /// );
    ///
    /// for leaf_id in bvh.intersect_aabb(&query_region) {
    ///     println!("Object {} is in the query region", leaf_id);
    ///     // You can now test the actual geometry of objects[leaf_id]
    /// }
    /// # }
    /// ```
    ///
    /// ## Broad-phase collision detection
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let objects = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(0.5, 0.0, 0.0), Vector::new(1.5, 1.0, 1.0)),  // Overlaps first
    ///     Aabb::new(Vector::new(10.0, 0.0, 0.0), Vector::new(11.0, 1.0, 1.0)), // Far away
    /// ];
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &objects);
    ///
    /// // Find potential collision pairs
    /// let mut collision_pairs = Vec::new();
    ///
    /// for (i, aabb) in objects.iter().enumerate() {
    ///     for leaf_id in bvh.intersect_aabb(aabb) {
    ///         if leaf_id as usize > i {  // Avoid duplicate pairs
    ///             collision_pairs.push((i as u32, leaf_id));
    ///         }
    ///     }
    /// }
    ///
    /// // collision_pairs now contains potential collision pairs
    /// // Next step: narrow-phase test actual geometry
    /// # }
    /// ```
    ///
    /// ## Frustum culling for rendering
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let scene_objects = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(2.0, 0.0, 0.0), Vector::new(3.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(100.0, 0.0, 0.0), Vector::new(101.0, 1.0, 1.0)), // Far away
    /// ];
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &scene_objects);
    ///
    /// // Camera frustum as an AABB (simplified - real frustums are more complex)
    /// let camera_frustum = Aabb::new(
    ///     Vector::new(-10.0, -10.0, -10.0),
    ///     Vector::new(10.0, 10.0, 10.0)
    /// );
    ///
    /// let mut visible_objects = Vec::new();
    /// for leaf_id in bvh.intersect_aabb(&camera_frustum) {
    ///     visible_objects.push(leaf_id);
    /// }
    ///
    /// // Only render visible_objects - massive performance win!
    /// assert_eq!(visible_objects.len(), 2); // Far object culled
    /// # }
    /// ```
    ///
    /// ## Finding neighbors for an object
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::partitioning::{Bvh, BvhBuildStrategy};
    /// use parry3d::bounding_volume::{Aabb, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let objects = vec![
    ///     Aabb::new(Vector::ZERO, Vector::new(1.0, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(1.5, 0.0, 0.0), Vector::new(2.5, 1.0, 1.0)),
    ///     Aabb::new(Vector::new(100.0, 0.0, 0.0), Vector::new(101.0, 1.0, 1.0)),
    /// ];
    ///
    /// let bvh = Bvh::from_leaves(BvhBuildStrategy::default(), &objects);
    ///
    /// // Expand an object's AABB to find nearby objects
    /// let search_radius = 2.0;
    /// let expanded = objects[0].loosened(search_radius);
    ///
    /// let mut neighbors = Vec::new();
    /// for leaf_id in bvh.intersect_aabb(&expanded) {
    ///     if leaf_id != 0 {  // Don't include the object itself
    ///         neighbors.push(leaf_id);
    ///     }
    /// }
    ///
    /// // neighbors contains objects within search_radius of object 0
    /// # }
    /// ```
    ///
    /// # Notes
    ///
    /// - This only tests AABB-AABB intersection, which is conservative (may have false positives)
    /// - For exact collision detection, test actual geometry after getting candidates
    /// - The iterator is lazy - work is only done as you consume items
    /// - Empty queries (no intersections) are very fast due to early pruning
    ///
    /// # See Also
    ///
    /// - [`cast_ray`](Self::cast_ray) - Ray casting queries
    /// - [`project_point`](Self::project_point) - Find closest point
    /// - [`traverse`](Self::traverse) - Custom traversal logic
    /// - [`leaves`](Self::leaves) - General leaf iteration with predicate
    pub fn intersect_aabb<'a>(&'a self, aabb: &'a Aabb) -> impl Iterator<Item = u32> + 'a {
        self.leaves(|node: &BvhNode| node.aabb().intersects(aabb))
    }

    /// Projects a point on this BVH using the provided leaf projection function.
    ///
    /// The `primitive_check` delegates the point-projection task to an external function that
    /// is assumed to map a leaf index to an actual geometry to project on. The `Real` argument
    /// given to that closure is the distance to the closest point found so far (or is equal to
    /// `max_distance` if no projection was found so far).
    pub fn project_point(
        &self,
        point: Vector,
        max_distance: Real,
        primitive_check: impl Fn(u32, Real) -> Option<PointProjection>,
    ) -> Option<(u32, (Real, PointProjection))> {
        self.find_best(
            max_distance,
            |node: &BvhNode, _| node.aabb().distance_to_local_point(point, true),
            |primitive, _| {
                let proj = primitive_check(primitive, max_distance)?;
                Some((proj.point.distance(point), proj))
            },
        )
    }

    /// Projects a point on this BVH using the provided leaf projection function.
    ///
    /// Also returns the feature the point was projected on.
    ///
    /// The `primitive_check` delegates the point-projection task to an external function that
    /// is assumed to map a leaf index to an actual geometry to project on. The `Real` argument
    /// given to that closure is the distance to the closest point found so far (or is equal to
    /// `max_distance` if no projection was found so far).
    pub fn project_point_and_get_feature(
        &self,
        point: Vector,
        max_distance: Real,
        primitive_check: impl Fn(u32, Real) -> Option<(PointProjection, FeatureId)>,
    ) -> Option<(u32, (Real, (PointProjection, FeatureId)))> {
        self.find_best(
            max_distance,
            |node: &BvhNode, _| node.aabb().distance_to_local_point(point, true),
            |primitive, _| {
                let proj = primitive_check(primitive, max_distance)?;
                Some((proj.0.point.distance(point), proj))
            },
        )
    }

    /// Casts a ray on this BVH using the provided leaf ray-cast function.
    ///
    /// The `primitive_check` delegates the ray-casting task to an external function that
    /// is assumed to map a leaf index to an actual geometry to cast a ray on. The `Real` argument
    /// given to that closure is the distance to the closest ray hit found so far (or is equal to
    /// `max_time_of_impact` if no projection was found so far).
    #[cfg(not(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32")))]
    pub fn cast_ray(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        primitive_check: impl Fn(u32, Real) -> Option<Real>,
    ) -> Option<(u32, Real)> {
        self.find_best(
            max_time_of_impact,
            |node: &BvhNode, best_so_far| node.cast_ray(ray, best_so_far),
            primitive_check,
        )
    }

    /// Casts a ray on this BVH using the provided leaf ray-cast function.
    ///
    /// The `primitive_check` delegates the ray-casting task to an external function that
    /// is assumed to map a leaf index to an actual geometry to cast a ray on. The `Real` argument
    /// given to that closure is the distance to the closest ray hit found so far (or is equal to
    /// `max_time_of_impact` if no projection was found so far).
    #[cfg(all(feature = "simd-is-enabled", feature = "dim3", feature = "f32"))]
    pub fn cast_ray(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        primitive_check: impl Fn(u32, Real) -> Option<Real>,
    ) -> Option<(u32, Real)> {
        // The commented code below relies on depth-first traversal instead of
        // depth first. Interesting to compare both approaches (the best-first is
        // expected to be consistently faster though).
        //
        // let simd_inv_ray = SimdInvRay::from(*ray);
        // let mut best_primitive = u32::MAX;
        // let mut best_toi = Real::MAX;
        // self.traverse(|node: &BvhNode| {
        //     if node.cast_inv_ray_simd(&simd_inv_ray) >= best_toi {
        //         return TraversalAction::Prune;
        //     }
        //
        //     if let Some(primitive) = node.leaf_data() {
        //         let toi = primitive_check(primitive as u32, best_toi);
        //         if toi < best_toi {
        //             best_primitive = primitive as u32;
        //             best_toi = toi;
        //         }
        //     }
        //
        //     TraversalAction::Continue
        // });
        // (best_primitive, best_toi)

        let simd_inv_ray = SimdInvRay::from(*ray);
        self.find_best(
            max_time_of_impact,
            |node: &BvhNode, _best_so_far| node.cast_inv_ray_simd(&simd_inv_ray),
            |primitive, best_so_far| primitive_check(primitive, best_so_far),
        )
    }
}
