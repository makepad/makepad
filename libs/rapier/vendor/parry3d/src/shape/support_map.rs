//! Traits for support mapping based shapes.
//!
//! # What is a Support Map?
//!
//! A **support map** (or support function) is a fundamental concept in computational geometry that
//! describes a convex shape by answering a simple question: "What point on the shape is furthest
//! in a given direction?"
//!
//! More formally, for a convex shape `S` and a direction vector `d`, the support function returns:
//!
//! ```text
//! support(S, d) = argmax { p · d : p ∈ S }
//! ```
//!
//! Where `p · d` is the dot product between a point `p` on the shape and the direction `d`.
//!
//! ## Visual Intuition
//!
//! Imagine shining a light from infinity in direction `d` onto your shape. The support point
//! is where the "shadow" boundary would be - the point that sticks out furthest in that direction.
//!
//! For example, for a circle centered at the origin with radius `r`:
//! - If `d = (1, 0)` (pointing right), the support point is `(r, 0)` (rightmost point)
//! - If `d = (0, 1)` (pointing up), the support point is `(0, r)` (topmost point)
//! - If `d = (1, 1)` (diagonal), the support point is `(r/√2, r/√2)` (northeast point)
//!
//! ## Why Support Maps Matter for Collision Detection
//!
//! Support maps are the foundation of two powerful collision detection algorithms:
//!
//! ### 1. GJK Algorithm (Gilbert-Johnson-Keerthi)
//!
//! GJK is an iterative algorithm that determines:
//! - Whether two convex shapes intersect
//! - The distance between two separated shapes
//! - The closest points between two shapes
//!
//! GJK works by computing the **Minkowski difference** of two shapes using only their support
//! functions. It builds a simplex (a simple polytope) that converges toward the origin, allowing
//! it to answer collision queries without ever explicitly computing the shapes' geometry.
//!
//! **Key advantage**: GJK only needs the support function - it never needs to know the actual
//! vertices, faces, or internal structure of the shapes. This makes it incredibly flexible and
//! efficient.
//!
//! ### 2. EPA Algorithm (Expanding Polytope Algorithm)
//!
//! EPA is used when two shapes are penetrating (overlapping). It computes:
//! - The penetration depth (how much they overlap)
//! - The penetration normal (the direction to separate them)
//! - Contact points for physics simulation
//!
//! EPA starts with the final simplex from GJK and expands it into a polytope that approximates
//! the Minkowski difference, converging toward the shallowest penetration.
//!
//! ## Why Support Maps are Efficient
//!
//! 1. **Simple to implement**: For most convex shapes, the support function is straightforward
//! 2. **No geometry storage**: Implicit shapes (like spheres, capsules) don't need vertex data
//! 3. **Transform-friendly**: Easy to handle rotations and translations
//! 4. **Composable**: Can combine support functions for compound shapes
//! 5. **Fast queries**: Often just a few dot products and comparisons
//!
//! ## Examples of Support Functions
//!
//! Here are some common shapes and their support functions:
//!
//! ### Sphere/Ball
//! ```text
//! support(sphere, d) = center + normalize(d) * radius
//! ```
//!
//! ### Cuboid (Box)
//! ```text
//! support(box, d) = (sign(d.x) * half_width,
//!                    sign(d.y) * half_height,
//!                    sign(d.z) * half_depth)
//! ```
//!
//! ### Convex Polygon/Polyhedron
//! ```text
//! support(poly, d) = vertex with maximum dot product with d
//! ```
//!
//! ## Limitations
//!
//! Support maps only work for **convex** shapes. Concave shapes must be decomposed into
//! convex parts or handled with different algorithms. This is why Parry provides composite
//! shapes and specialized algorithms for triangle meshes.

use crate::math::{Pose, Vector};

/// Trait for convex shapes representable by a support mapping function.
///
/// A support map is a function that returns the point on a shape that is furthest in a given
/// direction. This is the fundamental building block for collision detection algorithms like
/// GJK (Gilbert-Johnson-Keerthi) and EPA (Expanding Polytope Algorithm).
///
/// # What You Need to Know
///
/// If you're implementing this trait for a custom shape, you only need to implement
/// [`local_support_point`](SupportMap::local_support_point). The other methods have default
/// implementations that handle transformations and normalized directions.
///
/// # Requirements
///
/// - The shape **must be convex**. Non-convex shapes will produce incorrect results.
/// - The support function should return a point on the surface of the shape (or inside it,
///   but surface points are preferred for better accuracy).
/// - For a given direction `d`, the returned point `p` should maximize `p · d` (dot product).
///
/// # Examples
///
/// ## Using Support Maps for Distance Queries
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Ball, Cuboid, SupportMap};
/// use parry3d::math::{Vector, Real};
///
/// // Create a ball (sphere) with radius 1.0
/// let ball = Ball::new(1.0);
///
/// // Get the support point in the direction (1, 0, 0) - pointing right
/// let dir = Vector::new(1.0, 0.0, 0.0);
/// let support_point = ball.local_support_point(dir);
///
/// // For a ball centered at origin, this should be approximately (1, 0, 0)
/// assert!((support_point.x - 1.0).abs() < 1e-6);
/// assert!(support_point.y.abs() < 1e-6);
/// assert!(support_point.z.abs() < 1e-6);
///
/// // Try another direction - diagonal up and right
/// let dir2 = Vector::new(1.0, 1.0, 0.0);
/// let support_point2 = ball.local_support_point(dir2);
///
/// // The point should be on the surface of the ball (distance = radius)
/// let distance = (support_point2.length() - 1.0).abs();
/// assert!(distance < 1e-6);
/// # }
/// ```
///
/// ## Support Vectors on a Cuboid
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Cuboid, SupportMap};
/// use parry3d::math::Vector;
///
/// // Create a cuboid (box) with half-extents 2x3x4
/// let cuboid = Cuboid::new(Vector::new(2.0, 3.0, 4.0));
///
/// // Support point in positive X direction should be at the right face
/// let dir_x = Vector::new(1.0, 0.0, 0.0);
/// let support_x = cuboid.local_support_point(dir_x);
/// assert!((support_x.x - 2.0).abs() < 1e-6);
///
/// // Support point in negative Y direction should be at the bottom face
/// let dir_neg_y = Vector::new(0.0, -1.0, 0.0);
/// let support_neg_y = cuboid.local_support_point(dir_neg_y);
/// assert!((support_neg_y.y + 3.0).abs() < 1e-6);
///
/// // Support point in diagonal direction should be at a corner
/// let dir_diag = Vector::new(1.0, 1.0, 1.0);
/// let support_diag = cuboid.local_support_point(dir_diag);
/// assert!((support_diag.x - 2.0).abs() < 1e-6);
/// assert!((support_diag.y - 3.0).abs() < 1e-6);
/// assert!((support_diag.z - 4.0).abs() < 1e-6);
/// # }
/// ```
///
/// ## Implementing SupportMap for a Custom Shape
///
/// Here's how you might implement `SupportMap` for a simple custom shape:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # // Note: This example shows the concept but won't actually compile in doc tests
/// # // since we can't implement traits for external types in doc tests.
/// # // It's here for educational purposes.
/// use parry3d::shape::SupportMap;
/// use parry3d::math::{Vector, Real};
///
/// // A simple pill-shaped object aligned with the X axis
/// struct SimplePill {
///     half_length: Real,  // Half the length of the cylindrical part
///     radius: Real,       // Radius of the spherical ends
/// }
///
/// impl SupportMap for SimplePill {
///     fn local_support_point(&self, dir: Vector) -> Vector {
///         // Support point is on one of the spherical ends
///         // Choose the end that's in the direction of dir.x
///         let center_x = if dir.x >= 0.0 { self.half_length } else { -self.half_length };
///
///         // From that center, extend by radius in the direction of dir
///         let dir_normalized = dir.normalize();
///         Vector::new(
///             center_x + dir_normalized.x * self.radius,
///             dir_normalized.y * self.radius,
///             dir_normalized.z * self.radius,
///         )
///     }
/// }
/// # }
/// ```
pub trait SupportMap {
    /// Evaluates the support function of this shape in local space.
    ///
    /// The support function returns the point on the shape's surface (or interior) that is
    /// furthest in the given direction `dir`. More precisely, it finds the point `p` that
    /// maximizes the dot product `p · dir`.
    ///
    /// # Parameters
    ///
    /// - `dir`: The direction vector to query. Does not need to be normalized (unit length),
    ///   but the result may be more intuitive with normalized directions.
    ///
    /// # Returns
    ///
    /// A point on (or inside) the shape that is furthest in the direction `dir`.
    ///
    /// # Implementation Notes
    ///
    /// When implementing this method:
    /// - The direction `dir` may have any length (including zero, though this is unusual)
    /// - For better numerical stability, consider normalizing `dir` if needed
    /// - The returned point should be in the shape's local coordinate system
    /// - If `dir` is zero or very small, a reasonable point (like the center) should be returned
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{Ball, SupportMap};
    /// use parry3d::math::Vector;
    ///
    /// let ball = Ball::new(2.5);
    ///
    /// // Support point pointing up (Z direction)
    /// let up = Vector::new(0.0, 0.0, 1.0);
    /// let support_up = ball.local_support_point(up);
    ///
    /// // Should be at the top of the ball
    /// assert!((support_up.z - 2.5).abs() < 1e-6);
    /// assert!(support_up.x.abs() < 1e-6);
    /// assert!(support_up.y.abs() < 1e-6);
    ///
    /// // Support point pointing in negative X direction
    /// let left = Vector::new(-1.0, 0.0, 0.0);
    /// let support_left = ball.local_support_point(left);
    ///
    /// // Should be at the left side of the ball
    /// assert!((support_left.x + 2.5).abs() < 1e-6);
    /// # }
    /// ```
    ///
    /// ## Why "local" support point?
    ///
    /// The "local" prefix means the point is in the shape's own coordinate system, before
    /// any rotation or translation is applied. For transformed shapes, use
    /// [`support_point`](SupportMap::support_point) instead.
    fn local_support_point(&self, dir: Vector) -> Vector;

    /// Same as [`local_support_point`](SupportMap::local_support_point) except that `dir` is
    /// guaranteed to be normalized (unit length).
    ///
    /// This can be more efficient for some shapes that can take advantage of the normalized
    /// direction vector. By default, this just forwards to `local_support_point`.
    ///
    /// # Parameters
    ///
    /// - `dir`: A unit-length direction vector (guaranteed by the type system)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{Ball, SupportMap};
    /// use parry3d::math::Vector;
    ///
    /// let ball = Ball::new(1.5);
    ///
    /// // Create a normalized direction vector
    /// let dir = Vector::new(1.0, 1.0, 0.0).normalize();
    ///
    /// let support = ball.local_support_point_toward(dir);
    ///
    /// // The support point should be on the sphere's surface
    /// let distance_from_origin = support.length();
    /// assert!((distance_from_origin - 1.5).abs() < 1e-6);
    /// # }
    /// ```
    fn local_support_point_toward(&self, dir: Vector) -> Vector {
        self.local_support_point(dir)
    }

    /// Evaluates the support function of this shape transformed by `transform`.
    ///
    /// This is the world-space version of [`local_support_point`](SupportMap::local_support_point).
    /// It computes the support point for a shape that has been rotated and translated by the
    /// given isometry (rigid transformation).
    ///
    /// # How it Works
    ///
    /// The algorithm:
    /// 1. Transform the direction vector `dir` from world space to the shape's local space
    /// 2. Compute the support point in local space
    /// 3. Transform the support point from local space back to world space
    ///
    /// This is much more efficient than actually transforming the shape's geometry.
    ///
    /// # Parameters
    ///
    /// - `transform`: The rigid transformation (rotation + translation) applied to the shape
    /// - `dir`: The direction vector in world space
    ///
    /// # Returns
    ///
    /// A point in world space that is the furthest point on the transformed shape in direction `dir`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{Ball, SupportMap};
    /// use parry3d::math::{Pose, Vector};
    ///
    /// let ball = Ball::new(1.0);
    ///
    /// // Create a transformation: translate the ball to (10, 0, 0)
    /// let transform = Pose::translation(10.0, 0.0, 0.0);
    ///
    /// // Get support point in the positive X direction
    /// let dir = Vector::new(1.0, 0.0, 0.0);
    /// let support = ball.support_point(&transform, dir);
    ///
    /// // The support point should be at (11, 0, 0) - the rightmost point of the translated ball
    /// assert!((support.x - 11.0).abs() < 1e-6);
    /// assert!(support.y.abs() < 1e-6);
    /// assert!(support.z.abs() < 1e-6);
    /// # }
    /// ```
    ///
    /// ## Example with Rotation
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{Cuboid, SupportMap};
    /// use parry3d::math::{Pose, Rotation, Vector};
    /// use core::f32::consts::PI;
    ///
    /// let cuboid = Cuboid::new(Vector::new(2.0, 1.0, 1.0));
    ///
    /// // Rotate the cuboid 90 degrees around the Z axis
    /// let rotation = Rotation::from_axis_angle(Vector::Z, PI / 2.0);
    /// let transform = Pose::from_parts(Vector::ZERO, rotation);
    ///
    /// // In world space, ask for support in the X direction
    /// let dir = Vector::new(1.0, 0.0, 0.0);
    /// let support = cuboid.support_point(&transform, dir);
    ///
    /// // After 90° rotation, the long axis (originally X) now points in Y direction
    /// // So the support in X direction comes from the short axis
    /// assert!(support.x.abs() <= 1.0 + 1e-5); // Should be around 1.0 (the short axis)
    /// # }
    /// ```
    fn support_point(&self, transform: &Pose, dir: Vector) -> Vector {
        let local_dir = transform.rotation.inverse() * dir;
        transform * self.local_support_point(local_dir)
    }

    /// Same as [`support_point`](SupportMap::support_point) except that `dir` is guaranteed
    /// to be normalized (unit length).
    ///
    /// This combines the benefits of both [`local_support_point_toward`](SupportMap::local_support_point_toward)
    /// (normalized direction) and [`support_point`](SupportMap::support_point) (world-space transform).
    ///
    /// # Parameters
    ///
    /// - `transform`: The rigid transformation applied to the shape
    /// - `dir`: A unit-length direction vector in world space
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{Ball, SupportMap};
    /// use parry3d::math::{Pose, Vector};
    ///
    /// let ball = Ball::new(2.0);
    ///
    /// // Translate the ball
    /// let transform = Pose::translation(5.0, 3.0, -2.0);
    ///
    /// // Create a normalized direction
    /// let dir = Vector::new(1.0, 1.0, 1.0).normalize();
    ///
    /// let support = ball.support_point_toward(&transform, dir);
    ///
    /// // The support point should be 2.0 units away from the center in the diagonal direction
    /// let center = Vector::new(5.0, 3.0, -2.0);
    /// let offset = support - center;
    /// let distance = offset.length();
    /// assert!((distance - 2.0).abs() < 1e-6);
    ///
    /// // The offset should be parallel to the direction
    /// let normalized_offset = offset.normalize();
    /// assert!((normalized_offset.dot(dir) - 1.0).abs() < 1e-6);
    /// # }
    /// ```
    ///
    /// ## Practical Use: GJK Algorithm
    ///
    /// This method is commonly used in the GJK algorithm, which needs to compute support points
    /// for transformed shapes with normalized directions for better numerical stability:
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{Ball, Cuboid, SupportMap};
    /// use parry3d::math::{Pose, Vector};
    ///
    /// // Two shapes at different positions
    /// let ball = Ball::new(1.0);
    /// let cuboid = Cuboid::new(Vector::new(0.5, 0.5, 0.5));
    ///
    /// let ball_pos = Pose::translation(0.0, 0.0, 0.0);
    /// let cuboid_pos = Pose::translation(3.0, 0.0, 0.0);
    ///
    /// // Direction from ball to cuboid
    /// let dir = Vector::new(1.0, 0.0, 0.0).normalize();
    ///
    /// // Get support points for the Minkowski difference (used in GJK)
    /// let support_ball = ball.support_point_toward(&ball_pos, dir);
    /// let support_cuboid = cuboid.support_point_toward(&cuboid_pos, -dir);
    ///
    /// // The Minkowski difference support point
    /// let minkowski_support = support_ball - support_cuboid;
    ///
    /// println!("Support point for Minkowski difference: {:?}", minkowski_support);
    /// # }
    /// ```
    fn support_point_toward(&self, transform: &Pose, dir: Vector) -> Vector {
        let local_dir = transform.rotation.inverse() * dir;
        transform * self.local_support_point_toward(local_dir)
    }
}
