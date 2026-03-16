//! Support mapping based HalfSpace shape.
use crate::math::Vector;

/// A half-space delimited by an infinite plane.
///
/// # What is a HalfSpace?
///
/// A half-space represents an infinite region of space on one side of a plane. It divides
/// space into two regions:
/// - The "inside" region (where the normal vector points away from)
/// - The "outside" region (where the normal vector points toward)
///
/// The plane itself passes through the origin of the shape's coordinate system and is defined
/// by its outward normal vector. All points in the direction opposite to the normal are
/// considered "inside" the half-space.
///
/// # When to Use HalfSpace
///
/// Half-spaces are useful for representing:
/// - **Ground planes**: A flat, infinite floor for collision detection
/// - **Walls**: Infinite vertical barriers
/// - **Bounding regions**: Constraining objects to one side of a plane
/// - **Clipping planes**: Cutting off geometry in one direction
///
/// Because half-spaces are infinite, they are very efficient for collision detection and
/// don't require complex shape representations.
///
/// # Coordinate System
///
/// The plane always passes through the origin `(0, 0)` in 2D or `(0, 0, 0)` in 3D of the
/// half-space's local coordinate system. To position the plane elsewhere in your world,
/// use a [`Pose`](crate::math::Pose) transformation when performing queries.
///
/// # Examples
///
/// ## Creating a Ground Plane (3D)
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::HalfSpace;
/// use parry3d::math::{Vector};
///
/// // Create a horizontal ground plane with normal pointing up (positive Y-axis)
/// let ground = HalfSpace::new((Vector::Y.normalize()));
///
/// // The ground plane is at Y = 0 in local coordinates
/// // Everything below (negative Y) is "inside" the half-space
/// # }
/// ```
///
/// ## Vertical Wall (2D)
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::shape::HalfSpace;
/// use parry2d::math::Vector;
///
/// // Create a vertical wall with normal pointing right (positive X-axis)
/// let wall = HalfSpace::new(Vector::X.normalize());
///
/// // The wall is at X = 0 in local coordinates
/// // Everything to the left (negative X) is "inside" the half-space
/// # }
/// ```
///
/// ## Collision Detection with a Ball (3D)
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{HalfSpace, Ball};
/// use parry3d::query;
/// use parry3d::math::{Pose, Vector};
///
/// // Create a ground plane at Y = 0, normal pointing up
/// let ground = HalfSpace::new((Vector::Y.normalize()));
/// let ground_pos = Pose::identity();
///
/// // Create a ball with radius 1.0 at position (0, 0.5, 0)
/// // The ball is resting on the ground, just touching it
/// let ball = Ball::new(1.0);
/// let ball_pos = Pose::translation(0.0, 0.5, 0.0);
///
/// // Check if they're in contact (with a small prediction distance)
/// let contact = query::contact(
///     &ground_pos,
///     &ground,
///     &ball_pos,
///     &ball,
///     0.1
/// );
///
/// assert!(contact.unwrap().is_some());
/// # }
/// ```
///
/// ## Positioned Ground Plane (3D)
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::HalfSpace;
/// use parry3d::query::{PointQuery};
/// use parry3d::math::{Pose, Vector};
///
/// // Create a ground plane with normal pointing up
/// let ground = HalfSpace::new((Vector::Y.normalize()));
///
/// // Position the plane at Y = 5.0 using an isometry
/// let ground_pos = Pose::translation(0.0, 5.0, 0.0);
///
/// // Check if a point is below the ground (inside the half-space)
/// let point = Vector::new(0.0, 3.0, 0.0); // Vector at Y = 3.0 (below the plane)
///
/// // Project the point onto the ground plane
/// let proj = ground.project_point(&ground_pos, point, true);
///
/// // The point is below the ground (inside the half-space)
/// assert!(proj.is_inside);
/// # }
/// ```
///
/// ## Tilted Plane (3D)
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::HalfSpace;
/// use parry3d::math::{Vector};
///
/// // Create a plane tilted at 45 degrees
/// // Normal points up and to the right
/// let normal = Vector::new(1.0, 1.0, 0.0);
/// let tilted_plane = HalfSpace::new((normal).normalize());
///
/// // This plane passes through the origin and divides space diagonally
/// # }
/// ```
#[derive(PartialEq, Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
pub struct HalfSpace {
    /// The halfspace planar boundary's outward normal.
    ///
    /// This unit vector points in the direction considered "outside" the half-space.
    /// All points in the direction opposite to this normal (when measured from the
    /// plane at the origin) are considered "inside" the half-space.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::HalfSpace;
    /// use parry3d::math::{Vector};
    ///
    /// let ground = HalfSpace::new(Vector::Y.normalize());
    ///
    /// // The normal points up (positive Y direction)
    /// assert_eq!(ground.normal, Vector::Y);
    /// # }
    /// ```
    pub normal: Vector,
}

impl HalfSpace {
    /// Builds a new half-space from its outward normal vector.
    ///
    /// The plane defining the half-space passes through the origin of the local coordinate
    /// system and is perpendicular to the given normal vector. The normal points toward
    /// the "outside" region, while the opposite direction is considered "inside."
    ///
    /// # Parameters
    ///
    /// * `normal` - A unit vector defining the plane's outward normal direction. This must
    ///   be a normalized vector (use `.normalize()` on any vector to create one).
    ///
    /// # Examples
    ///
    /// ## Creating a Horizontal Ground Plane (3D)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::HalfSpace;
    /// use parry3d::math::{Vector};
    ///
    /// // Ground plane with normal pointing up
    /// let ground = HalfSpace::new((Vector::Y.normalize()));
    /// # }
    /// ```
    ///
    /// ## Creating a Vertical Wall (2D)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::HalfSpace;
    /// use parry2d::math::Vector;
    ///
    /// // Wall with normal pointing to the right
    /// let wall = HalfSpace::new(Vector::X.normalize());
    /// # }
    /// ```
    ///
    /// ## Custom Normal Direction (3D)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::HalfSpace;
    /// use parry3d::math::{Vector};
    ///
    /// // Plane with normal at 45-degree angle
    /// let custom_normal = Vector::new(1.0, 1.0, 0.0);
    /// let plane = HalfSpace::new((custom_normal).normalize());
    ///
    /// // Verify the normal is normalized
    /// assert!((plane.normal.length() - 1.0).abs() < 1e-5);
    /// # }
    /// ```
    #[inline]
    pub fn new(normal: Vector) -> HalfSpace {
        HalfSpace { normal }
    }

    /// Computes a scaled version of this half-space.
    ///
    /// Scaling a half-space applies non-uniform scaling to its normal vector. This is useful
    /// when transforming shapes in a scaled coordinate system. The resulting normal is
    /// re-normalized to maintain the half-space's validity.
    ///
    /// # Parameters
    ///
    /// * `scale` - A vector containing the scaling factors for each axis. For example,
    ///   `Vector::new(2.0, 1.0, 1.0)` doubles the X-axis scaling.
    ///
    /// # Returns
    ///
    /// * `Some(HalfSpace)` - The scaled half-space with the transformed normal
    /// * `None` - If the scaled normal becomes zero (degenerate case), meaning the
    ///   half-space cannot be represented after scaling
    ///
    /// # When This Returns None
    ///
    /// The method returns `None` when any component of the normal becomes zero after
    /// scaling AND that component was the only non-zero component. For example:
    /// - A horizontal plane (normal = `[0, 1, 0]`) scaled by `[1, 0, 1]` → `None`
    /// - A diagonal plane (normal = `[0.7, 0.7, 0]`) scaled by `[1, 0, 1]` → `Some(...)`
    ///
    /// # Examples
    ///
    /// ## Uniform Scaling (3D)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::HalfSpace;
    /// use parry3d::math::{Vector};
    ///
    /// let ground = HalfSpace::new(Vector::Y.normalize());
    ///
    /// // Uniform scaling doesn't change the normal direction
    /// let scaled = ground.scaled(Vector::splat(2.0)).unwrap();
    /// assert_eq!(scaled.normal, Vector::Y);
    /// # }
    /// ```
    ///
    /// ## Non-Uniform Scaling (3D)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::HalfSpace;
    /// use parry3d::math::{Vector};
    ///
    /// // Diagonal plane
    /// let plane = HalfSpace::new(
    ///     (Vector::new(1.0, 1.0, 0.0).normalize())
    /// );
    ///
    /// // Scale X-axis by 2.0, Y-axis stays 1.0
    /// let scaled = plane.scaled(Vector::new(2.0, 1.0, 1.0)).unwrap();
    ///
    /// // The normal changes direction due to non-uniform scaling
    /// // It's no longer at 45 degrees
    /// assert!(scaled.normal.x != scaled.normal.y);
    /// # }
    /// ```
    ///
    /// ## Degenerate Case (3D)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::HalfSpace;
    /// use parry3d::math::{Vector};
    ///
    /// // Horizontal ground plane
    /// let ground = HalfSpace::new((Vector::Y.normalize()));
    ///
    /// // Scaling Y to zero makes the normal degenerate
    /// let scaled = ground.scaled(Vector::new(1.0, 0.0, 1.0));
    /// assert!(scaled.is_none()); // Returns None because normal becomes zero
    /// # }
    /// ```
    ///
    /// ## Practical Use Case (2D)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::HalfSpace;
    /// use parry2d::math::Vector;
    ///
    /// // Create a wall in a 2D platformer
    /// let wall = HalfSpace::new(Vector::X.normalize());
    ///
    /// // Apply level scaling (e.g., for pixel-perfect rendering)
    /// let pixel_scale = Vector::new(16.0, 16.0);
    /// if let Some(scaled_wall) = wall.scaled(pixel_scale) {
    ///     // Use the scaled wall for collision detection
    /// }
    /// # }
    /// ```
    pub fn scaled(self, scale: Vector) -> Option<Self> {
        let scaled = self.normal * scale;
        scaled.try_normalize().map(|normal| Self { normal })
    }
}
