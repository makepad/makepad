//! Rounded shapes are shapes with smoothed/rounded borders.
//!
//! This module provides the `RoundShape` wrapper that adds a border radius to any shape,
//! effectively creating a "padded" or "rounded" version of the original shape.

use crate::math::{Real, Vector};
use crate::shape::SupportMap;

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(Copy, Clone, Debug)]
#[repr(C)]
/// A shape with rounded borders.
///
/// # What is a Rounded Shape?
///
/// A `RoundShape` wraps an existing shape and adds a "border radius" around it. This creates
/// a smooth, rounded version of the original shape by effectively expanding it outward by
/// the border radius distance. Think of it as adding padding or a cushion around the shape.
///
/// The rounding is achieved by using Minkowski sum operations: any point on the surface of
/// the rounded shape is computed by taking a point on the original shape's surface and moving
/// it outward along the surface normal by the border radius distance.
///
/// # Common Use Cases
///
/// - **Creating softer collisions**: Rounded shapes can make collision detection more forgiving
///   and realistic, as sharp corners and edges are smoothed out.
/// - **Capsule-like shapes**: You can create capsule variations of any shape by adding a
///   border radius (e.g., a rounded cuboid becomes similar to a capsule).
/// - **Visual aesthetics**: Rounded shapes often look more pleasing and natural than sharp-edged
///   shapes.
/// - **Improved numerical stability**: Rounded shapes can sometimes be more numerically stable
///   in collision detection algorithms since they avoid sharp corners.
///
/// # Examples
///
/// ## Creating a Rounded Cuboid
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # #[cfg(feature = "f32")]
/// use parry3d::shape::{RoundShape, Cuboid};
/// # #[cfg(feature = "f64")]
/// use parry3d_f64::shape::{RoundShape, Cuboid};
/// use parry3d::math::Vector;
///
/// // Create a cuboid with half-extents of 1.0 in each direction
/// let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
///
/// // Add a border radius of 0.2 to create a rounded cuboid
/// let rounded_cuboid = RoundShape {
///     inner_shape: cuboid,
///     border_radius: 0.2,
/// };
///
/// // The effective size is now 1.2 in each direction from the center
/// // (1.0 from the cuboid + 0.2 from the border)
/// assert_eq!(rounded_cuboid.inner_shape.half_extents.x, 1.0);
/// assert_eq!(rounded_cuboid.border_radius, 0.2);
/// # }
/// ```
///
/// ## Creating a Rounded Triangle (2D)
///
/// ```rust
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// # #[cfg(feature = "f32")]
/// use parry2d::shape::{RoundShape, Triangle};
/// # #[cfg(feature = "f64")]
/// use parry2d_f64::shape::{RoundShape, Triangle};
/// use parry2d::math::Vector;
///
/// // Create a triangle
/// let triangle = Triangle::new(
///     Vector::ZERO,
///     Vector::new(1.0, 0.0),
///     Vector::new(0.0, 1.0),
/// );
///
/// // Add rounding with a 0.1 border radius
/// let rounded_triangle = RoundShape {
///     inner_shape: triangle,
///     border_radius: 0.1,
/// };
///
/// // The rounded triangle will have smooth, curved edges instead of sharp corners
/// assert_eq!(rounded_triangle.border_radius, 0.1);
/// # }
/// ```
///
/// ## Comparing Support Vectors
///
/// This example shows how the border radius affects the support point calculation:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # #[cfg(feature = "f32")]
/// use parry3d::shape::{RoundShape, Cuboid, SupportMap};
/// # #[cfg(feature = "f64")]
/// use parry3d_f64::shape::{RoundShape, Cuboid, SupportMap};
/// use parry3d::math::Vector;
///
/// let cuboid = Cuboid::new(Vector::splat(1.0));
/// let rounded_cuboid = RoundShape {
///     inner_shape: cuboid,
///     border_radius: 0.5,
/// };
///
/// // Query the support point in the direction (1, 1, 1)
/// let direction = Vector::new(1.0, 1.0, 1.0);
/// let support_point = rounded_cuboid.local_support_point(direction);
///
/// // The support point will be further out than the original cuboid's support point
/// // due to the border radius
/// let cuboid_support = cuboid.local_support_point(direction);
///
/// // The rounded shape extends further in all directions
/// assert!(support_point.x > cuboid_support.x);
/// assert!(support_point.y > cuboid_support.y);
/// assert!(support_point.z > cuboid_support.z);
/// # }
/// ```
///
/// ## Using with Different Shape Types
///
/// `RoundShape` can wrap any shape that implements the `SupportMap` trait:
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// # #[cfg(feature = "f32")]
/// use parry3d::shape::{RoundShape, Ball, Segment, SupportMap};
/// # #[cfg(feature = "f64")]
/// use parry3d_f64::shape::{RoundShape, Ball, Segment, SupportMap};
/// use parry3d::math::Vector;
///
/// // Rounded ball (creates a slightly larger sphere)
/// let ball = Ball::new(1.0);
/// let rounded_ball = RoundShape {
///     inner_shape: ball,
///     border_radius: 0.1,
/// };
/// // Effective radius is now 1.1
///
/// // Rounded segment (creates a capsule)
/// let segment = Segment::new(
///     Vector::ZERO,
///     Vector::new(0.0, 2.0, 0.0),
/// );
/// let rounded_segment = RoundShape {
///     inner_shape: segment,
///     border_radius: 0.5,
/// };
/// // This creates a capsule with radius 0.5
/// # }
/// ```
///
/// # Performance Considerations
///
/// - The computational cost of queries on a `RoundShape` is essentially the same as for the
///   inner shape, plus a small constant overhead to apply the border radius.
/// - `RoundShape` is most efficient when used with shapes that already implement `SupportMap`
///   efficiently (like primitives: Ball, Cuboid, Capsule, etc.).
/// - The struct is `Copy` when the inner shape is `Copy`, making it efficient to pass around.
///
/// # Technical Details
///
/// The `RoundShape` implements the `SupportMap` trait by computing the support point of the
/// inner shape and then offsetting it by the border radius in the query direction. This is
/// mathematically equivalent to computing the Minkowski sum of the inner shape with a ball
/// of radius equal to the border radius.
pub struct RoundShape<S> {
    /// The shape being rounded.
    ///
    /// This is the original, "inner" shape before the border radius is applied.
    /// The rounded shape's surface will be at a distance of `border_radius` from
    /// this inner shape's surface.
    pub inner_shape: S,

    /// The radius of the rounded border.
    ///
    /// This value determines how much the shape is expanded outward. A larger border
    /// radius creates a more "padded" shape. Must be non-negative (typically positive).
    ///
    /// For example, if `border_radius` is 0.5, every point on the original shape's
    /// surface will be moved 0.5 units outward along its surface normal.
    pub border_radius: Real,
}

impl<S: SupportMap> SupportMap for RoundShape<S> {
    /// Computes the support point of the rounded shape in the given direction.
    ///
    /// The support point is the point on the shape's surface that is furthest in the
    /// given direction. For a rounded shape, this is computed by:
    /// 1. Finding the support point of the inner shape in the given direction
    /// 2. Moving that point outward by `border_radius` units along the direction
    ///
    /// # Parameters
    ///
    /// * `dir` - The direction vector (will be normalized internally)
    ///
    /// # Returns
    ///
    /// The point on the rounded shape's surface that is furthest in the given direction.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// # #[cfg(feature = "f32")]
    /// use parry3d::shape::{RoundShape, Cuboid, SupportMap};
    /// # #[cfg(feature = "f64")]
    /// use parry3d_f64::shape::{RoundShape, Cuboid, SupportMap};
    /// use parry3d::math::Vector;
    ///
    /// let cuboid = Cuboid::new(Vector::splat(1.0));
    /// let rounded = RoundShape {
    ///     inner_shape: cuboid,
    ///     border_radius: 0.5,
    /// };
    ///
    /// // Support point in the positive X direction
    /// let dir = Vector::new(1.0, 0.0, 0.0);
    /// let support = rounded.local_support_point(dir);
    ///
    /// // The X coordinate is the cuboid's half-extent plus the border radius
    /// assert!((support.x - 1.5).abs() < 1e-6);
    /// # }
    /// ```
    fn local_support_point(&self, dir: Vector) -> Vector {
        self.local_support_point_toward(dir.normalize())
    }

    /// Computes the support point of the rounded shape toward the given unit direction.
    ///
    /// This is similar to `local_support_point` but takes a pre-normalized direction vector,
    /// which can be more efficient when the direction is already normalized.
    ///
    /// The implementation adds the border radius offset to the inner shape's support point:
    /// `support_point = inner_support_point + direction * border_radius`
    ///
    /// # Parameters
    ///
    /// * `dir` - A unit-length direction vector
    ///
    /// # Returns
    ///
    /// The point on the rounded shape's surface that is furthest in the given direction.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::{RoundShape, Ball, SupportMap};
    /// use parry2d::math::Vector;
    ///
    /// let ball = Ball::new(1.0);
    /// let rounded = RoundShape {
    ///     inner_shape: ball,
    ///     border_radius: 0.3,
    /// };
    ///
    /// // Create a unit direction
    /// let dir = Vector::new(1.0, 1.0).normalize();
    /// let support = rounded.local_support_point_toward(dir);
    ///
    /// // The distance from origin should be ball radius + border radius
    /// let distance = (support.x.powi(2) + support.y.powi(2)).sqrt();
    /// assert!((distance - 1.3).abs() < 1e-6);
    /// # }
    /// ```
    fn local_support_point_toward(&self, dir: Vector) -> Vector {
        self.inner_shape.local_support_point_toward(dir) + dir * self.border_radius
    }
}

/// A shape reference with rounded borders.
///
/// This is an internal helper struct that provides the same rounding functionality as
/// `RoundShape`, but works with a borrowed reference to a shape instead of owning the shape.
/// This is useful in contexts where you want to temporarily treat a shape reference as
/// a rounded shape without creating a new allocation.
///
/// # Differences from `RoundShape`
///
/// - `RoundShapeRef` stores a reference (`&'a S`) instead of owning the shape
/// - It's marked `pub(crate)`, meaning it's only used internally within the Parry library
/// - The lifetime `'a` ensures the reference remains valid for the duration of use
/// - Works with unsized types (`S: ?Sized`), allowing it to work with trait objects
///
/// # Internal Use
///
/// This struct is primarily used internally by Parry for implementing collision detection
/// algorithms efficiently, where creating temporary rounded views of shapes is needed without
/// the overhead of cloning or moving data.
pub(crate) struct RoundShapeRef<'a, S: ?Sized> {
    /// The shape being rounded (borrowed reference).
    ///
    /// This is a reference to the inner shape, allowing the rounded view to be created
    /// without taking ownership or cloning the shape data.
    pub inner_shape: &'a S,

    /// The radius of the rounded border.
    ///
    /// Same semantics as in `RoundShape` - determines how much the shape is expanded outward.
    pub border_radius: Real,
}

impl<S: ?Sized + SupportMap> SupportMap for RoundShapeRef<'_, S> {
    /// Computes the support point of the rounded shape reference in the given direction.
    ///
    /// Behaves identically to `RoundShape::local_support_point`, but operates on a
    /// borrowed shape reference.
    ///
    /// # Parameters
    ///
    /// * `dir` - The direction vector (will be normalized internally)
    ///
    /// # Returns
    ///
    /// The point on the rounded shape's surface that is furthest in the given direction.
    fn local_support_point(&self, dir: Vector) -> Vector {
        self.local_support_point_toward(dir.normalize())
    }

    /// Computes the support point of the rounded shape reference toward the given unit direction.
    ///
    /// Behaves identically to `RoundShape::local_support_point_toward`, but operates on a
    /// borrowed shape reference. The implementation is the same: add the border radius offset
    /// to the inner shape's support point.
    ///
    /// # Parameters
    ///
    /// * `dir` - A unit-length direction vector
    ///
    /// # Returns
    ///
    /// The point on the rounded shape's surface that is furthest in the given direction.
    fn local_support_point_toward(&self, dir: Vector) -> Vector {
        self.inner_shape.local_support_point_toward(dir) + dir * self.border_radius
    }
}
