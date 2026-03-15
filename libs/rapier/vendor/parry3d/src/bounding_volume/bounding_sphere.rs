//! Bounding sphere.

use crate::bounding_volume::BoundingVolume;
use crate::math::{Pose, Real, Vector};

/// A Bounding Sphere.
///
/// A bounding sphere is a spherical bounding volume defined by a center point and a radius.
/// Unlike an AABB, a bounding sphere is rotation-invariant, meaning it doesn't need to be
/// recomputed when an object rotates.
///
/// # Structure
///
/// - **center**: The center point of the sphere
/// - **radius**: The distance from the center to any point on the sphere's surface
///
/// # Properties
///
/// - **Rotation-invariant**: Remains valid under rotation transformations
/// - **Simple**: Only 4 values (3D: x, y, z, radius; 2D: x, y, radius)
/// - **Conservative**: Often larger than the actual shape, especially for elongated objects
/// - **Fast intersection tests**: Only requires distance comparison
///
/// # Use Cases
///
/// Bounding spheres are useful for:
///
/// - **Rotating objects**: No recomputation needed when objects rotate
/// - **Broad-phase culling**: Quick rejection of distant object pairs
/// - **View frustum culling**: Simple sphere-frustum tests
/// - **Level of detail (LOD)**: Distance-based detail switching
/// - **Physics simulations**: Fast bounds checking for moving/rotating bodies
///
/// # Performance
///
/// - **Intersection test**: O(1) - Single distance comparison
/// - **Rotation**: O(1) - Only center needs transformation
/// - **Contains test**: O(1) - Distance plus radius comparison
///
/// # Comparison to AABB
///
/// **When to use BoundingSphere:**
/// - Objects rotate frequently
/// - Objects are roughly spherical or evenly distributed
/// - Memory is tight (fewer values to store)
/// - Rotation-invariant bounds are required
///
/// **When to use AABB:**
/// - Objects are axis-aligned or rarely rotate
/// - Objects are elongated or box-like
/// - Tighter bounds are critical
/// - Building spatial hierarchies (BVH, octree)
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::bounding_volume::BoundingSphere;
/// use parry3d::math::Vector;
///
/// // Create a bounding sphere with center at origin and radius 2.0
/// let sphere = BoundingSphere::new(Vector::ZERO, 2.0);
///
/// // Check basic properties
/// assert_eq!(sphere.center(), Vector::ZERO);
/// assert_eq!(sphere.radius(), 2.0);
///
/// // Test if a point is within the sphere
/// let point = Vector::new(1.0, 1.0, 0.0);
/// let distance = (point - sphere.center()).length();
/// assert!(distance <= sphere.radius());
/// # }
/// ```
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::bounding_volume::BoundingSphere;
/// use parry3d::math::Vector;
///
/// // Create a sphere and translate it
/// let sphere = BoundingSphere::new(Vector::new(1.0, 2.0, 3.0), 1.5);
/// let translation = Vector::new(5.0, 0.0, 0.0);
/// let moved = sphere.translated(translation);
///
/// assert_eq!(moved.center(), Vector::new(6.0, 2.0, 3.0));
/// assert_eq!(moved.radius(), 1.5); // Radius unchanged by translation
/// # }
/// ```
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::bounding_volume::{BoundingSphere, BoundingVolume};
/// use parry3d::math::Vector;
///
/// // Merge two bounding spheres
/// let sphere1 = BoundingSphere::new(Vector::ZERO, 1.0);
/// let sphere2 = BoundingSphere::new(Vector::new(4.0, 0.0, 0.0), 1.0);
///
/// let merged = sphere1.merged(&sphere2);
/// // The merged sphere contains both original spheres
/// assert!(merged.contains(&sphere1));
/// assert!(merged.contains(&sphere2));
/// # }
/// ```
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "bytemuck", derive(bytemuck::Pod, bytemuck::Zeroable))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(Debug, PartialEq, Copy, Clone)]
#[repr(C)]
pub struct BoundingSphere {
    /// The center point of the bounding sphere.
    pub center: Vector,

    /// The radius of the bounding sphere.
    ///
    /// This is the distance from the center to any point on the sphere's surface.
    /// All points within the bounded object should satisfy: distance(point, center) <= radius
    pub radius: Real,
}

impl BoundingSphere {
    /// Creates a new bounding sphere from a center point and radius.
    ///
    /// # Arguments
    ///
    /// * `center` - The center point of the sphere
    /// * `radius` - The radius of the sphere (must be non-negative)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::BoundingSphere;
    /// use parry3d::math::Vector;
    ///
    /// // Create a sphere centered at (1, 2, 3) with radius 5.0
    /// let sphere = BoundingSphere::new(
    ///     Vector::new(1.0, 2.0, 3.0),
    ///     5.0
    /// );
    ///
    /// assert_eq!(sphere.center(), Vector::new(1.0, 2.0, 3.0));
    /// assert_eq!(sphere.radius(), 5.0);
    /// # }
    /// ```
    pub fn new(center: Vector, radius: Real) -> BoundingSphere {
        BoundingSphere { center, radius }
    }

    /// Returns a reference to the center point of this bounding sphere.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::BoundingSphere;
    /// use parry3d::math::Vector;
    ///
    /// let sphere = BoundingSphere::new(Vector::new(1.0, 2.0, 3.0), 5.0);
    /// let center = sphere.center();
    ///
    /// assert_eq!(center, Vector::new(1.0, 2.0, 3.0));
    /// # }
    /// ```
    #[inline]
    pub fn center(&self) -> Vector {
        self.center
    }

    /// Returns the radius of this bounding sphere.
    ///
    /// The radius is the distance from the center to any point on the sphere's surface.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::BoundingSphere;
    /// use parry3d::math::Vector;
    ///
    /// let sphere = BoundingSphere::new(Vector::ZERO, 10.0);
    ///
    /// assert_eq!(sphere.radius(), 10.0);
    /// # }
    /// ```
    #[inline]
    pub fn radius(&self) -> Real {
        self.radius
    }

    /// Transforms this bounding sphere by the given isometry.
    ///
    /// For a bounding sphere, only the center point is affected by the transformation.
    /// The radius remains unchanged because spheres are rotation-invariant and isometries
    /// preserve distances.
    ///
    /// # Arguments
    ///
    /// * `m` - The isometry (rigid transformation) to apply
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::BoundingSphere;
    /// use parry3d::math::{Vector, Pose, Rotation};
    ///
    /// let sphere = BoundingSphere::new(Vector::new(1.0, 0.0, 0.0), 2.0);
    ///
    /// // Create a transformation: translate by (5, 0, 0) and rotate 90 degrees around Z
    /// let translation = Vector::new(5.0, 0.0, 0.0);
    /// let rotation = Rotation::from_rotation_z(std::f32::consts::FRAC_PI_2);
    /// let transform = Pose::from_parts(translation, rotation);
    ///
    /// let transformed = sphere.transform_by(&transform);
    ///
    /// // The center is transformed
    /// assert!((transformed.center() - Vector::new(5.0, 1.0, 0.0)).length() < 1e-5);
    /// // The radius is unchanged
    /// assert_eq!(transformed.radius(), 2.0);
    /// # }
    /// ```
    #[inline]
    pub fn transform_by(&self, m: &Pose) -> BoundingSphere {
        BoundingSphere::new(m * self.center, self.radius)
    }

    /// Translates this bounding sphere by the given vector.
    ///
    /// This is equivalent to `transform_by` with a pure translation, but more efficient
    /// as it doesn't involve rotation.
    ///
    /// # Arguments
    ///
    /// * `translation` - The translation vector to add to the center
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::BoundingSphere;
    /// use parry3d::math::Vector;
    ///
    /// let sphere = BoundingSphere::new(Vector::ZERO, 1.0);
    /// let translation = Vector::new(10.0, 5.0, -3.0);
    ///
    /// let moved = sphere.translated(translation);
    ///
    /// assert_eq!(moved.center(), Vector::new(10.0, 5.0, -3.0));
    /// assert_eq!(moved.radius(), 1.0); // Radius unchanged
    /// # }
    /// ```
    #[inline]
    pub fn translated(&self, translation: Vector) -> BoundingSphere {
        BoundingSphere::new(self.center + translation, self.radius)
    }
}

impl BoundingVolume for BoundingSphere {
    /// Returns the center point of this bounding sphere.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::{BoundingSphere, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let sphere = BoundingSphere::new(Vector::new(1.0, 2.0, 3.0), 5.0);
    ///
    /// // BoundingVolume::center() returns a Vector by value
    /// assert_eq!(BoundingVolume::center(&sphere), Vector::new(1.0, 2.0, 3.0));
    /// # }
    /// ```
    #[inline]
    fn center(&self) -> Vector {
        self.center()
    }

    /// Tests if this bounding sphere intersects another bounding sphere.
    ///
    /// Two spheres intersect if the distance between their centers is less than or equal
    /// to the sum of their radii.
    ///
    /// # Arguments
    ///
    /// * `other` - The other bounding sphere to test against
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::{BoundingSphere, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let sphere1 = BoundingSphere::new(Vector::ZERO, 2.0);
    /// let sphere2 = BoundingSphere::new(Vector::new(3.0, 0.0, 0.0), 2.0);
    /// let sphere3 = BoundingSphere::new(Vector::new(10.0, 0.0, 0.0), 1.0);
    ///
    /// assert!(sphere1.intersects(&sphere2)); // Distance 3.0 <= sum of radii 4.0
    /// assert!(!sphere1.intersects(&sphere3)); // Distance 10.0 > sum of radii 3.0
    /// # }
    /// ```
    #[inline]
    fn intersects(&self, other: &BoundingSphere) -> bool {
        // TODO: refactor that with the code from narrow_phase::ball_ball::collide(...) ?
        let delta_pos = other.center - self.center;
        let distance_squared = delta_pos.length_squared();
        let sum_radius = self.radius + other.radius;

        distance_squared <= sum_radius * sum_radius
    }

    /// Tests if this bounding sphere fully contains another bounding sphere.
    ///
    /// A sphere fully contains another sphere if the distance between their centers
    /// plus the other's radius is less than or equal to this sphere's radius.
    ///
    /// # Arguments
    ///
    /// * `other` - The other bounding sphere to test
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::{BoundingSphere, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let large = BoundingSphere::new(Vector::ZERO, 10.0);
    /// let small = BoundingSphere::new(Vector::new(2.0, 0.0, 0.0), 1.0);
    /// let outside = BoundingSphere::new(Vector::new(15.0, 0.0, 0.0), 2.0);
    ///
    /// assert!(large.contains(&small)); // Small sphere is inside large sphere
    /// assert!(!large.contains(&outside)); // Outside sphere extends beyond large sphere
    /// assert!(!small.contains(&large)); // Small cannot contain large
    /// # }
    /// ```
    #[inline]
    fn contains(&self, other: &BoundingSphere) -> bool {
        let delta_pos = other.center - self.center;
        let distance = delta_pos.length();

        distance + other.radius <= self.radius
    }

    /// Merges this bounding sphere with another in-place.
    ///
    /// After this operation, this sphere will be the smallest sphere that contains
    /// both the original sphere and the other sphere.
    ///
    /// # Arguments
    ///
    /// * `other` - The other bounding sphere to merge with
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::{BoundingSphere, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let mut sphere1 = BoundingSphere::new(Vector::ZERO, 1.0);
    /// let sphere2 = BoundingSphere::new(Vector::new(4.0, 0.0, 0.0), 1.0);
    ///
    /// sphere1.merge(&sphere2);
    ///
    /// // The merged sphere now contains both original spheres
    /// assert!(sphere1.contains(&BoundingSphere::new(Vector::ZERO, 1.0)));
    /// assert!(sphere1.contains(&sphere2));
    /// # }
    /// ```
    #[inline]
    fn merge(&mut self, other: &BoundingSphere) {
        let dir = other.center() - self.center();
        let (dir, length) = dir.normalize_and_length();

        if length == 0.0 {
            if other.radius > self.radius {
                self.radius = other.radius
            }
        } else {
            let s_center_dir = self.center.dot(dir);
            let o_center_dir = other.center.dot(dir);

            let right = if s_center_dir + self.radius > o_center_dir + other.radius {
                self.center + dir * self.radius
            } else {
                other.center + dir * other.radius
            };

            let left = if -s_center_dir + self.radius > -o_center_dir + other.radius {
                self.center - dir * self.radius
            } else {
                other.center - dir * other.radius
            };

            self.center = left.midpoint(right);
            self.radius = right.distance(self.center);
        }
    }

    /// Returns a new bounding sphere that is the merge of this sphere and another.
    ///
    /// The returned sphere is the smallest sphere that contains both input spheres.
    /// This is the non-mutating version of `merge`.
    ///
    /// # Arguments
    ///
    /// * `other` - The other bounding sphere to merge with
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::{BoundingSphere, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let sphere1 = BoundingSphere::new(Vector::ZERO, 1.0);
    /// let sphere2 = BoundingSphere::new(Vector::new(4.0, 0.0, 0.0), 1.0);
    ///
    /// let merged = sphere1.merged(&sphere2);
    ///
    /// // Original spheres are unchanged
    /// assert_eq!(sphere1.radius(), 1.0);
    /// // Merged sphere contains both
    /// assert!(merged.contains(&sphere1));
    /// assert!(merged.contains(&sphere2));
    /// # }
    /// ```
    #[inline]
    fn merged(&self, other: &BoundingSphere) -> BoundingSphere {
        let mut res = *self;

        res.merge(other);

        res
    }

    /// Increases the radius of this bounding sphere by the given amount in-place.
    ///
    /// This creates a larger sphere with the same center. Useful for adding safety margins
    /// or creating conservative bounds.
    ///
    /// # Arguments
    ///
    /// * `amount` - The amount to increase the radius (must be non-negative)
    ///
    /// # Panics
    ///
    /// Panics if `amount` is negative.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::{BoundingSphere, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let mut sphere = BoundingSphere::new(Vector::ZERO, 5.0);
    /// sphere.loosen(2.0);
    ///
    /// assert_eq!(sphere.radius(), 7.0);
    /// assert_eq!(sphere.center(), Vector::ZERO); // Center unchanged
    /// # }
    /// ```
    #[inline]
    fn loosen(&mut self, amount: Real) {
        assert!(amount >= 0.0, "The loosening margin must be positive.");
        self.radius += amount
    }

    /// Returns a new bounding sphere with increased radius.
    ///
    /// This is the non-mutating version of `loosen`. The returned sphere has the same
    /// center but a larger radius.
    ///
    /// # Arguments
    ///
    /// * `amount` - The amount to increase the radius (must be non-negative)
    ///
    /// # Panics
    ///
    /// Panics if `amount` is negative.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::{BoundingSphere, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let sphere = BoundingSphere::new(Vector::ZERO, 5.0);
    /// let larger = sphere.loosened(3.0);
    ///
    /// assert_eq!(sphere.radius(), 5.0); // Original unchanged
    /// assert_eq!(larger.radius(), 8.0);
    /// assert_eq!(larger.center(), Vector::ZERO);
    /// # }
    /// ```
    #[inline]
    fn loosened(&self, amount: Real) -> BoundingSphere {
        assert!(amount >= 0.0, "The loosening margin must be positive.");
        BoundingSphere::new(self.center, self.radius + amount)
    }

    /// Decreases the radius of this bounding sphere by the given amount in-place.
    ///
    /// This creates a smaller sphere with the same center. Useful for conservative
    /// collision detection or creating inner bounds.
    ///
    /// # Arguments
    ///
    /// * `amount` - The amount to decrease the radius (must be non-negative and ≤ radius)
    ///
    /// # Panics
    ///
    /// Panics if `amount` is negative or greater than the current radius.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::{BoundingSphere, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let mut sphere = BoundingSphere::new(Vector::ZERO, 10.0);
    /// sphere.tighten(3.0);
    ///
    /// assert_eq!(sphere.radius(), 7.0);
    /// assert_eq!(sphere.center(), Vector::ZERO); // Center unchanged
    /// # }
    /// ```
    #[inline]
    fn tighten(&mut self, amount: Real) {
        assert!(amount >= 0.0, "The tightening margin must be positive.");
        assert!(amount <= self.radius, "The tightening margin is to large.");
        self.radius -= amount
    }

    /// Returns a new bounding sphere with decreased radius.
    ///
    /// This is the non-mutating version of `tighten`. The returned sphere has the same
    /// center but a smaller radius.
    ///
    /// # Arguments
    ///
    /// * `amount` - The amount to decrease the radius (must be non-negative and ≤ radius)
    ///
    /// # Panics
    ///
    /// Panics if `amount` is negative or greater than the current radius.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::bounding_volume::{BoundingSphere, BoundingVolume};
    /// use parry3d::math::Vector;
    ///
    /// let sphere = BoundingSphere::new(Vector::ZERO, 10.0);
    /// let smaller = sphere.tightened(4.0);
    ///
    /// assert_eq!(sphere.radius(), 10.0); // Original unchanged
    /// assert_eq!(smaller.radius(), 6.0);
    /// assert_eq!(smaller.center(), Vector::ZERO);
    /// # }
    /// ```
    #[inline]
    fn tightened(&self, amount: Real) -> BoundingSphere {
        assert!(amount >= 0.0, "The tightening margin must be positive.");
        assert!(amount <= self.radius, "The tightening margin is to large.");
        BoundingSphere::new(self.center, self.radius - amount)
    }
}
