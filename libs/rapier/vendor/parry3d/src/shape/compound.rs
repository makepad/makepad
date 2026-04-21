//!
//! Shape composed from the union of primitives.
//!

use crate::bounding_volume::{Aabb, BoundingSphere, BoundingVolume};
use crate::math::Pose;
use crate::partitioning::{Bvh, BvhBuildStrategy};
use crate::query::details::NormalConstraints;
use crate::shape::{CompositeShape, Shape, SharedShape, TypedCompositeShape};
#[cfg(feature = "dim2")]
use crate::shape::{ConvexPolygon, TriMesh, Triangle};
#[cfg(feature = "dim2")]
use crate::transformation::hertel_mehlhorn;
use alloc::vec::Vec;

/// A compound shape with an aabb bounding volume.
///
/// A compound shape is a shape composed of the union of several simpler shape. This is
/// the main way of creating a concave shape from convex parts. Each parts can have its own
/// delta transformation to shift or rotate it with regard to the other shapes.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
pub struct Compound {
    shapes: Vec<(Pose, SharedShape)>,
    bvh: Bvh,
    aabbs: Vec<Aabb>,
    aabb: Aabb,
}

impl Compound {
    /// Builds a new compound shape from a collection of sub-shapes.
    ///
    /// A compound shape combines multiple primitive shapes into a single composite shape,
    /// each with its own relative position and orientation. This is the primary way to
    /// create concave shapes from convex primitives. The compound internally builds a
    /// Bounding Volume Hierarchy (BVH) for efficient collision detection.
    ///
    /// # Arguments
    ///
    /// * `shapes` - A vector of (position, shape) pairs. Each pair defines:
    ///   - An [`Pose`] representing the sub-shape's position and orientation relative to the compound's origin
    ///   - A [`SharedShape`] containing the actual geometry
    ///
    /// # Panics
    ///
    /// - If the input vector is empty (a compound must contain at least one shape)
    /// - If any of the provided shapes are themselves composite shapes (nested composites are not allowed)
    ///
    /// # Performance
    ///
    /// The BVH is built using a binned construction strategy optimized for static scenes.
    /// For large compounds (100+ shapes), construction may take noticeable time but provides
    /// excellent query performance.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{Compound, Ball, Cuboid, SharedShape};
    /// use parry3d::math::Pose;
    /// use parry3d::math::Vector;
    ///
    /// // Create a compound shape resembling a dumbbell
    /// let shapes = vec![
    ///     // Left sphere
    ///     (
    ///         Pose::translation(-2.0, 0.0, 0.0),
    ///         SharedShape::new(Ball::new(0.5))
    ///     ),
    ///     // Center bar
    ///     (
    ///         Pose::identity(),
    ///         SharedShape::new(Cuboid::new(Vector::new(2.0, 0.2, 0.2)))
    ///     ),
    ///     // Right sphere
    ///     (
    ///         Pose::translation(2.0, 0.0, 0.0),
    ///         SharedShape::new(Ball::new(0.5))
    ///     ),
    /// ];
    ///
    /// let compound = Compound::new(shapes);
    ///
    /// // The compound now contains all three shapes
    /// assert_eq!(compound.shapes().len(), 3);
    /// # }
    /// ```
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::shape::{Compound, Ball, Cuboid, SharedShape};
    /// # use parry2d::math::Pose;
    /// # use parry2d::math::Vector;
    ///
    /// // Create an L-shaped compound
    /// let shapes = vec![
    ///     // Vertical rectangle
    ///     (
    ///         Pose::translation(0.0, 1.0),
    ///         SharedShape::new(Cuboid::new(Vector::new(0.5, 1.0)))
    ///     ),
    ///     // Horizontal rectangle
    ///     (
    ///         Pose::translation(1.0, 0.0),
    ///         SharedShape::new(Cuboid::new(Vector::new(1.0, 0.5)))
    ///     ),
    /// ];
    ///
    /// let l_shape = Compound::new(shapes);
    /// assert_eq!(l_shape.shapes().len(), 2);
    /// # }
    /// ```
    pub fn new(shapes: Vec<(Pose, SharedShape)>) -> Compound {
        assert!(
            !shapes.is_empty(),
            "A compound shape must contain at least one shape."
        );
        let mut aabbs = Vec::new();
        let mut leaves = Vec::new();
        let mut aabb = Aabb::new_invalid();

        for (i, (delta, shape)) in shapes.iter().enumerate() {
            let bv = shape.compute_aabb(delta);

            aabb.merge(&bv);
            aabbs.push(bv);
            leaves.push((i, bv));

            if shape.as_composite_shape().is_some() {
                panic!("Nested composite shapes are not allowed.");
            }
        }

        // NOTE: we apply no dilation factor because we won't
        // update this tree dynamically.
        let bvh = Bvh::from_iter(BvhBuildStrategy::Binned, leaves);

        Compound {
            shapes,
            bvh,
            aabbs,
            aabb,
        }
    }

    /// Creates a compound shape by decomposing a triangle mesh into convex polygons.
    ///
    /// This 2D-only method takes a [`TriMesh`] and merges adjacent triangles into larger
    /// convex polygons using the Hertel-Mehlhorn algorithm. This is useful for creating
    /// efficient collision shapes from arbitrary 2D meshes, as the resulting compound
    /// has fewer sub-shapes than using individual triangles.
    ///
    /// The algorithm works by:
    /// 1. Starting with all triangles from the input mesh
    /// 2. Merging adjacent triangles if the result is still convex
    /// 3. Creating a compound from the resulting convex polygons
    ///
    /// # Arguments
    ///
    /// * `trimesh` - A reference to the triangle mesh to decompose
    ///
    /// # Returns
    ///
    /// * `Some(Compound)` - A compound shape containing the convex polygons
    /// * `None` - If any of the created shapes has zero or near-zero area
    ///
    /// # Performance
    ///
    /// This decomposition is typically much more efficient than using the raw triangle mesh,
    /// as it reduces the number of shapes from N triangles to potentially N/2 or fewer polygons.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::shape::{Compound, TriMesh};
    /// # use parry2d::math::Vector;
    ///
    /// // Create a simple square mesh (2 triangles)
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0),
    ///     Vector::new(1.0, 1.0),
    ///     Vector::new(0.0, 1.0),
    /// ];
    ///
    /// let indices = vec![
    ///     [0, 1, 2],  // First triangle
    ///     [0, 2, 3],  // Second triangle
    /// ];
    ///
    /// let trimesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// // Decompose into convex polygons
    /// if let Some(compound) = Compound::decompose_trimesh(&trimesh) {
    ///     // The two triangles should be merged into one or two convex polygons
    ///     assert!(compound.shapes().len() <= 2);
    /// }
    /// # }
    /// ```
    ///
    /// # Example: Complex Shape
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// # use parry2d::shape::{Compound, TriMesh};
    /// # use parry2d::math::Vector;
    ///
    /// // Create an L-shaped mesh
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(2.0, 0.0),
    ///     Vector::new(2.0, 1.0),
    ///     Vector::new(1.0, 1.0),
    ///     Vector::new(1.0, 2.0),
    ///     Vector::new(0.0, 2.0),
    /// ];
    ///
    /// let indices = vec![
    ///     [0, 1, 2],
    ///     [0, 2, 3],
    ///     [0, 3, 4],
    ///     [0, 4, 5],
    /// ];
    ///
    /// let trimesh = TriMesh::new(vertices, indices).unwrap();
    ///
    /// // Decompose the L-shape into convex polygons
    /// if let Some(compound) = Compound::decompose_trimesh(&trimesh) {
    ///     // The result will have fewer shapes than the original 4 triangles
    ///     assert!(compound.shapes().len() > 0);
    ///     println!("Decomposed into {} convex polygons", compound.shapes().len());
    /// }
    /// # }
    /// ```
    #[cfg(feature = "dim2")]
    pub fn decompose_trimesh(trimesh: &TriMesh) -> Option<Self> {
        let polygons = hertel_mehlhorn(trimesh.vertices(), trimesh.indices());
        let shapes: Option<Vec<_>> = polygons
            .into_iter()
            .map(|points| {
                match points.len() {
                    3 => {
                        let triangle = Triangle::new(points[0], points[1], points[2]);
                        Some(SharedShape::new(triangle))
                    }
                    _ => ConvexPolygon::from_convex_polyline(points).map(SharedShape::new),
                }
                .map(|shape| (Pose::IDENTITY, shape))
            })
            .collect();
        Some(Self::new(shapes?))
    }
}

impl Compound {
    /// Returns a slice containing all sub-shapes and their positions in this compound.
    ///
    /// Each element in the returned slice is a tuple containing:
    /// - The sub-shape's position and orientation ([`Pose`]) relative to the compound's origin
    /// - The sub-shape itself ([`SharedShape`])
    ///
    /// The order of shapes matches the order they were provided to [`Compound::new`].
    /// The index of each shape in this slice corresponds to its ID used in other operations
    /// like BVH traversal.
    ///
    /// # Returns
    ///
    /// A slice of (isometry, shape) pairs representing all sub-shapes in this compound.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{Compound, Ball, Cuboid, SharedShape};
    /// use parry3d::math::Pose;
    /// use parry3d::math::Vector;
    ///
    /// let shapes = vec![
    ///     (Pose::translation(1.0, 0.0, 0.0), SharedShape::new(Ball::new(0.5))),
    ///     (Pose::translation(-1.0, 0.0, 0.0), SharedShape::new(Ball::new(0.3))),
    ///     (Pose::identity(), SharedShape::new(Cuboid::new(Vector::new(0.5, 0.5, 0.5)))),
    /// ];
    ///
    /// let compound = Compound::new(shapes);
    ///
    /// // Access all shapes
    /// assert_eq!(compound.shapes().len(), 3);
    ///
    /// // Inspect individual shapes
    /// for (i, (position, shape)) in compound.shapes().iter().enumerate() {
    ///     println!("Shape {} at position: {:?}", i, position.translation);
    ///
    ///     // Check if it's a ball
    ///     if let Some(ball) = shape.as_ball() {
    ///         println!("  Ball with radius: {}", ball.radius);
    ///     }
    /// }
    /// # }
    /// ```
    ///
    /// # Example: Modifying Sub-Shape Positions
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{Compound, Ball, SharedShape};
    /// use parry3d::math::Pose;
    ///
    /// let shapes = vec![
    ///     (Pose::translation(0.0, 0.0, 0.0), SharedShape::new(Ball::new(1.0))),
    ///     (Pose::translation(2.0, 0.0, 0.0), SharedShape::new(Ball::new(1.0))),
    /// ];
    ///
    /// let compound = Compound::new(shapes);
    ///
    /// // Note: To modify positions, you need to create a new compound
    /// let mut new_shapes: Vec<_> = compound.shapes()
    ///     .iter()
    ///     .map(|(pos, shape)| {
    ///         // Shift all shapes up by 1 unit
    ///         let new_pos = pos * Pose::translation(0.0, 1.0, 0.0);
    ///         (new_pos, shape.clone())
    ///     })
    ///     .collect();
    ///
    /// let shifted_compound = Compound::new(new_shapes);
    /// assert_eq!(shifted_compound.shapes().len(), 2);
    /// # }
    /// ```
    #[inline]
    pub fn shapes(&self) -> &[(Pose, SharedShape)] {
        &self.shapes[..]
    }

    /// Returns the Axis-Aligned Bounding Box (AABB) of this compound in local space.
    ///
    /// The local AABB is the smallest axis-aligned box that contains all sub-shapes
    /// in the compound, computed in the compound's local coordinate system. This AABB
    /// is automatically computed when the compound is created and encompasses all
    /// sub-shapes at their specified positions.
    ///
    /// # Returns
    ///
    /// A reference to the [`Aabb`] representing the compound's bounding box in local space.
    ///
    /// # Use Cases
    ///
    /// - **Broad-phase collision detection**: Quick rejection tests before detailed queries
    /// - **Spatial partitioning**: Organizing compounds in larger spatial structures
    /// - **Culling**: Determining if the compound is visible or relevant to a query
    /// - **Size estimation**: Getting approximate dimensions of the compound
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{Compound, Ball, SharedShape};
    /// use parry3d::math::Pose;
    /// use parry3d::math::Vector;
    ///
    /// let shapes = vec![
    ///     (Pose::translation(-2.0, 0.0, 0.0), SharedShape::new(Ball::new(0.5))),
    ///     (Pose::translation(2.0, 0.0, 0.0), SharedShape::new(Ball::new(0.5))),
    /// ];
    ///
    /// let compound = Compound::new(shapes);
    /// let aabb = compound.local_aabb();
    ///
    /// // The AABB should contain both balls
    /// // Left ball extends from -2.5 to -1.5 on X axis
    /// // Right ball extends from 1.5 to 2.5 on X axis
    /// assert!(aabb.mins.x <= -2.5);
    /// assert!(aabb.maxs.x >= 2.5);
    ///
    /// // Check if a point is inside the AABB
    /// assert!(aabb.contains_local_point(Vector::ZERO));
    /// assert!(!aabb.contains_local_point(Vector::new(10.0, 0.0, 0.0)));
    /// # }
    /// ```
    ///
    /// # Example: Computing Compound Dimensions
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{Compound, Cuboid, SharedShape};
    /// use parry3d::math::Pose;
    /// use parry3d::math::Vector;
    ///
    /// let shapes = vec![
    ///     (Pose::identity(), SharedShape::new(Cuboid::new(Vector::new(1.0, 1.0, 1.0)))),
    ///     (Pose::translation(3.0, 0.0, 0.0), SharedShape::new(Cuboid::new(Vector::new(0.5, 0.5, 0.5)))),
    /// ];
    ///
    /// let compound = Compound::new(shapes);
    /// let aabb = compound.local_aabb();
    ///
    /// // Calculate the total dimensions
    /// let dimensions = aabb.maxs - aabb.mins;
    /// println!("Compound dimensions: {:?}", dimensions);
    ///
    /// // The compound extends from -1.0 to 3.5 on the X axis (4.5 units total)
    /// assert!((dimensions.x - 4.5).abs() < 1e-5);
    /// # }
    /// ```
    #[inline]
    pub fn local_aabb(&self) -> &Aabb {
        &self.aabb
    }

    /// Returns the bounding sphere of this compound in local space.
    ///
    /// The bounding sphere is the smallest sphere that contains all sub-shapes in the
    /// compound. It is computed from the compound's AABB by finding the sphere that
    /// tightly encloses that box. This provides a simple, rotation-invariant bounding
    /// volume useful for certain collision detection algorithms.
    ///
    /// # Returns
    ///
    /// A [`BoundingSphere`] centered in local space that contains the entire compound.
    ///
    /// # Performance
    ///
    /// This method is very fast as it simply computes the bounding sphere from the
    /// pre-computed AABB. The bounding sphere is not cached - it's computed on each call.
    ///
    /// # Comparison with AABB
    ///
    /// - **Bounding Sphere**: Rotation-invariant, simpler intersection tests, but often looser fit
    /// - **AABB**: Tighter fit for axis-aligned objects, but must be recomputed when rotated
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{Compound, Ball, SharedShape};
    /// use parry3d::math::Pose;
    ///
    /// let shapes = vec![
    ///     (Pose::translation(-1.0, 0.0, 0.0), SharedShape::new(Ball::new(0.5))),
    ///     (Pose::translation(1.0, 0.0, 0.0), SharedShape::new(Ball::new(0.5))),
    /// ];
    ///
    /// let compound = Compound::new(shapes);
    /// let bounding_sphere = compound.local_bounding_sphere();
    ///
    /// // The bounding sphere should contain both balls
    /// println!("Center: {:?}", bounding_sphere.center());
    /// println!("Radius: {}", bounding_sphere.radius());
    ///
    /// // The center should be near the origin
    /// assert!(bounding_sphere.center().length() < 0.1);
    ///
    /// // The radius should be at least 1.5 (distance to ball edge: 1.0 + 0.5)
    /// assert!(bounding_sphere.radius() >= 1.5);
    /// # }
    /// ```
    ///
    /// # Example: Using Bounding Sphere for Quick Rejection
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{Compound, Cuboid, SharedShape};
    /// use parry3d::math::{Pose, Vector};
    ///
    /// let shapes = vec![
    ///     (Pose::identity(), SharedShape::new(Cuboid::new(Vector::new(1.0, 1.0, 1.0)))),
    ///     (Pose::translation(2.0, 0.0, 0.0), SharedShape::new(Cuboid::new(Vector::new(0.5, 0.5, 0.5)))),
    /// ];
    ///
    /// let compound = Compound::new(shapes);
    /// let sphere = compound.local_bounding_sphere();
    ///
    /// // Quick test: is a point potentially inside the compound?
    /// let test_point = Vector::new(5.0, 5.0, 5.0);
    /// let distance_to_center = (test_point - sphere.center()).length();
    ///
    /// if distance_to_center > sphere.radius() {
    ///     println!("Vector is definitely outside the compound");
    ///     assert!(distance_to_center > sphere.radius());
    /// } else {
    ///     println!("Vector might be inside - need detailed check");
    /// }
    /// # }
    /// ```
    #[inline]
    pub fn local_bounding_sphere(&self) -> BoundingSphere {
        self.aabb.bounding_sphere()
    }

    /// Returns a slice of AABBs, one for each sub-shape in this compound.
    ///
    /// Each AABB in the returned slice corresponds to the bounding box of a sub-shape,
    /// transformed to the compound's local coordinate system. The AABBs are stored in
    /// the same order as the shapes returned by [`Compound::shapes`], so index `i` in
    /// this slice corresponds to shape `i`.
    ///
    /// These AABBs are used internally by the BVH for efficient spatial queries and
    /// collision detection. They are pre-computed during compound construction.
    ///
    /// # Returns
    ///
    /// A slice of [`Aabb`] representing the local-space bounding boxes of each sub-shape.
    ///
    /// # Use Cases
    ///
    /// - Inspecting individual sub-shape bounds without accessing the shapes themselves
    /// - Custom spatial queries or culling operations
    /// - Debugging and visualization of the compound's structure
    /// - Understanding the BVH's leaf nodes
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{Compound, Ball, Cuboid, SharedShape};
    /// use parry3d::math::Pose;
    /// use parry3d::math::Vector;
    ///
    /// let shapes = vec![
    ///     (Pose::translation(-2.0, 0.0, 0.0), SharedShape::new(Ball::new(0.5))),
    ///     (Pose::identity(), SharedShape::new(Cuboid::new(Vector::new(1.0, 1.0, 1.0)))),
    ///     (Pose::translation(3.0, 0.0, 0.0), SharedShape::new(Ball::new(0.3))),
    /// ];
    ///
    /// let compound = Compound::new(shapes);
    ///
    /// // Get AABBs for all sub-shapes
    /// let aabbs = compound.aabbs();
    /// assert_eq!(aabbs.len(), 3);
    ///
    /// // Inspect each AABB
    /// for (i, aabb) in aabbs.iter().enumerate() {
    ///     println!("Shape {} AABB:", i);
    ///     println!("  Min: {:?}", aabb.mins);
    ///     println!("  Max: {:?}", aabb.maxs);
    ///
    ///     let center = aabb.center();
    ///     let extents = aabb.half_extents();
    ///     println!("  Center: {:?}", center);
    ///     println!("  Half-extents: {:?}", extents);
    /// }
    /// # }
    /// ```
    ///
    /// # Example: Finding Sub-Shapes in a Region
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{Compound, Ball, SharedShape};
    /// use parry3d::math::Pose;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let shapes = vec![
    ///     (Pose::translation(-5.0, 0.0, 0.0), SharedShape::new(Ball::new(0.5))),
    ///     (Pose::translation(0.0, 0.0, 0.0), SharedShape::new(Ball::new(0.5))),
    ///     (Pose::translation(5.0, 0.0, 0.0), SharedShape::new(Ball::new(0.5))),
    /// ];
    ///
    /// let compound = Compound::new(shapes);
    ///
    /// // Define a query point
    /// let query_point = Vector::ZERO;
    ///
    /// // Find which sub-shapes might contain this point
    /// let potentially_containing: Vec<usize> = compound.aabbs()
    ///     .iter()
    ///     .enumerate()
    ///     .filter(|(_, aabb)| aabb.contains_local_point(query_point))
    ///     .map(|(i, _)| i)
    ///     .collect();
    ///
    /// // Only the middle shape (index 1) should contain the origin
    /// assert_eq!(potentially_containing.len(), 1);
    /// assert_eq!(potentially_containing[0], 1);
    /// # }
    /// ```
    #[inline]
    pub fn aabbs(&self) -> &[Aabb] {
        &self.aabbs[..]
    }

    /// Returns the Bounding Volume Hierarchy (BVH) used for efficient spatial queries.
    ///
    /// The BVH is an acceleration structure that organizes the sub-shapes hierarchically
    /// for fast collision detection and spatial queries. It enables logarithmic-time queries
    /// instead of linear searches through all sub-shapes. The BVH is automatically built
    /// when the compound is created using a binned construction strategy.
    ///
    /// # Returns
    ///
    /// A reference to the [`Bvh`] acceleration structure for this compound.
    ///
    /// # Use Cases
    ///
    /// - **Custom spatial queries**: Traverse the BVH for specialized collision detection
    /// - **Ray casting**: Efficiently find which sub-shapes intersect a ray
    /// - **AABB queries**: Find all sub-shapes intersecting a region
    /// - **Debugging**: Inspect the BVH structure and quality
    /// - **Performance analysis**: Understand query performance characteristics
    ///
    /// # Performance
    ///
    /// The BVH provides O(log n) query performance for most spatial operations, where n
    /// is the number of sub-shapes. For compounds with many shapes (100+), the BVH
    /// provides dramatic speedups compared to naive linear searches.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{Compound, Ball, SharedShape};
    /// use parry3d::math::Pose;
    /// use parry3d::bounding_volume::Aabb;
    /// use parry3d::math::Vector;
    ///
    /// let shapes = vec![
    ///     (Pose::translation(-3.0, 0.0, 0.0), SharedShape::new(Ball::new(0.5))),
    ///     (Pose::translation(0.0, 0.0, 0.0), SharedShape::new(Ball::new(0.5))),
    ///     (Pose::translation(3.0, 0.0, 0.0), SharedShape::new(Ball::new(0.5))),
    /// ];
    ///
    /// let compound = Compound::new(shapes);
    /// let bvh = compound.bvh();
    ///
    /// // The BVH provides efficient hierarchical organization
    /// assert_eq!(bvh.leaf_count(), 3);
    /// println!("BVH root AABB: {:?}", bvh.root_aabb());
    /// # }
    /// ```
    ///
    /// # Example: Accessing BVH for Custom Queries
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # use parry3d::shape::{Compound, Ball, SharedShape};
    /// use parry3d::math::Pose;
    ///
    /// let mut shapes = vec![];
    /// for i in 0..10 {
    ///     let x = i as f32 * 2.0;
    ///     shapes.push((
    ///         Pose::translation(x, 0.0, 0.0),
    ///         SharedShape::new(Ball::new(0.5))
    ///     ));
    /// }
    ///
    /// let compound = Compound::new(shapes);
    /// let bvh = compound.bvh();
    ///
    /// // The BVH organizes 10 shapes hierarchically
    /// assert_eq!(bvh.leaf_count(), 10);
    /// assert_eq!(compound.shapes().len(), 10);
    ///
    /// // Access the root AABB which bounds all shapes
    /// let root_aabb = bvh.root_aabb();
    /// println!("Root AABB spans from {:?} to {:?}", root_aabb.mins, root_aabb.maxs);
    /// # }
    /// ```
    #[inline]
    pub fn bvh(&self) -> &Bvh {
        &self.bvh
    }
}

impl CompositeShape for Compound {
    #[inline]
    fn map_part_at(
        &self,
        shape_id: u32,
        f: &mut dyn FnMut(Option<&Pose>, &dyn Shape, Option<&dyn NormalConstraints>),
    ) {
        if let Some(shape) = self.shapes.get(shape_id as usize) {
            f(Some(&shape.0), &*shape.1, None)
        }
    }

    #[inline]
    fn bvh(&self) -> &Bvh {
        &self.bvh
    }
}

impl TypedCompositeShape for Compound {
    type PartShape = dyn Shape;
    type PartNormalConstraints = ();

    #[inline(always)]
    fn map_typed_part_at<T>(
        &self,
        i: u32,
        mut f: impl FnMut(Option<&Pose>, &Self::PartShape, Option<&Self::PartNormalConstraints>) -> T,
    ) -> Option<T> {
        let (part_pos, part) = &self.shapes[i as usize];
        Some(f(Some(part_pos), &**part, None))
    }

    #[inline(always)]
    fn map_untyped_part_at<T>(
        &self,
        i: u32,
        mut f: impl FnMut(Option<&Pose>, &Self::PartShape, Option<&dyn NormalConstraints>) -> T,
    ) -> Option<T> {
        let (part_pos, part) = &self.shapes[i as usize];
        Some(f(Some(part_pos), &**part, None))
    }
}
