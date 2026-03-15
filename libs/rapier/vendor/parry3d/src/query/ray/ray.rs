//! Traits and structure needed to cast rays.

use crate::math::{Pose, Real, Vector};
use crate::shape::FeatureId;

#[cfg(feature = "alloc")]
use crate::partitioning::BvhLeafCost;

/// A ray for ray-casting queries.
///
/// A ray is a half-infinite line starting at an origin point and extending
/// infinitely in a direction. Rays are fundamental for visibility queries,
/// shooting mechanics, and collision prediction.
///
/// # Structure
///
/// - **origin**: The starting point of the ray
/// - **dir**: The direction vector (does NOT need to be normalized)
///
/// # Direction Vector
///
/// The direction can be any non-zero vector:
/// - **Normalized**: `dir` with length 1.0 gives time-of-impact in world units
/// - **Not normalized**: Time-of-impact is scaled by `dir.length()`
///
/// Most applications use normalized directions for intuitive results.
///
/// # Use Cases
///
/// - **Shooting/bullets**: Check what a projectile hits
/// - **Line of sight**: Check if one object can "see" another
/// - **Mouse picking**: Select objects by clicking
/// - **Laser beams**: Simulate light or laser paths
/// - **Proximity sensing**: Detect obstacles in a direction
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{Ray, RayCast};
/// use parry3d::shape::Ball;
/// use parry3d::math::{Vector, Pose};
///
/// // Create a ray from origin pointing along +X axis
/// let ray = Ray::new(
///     Vector::ZERO,
///     Vector::new(1.0, 0.0, 0.0)  // Normalized direction
/// );
///
/// // Create a ball at position (5, 0, 0) with radius 1
/// let ball = Ball::new(1.0);
/// let ball_pos = Pose::translation(5.0, 0.0, 0.0);
///
/// // Cast the ray against the ball
/// if let Some(toi) = ball.cast_ray(&ball_pos, &ray, 100.0, true) {
///     // Ray hits at t=4.0 (center at 5.0 minus radius 1.0)
///     assert_eq!(toi, 4.0);
///
///     // Compute the actual hit point
///     let hit_point = ray.point_at(toi);
///     assert_eq!(hit_point, Vector::new(4.0, 0.0, 0.0));
/// }
/// # }
/// ```
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[repr(C)]
pub struct Ray {
    /// Starting point of the ray.
    ///
    /// This is where the ray begins. Vectors along the ray are computed as
    /// `origin + dir * t` for `t ≥ 0`.
    pub origin: Vector,

    /// Direction vector of the ray.
    ///
    /// This vector points in the direction the ray travels. It does NOT need
    /// to be normalized, but using a normalized direction makes time-of-impact
    /// values represent actual distances.
    pub dir: Vector,
}

impl Ray {
    /// Creates a new ray from an origin point and direction vector.
    ///
    /// # Arguments
    ///
    /// * `origin` - The starting point of the ray
    /// * `dir` - The direction vector (typically normalized but not required)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::Ray;
    /// use parry3d::math::Vector;
    ///
    /// // Horizontal ray pointing along +X axis
    /// let ray = Ray::new(
    ///     Vector::new(0.0, 5.0, 0.0),
    ///     Vector::new(1.0, 0.0, 0.0)
    /// );
    ///
    /// // Ray starts at (0, 5, 0) and points along +X
    /// assert_eq!(ray.origin, Vector::new(0.0, 5.0, 0.0));
    /// assert_eq!(ray.dir, Vector::new(1.0, 0.0, 0.0));
    /// # }
    /// ```
    pub fn new(origin: Vector, dir: Vector) -> Ray {
        Ray { origin, dir }
    }

    /// Transforms this ray by the given isometry (translation + rotation).
    ///
    /// Both the origin and direction are transformed.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::Ray;
    /// use parry3d::math::{Pose, Vector};
    ///
    /// let ray = Ray::new(Vector::ZERO, Vector::X);
    ///
    /// // Translate by (5, 0, 0)
    /// let transform = Pose::translation(5.0, 0.0, 0.0);
    /// let transformed = ray.transform_by(&transform);
    ///
    /// assert_eq!(transformed.origin, Vector::new(5.0, 0.0, 0.0));
    /// assert_eq!(transformed.dir, Vector::X);
    /// # }
    /// ```
    #[inline]
    pub fn transform_by(&self, m: &Pose) -> Self {
        Self::new(m * self.origin, m.rotation * self.dir)
    }

    /// Transforms this ray by the inverse of the given isometry.
    ///
    /// This is equivalent to transforming the ray to the local space of an object.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::Ray;
    /// use parry3d::math::{Pose, Vector};
    ///
    /// let ray = Ray::new(Vector::new(10.0, 0.0, 0.0), Vector::X);
    ///
    /// let transform = Pose::translation(5.0, 0.0, 0.0);
    /// let local_ray = ray.inverse_transform_by(&transform);
    ///
    /// // Origin moved back by the translation
    /// assert_eq!(local_ray.origin, Vector::new(5.0, 0.0, 0.0));
    /// # }
    /// ```
    #[inline]
    pub fn inverse_transform_by(&self, m: &Pose) -> Self {
        Self::new(
            m.inverse_transform_point(self.origin),
            m.rotation.inverse() * self.dir,
        )
    }

    /// Translates this ray by the given vector.
    ///
    /// Only the origin is moved; the direction remains unchanged.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::Ray;
    /// use parry3d::math::Vector;
    ///
    /// let ray = Ray::new(Vector::ZERO, Vector::X);
    /// let translated = ray.translate_by(Vector::new(10.0, 5.0, 0.0));
    ///
    /// assert_eq!(translated.origin, Vector::new(10.0, 5.0, 0.0));
    /// assert_eq!(translated.dir, Vector::X); // Direction unchanged
    /// # }
    /// ```
    #[inline]
    pub fn translate_by(&self, v: Vector) -> Self {
        Self::new(self.origin + v, self.dir)
    }

    /// Computes a point along the ray at parameter `t`.
    ///
    /// Returns `origin + dir * t`. For `t ≥ 0`, this gives points along the ray.
    ///
    /// # Arguments
    ///
    /// * `t` - The parameter (typically the time-of-impact from ray casting)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::Ray;
    /// use parry3d::math::Vector;
    ///
    /// let ray = Ray::new(
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0)
    /// );
    ///
    /// // Vector at t=5.0
    /// assert_eq!(ray.point_at(5.0), Vector::new(5.0, 0.0, 0.0));
    ///
    /// // Vector at t=0.0 is the origin
    /// assert_eq!(ray.point_at(0.0), ray.origin);
    /// # }
    /// ```
    #[inline]
    pub fn point_at(&self, t: Real) -> Vector {
        self.origin + self.dir * t
    }
}

/// Result of a successful ray cast against a shape.
///
/// This structure contains all information about where and how a ray intersects
/// a shape, including the time of impact, surface normal, and geometric feature hit.
///
/// # Fields
///
/// - **time_of_impact**: The `t` parameter where the ray hits (use with `ray.point_at(t)`)
/// - **normal**: The surface normal at the hit point
/// - **feature**: Which geometric feature was hit (vertex, edge, or face)
///
/// # Time of Impact
///
/// The time of impact is the parameter `t` in the ray equation `origin + dir * t`:
/// - If `dir` is normalized: `t` represents the distance traveled
/// - If `dir` is not normalized: `t` represents time (distance / speed)
///
/// # Normal Direction
///
/// The normal behavior depends on the ray origin and `solid` parameter:
/// - **Outside solid shape**: Normal points outward from the surface
/// - **Inside non-solid shape**: Normal points inward (toward the interior)
/// - **At t=0.0**: Normal may be unreliable due to numerical precision
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{Ray, RayCast};
/// use parry3d::shape::Cuboid;
/// use parry3d::math::{Vector, Pose};
///
/// let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
/// let ray = Ray::new(
///     Vector::new(-5.0, 0.0, 0.0),
///     Vector::new(1.0, 0.0, 0.0)
/// );
///
/// if let Some(intersection) = cuboid.cast_ray_and_get_normal(
///     &Pose::identity(),
///     &ray,
///     100.0,
///     true
/// ) {
///     // Ray hits the -X face of the cuboid at x=-1.0
///     assert_eq!(intersection.time_of_impact, 4.0); // Distance from -5 to -1
///
///     // Hit point is at (-1, 0, 0) - the surface of the cuboid
///     let hit_point = ray.point_at(intersection.time_of_impact);
///     assert_eq!(hit_point.x, -1.0);
///
///     // Normal points outward (in -X direction for the -X face)
///     assert_eq!(intersection.normal, Vector::new(-1.0, 0.0, 0.0));
/// }
/// # }
/// ```
#[derive(Copy, Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
pub struct RayIntersection {
    /// The time of impact (parameter `t`) where the ray hits the shape.
    ///
    /// The exact hit point can be computed with `ray.point_at(time_of_impact)`.
    /// If the ray direction is normalized, this represents the distance traveled.
    pub time_of_impact: Real,

    /// The surface normal at the intersection point.
    ///
    /// - Typically points outward from the shape
    /// - May point inward if ray origin is inside a non-solid shape
    /// - May be unreliable if `time_of_impact` is exactly zero
    ///
    /// Note: This should be a unit vector but is not enforced by the type system yet.
    // TODO: use a Vector instead.
    pub normal: Vector,

    /// The geometric feature (vertex, edge, or face) that was hit.
    ///
    /// This can be used for more detailed collision response or to identify
    /// exactly which part of the shape was struck.
    pub feature: FeatureId,
}

impl RayIntersection {
    #[inline]
    /// Creates a new `RayIntersection`.
    #[cfg(feature = "dim3")]
    pub fn new(time_of_impact: Real, normal: Vector, feature: FeatureId) -> RayIntersection {
        RayIntersection {
            time_of_impact,
            normal,
            feature,
        }
    }

    #[inline]
    /// Creates a new `RayIntersection`.
    #[cfg(feature = "dim2")]
    pub fn new(time_of_impact: Real, normal: Vector, feature: FeatureId) -> RayIntersection {
        RayIntersection {
            time_of_impact,
            normal,
            feature,
        }
    }

    #[inline]
    pub fn transform_by(&self, transform: &Pose) -> Self {
        RayIntersection {
            time_of_impact: self.time_of_impact,
            normal: transform.rotation * self.normal,
            feature: self.feature,
        }
    }
}

#[cfg(feature = "alloc")]
impl BvhLeafCost for RayIntersection {
    #[inline]
    fn cost(&self) -> Real {
        self.time_of_impact
    }
}

/// Traits of objects which can be transformed and tested for intersection with a ray.
pub trait RayCast {
    /// Computes the time of impact between this transform shape and a ray.
    fn cast_local_ray(&self, ray: &Ray, max_time_of_impact: Real, solid: bool) -> Option<Real> {
        self.cast_local_ray_and_get_normal(ray, max_time_of_impact, solid)
            .map(|inter| inter.time_of_impact)
    }

    /// Computes the time of impact, and normal between this transformed shape and a ray.
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection>;

    /// Tests whether a ray intersects this transformed shape.
    #[inline]
    fn intersects_local_ray(&self, ray: &Ray, max_time_of_impact: Real) -> bool {
        self.cast_local_ray(ray, max_time_of_impact, true).is_some()
    }

    /// Computes the time of impact between this transform shape and a ray.
    fn cast_ray(&self, m: &Pose, ray: &Ray, max_time_of_impact: Real, solid: bool) -> Option<Real> {
        let ls_ray = ray.inverse_transform_by(m);
        self.cast_local_ray(&ls_ray, max_time_of_impact, solid)
    }

    /// Computes the time of impact, and normal between this transformed shape and a ray.
    fn cast_ray_and_get_normal(
        &self,
        m: &Pose,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        let ls_ray = ray.inverse_transform_by(m);
        self.cast_local_ray_and_get_normal(&ls_ray, max_time_of_impact, solid)
            .map(|inter| inter.transform_by(m))
    }

    /// Tests whether a ray intersects this transformed shape.
    #[inline]
    fn intersects_ray(&self, m: &Pose, ray: &Ray, max_time_of_impact: Real) -> bool {
        let ls_ray = ray.inverse_transform_by(m);
        self.intersects_local_ray(&ls_ray, max_time_of_impact)
    }
}
