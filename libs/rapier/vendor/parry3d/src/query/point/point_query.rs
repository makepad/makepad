use crate::math::{Pose, Real, Vector};
use crate::shape::FeatureId;

/// The result of projecting a point onto a shape.
///
/// Vector projection finds the closest point on a shape's surface to a given query point.
/// This is fundamental for many geometric queries including distance calculation,
/// collision detection, and surface sampling.
///
/// # Fields
///
/// - **is_inside**: Whether the query point is inside the shape
/// - **point**: The closest point on the shape's surface
///
/// # Inside vs Outside
///
/// The `is_inside` flag indicates the query point's location relative to the shape:
/// - `true`: Query point is inside the shape (projection is on boundary)
/// - `false`: Query point is outside the shape (projection is nearest surface point)
///
/// # Solid Parameter
///
/// Most projection functions take a `solid` parameter:
/// - `solid = true`: Shape is treated as solid (filled interior)
/// - `solid = false`: Shape is treated as hollow (surface only)
///
/// This affects `is_inside` calculation for points in the interior.
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::PointQuery;
/// use parry3d::shape::Ball;
/// use parry3d::math::{Vector, Pose};
///
/// let ball = Ball::new(5.0);
/// let ball_pos = Pose::translation(10.0, 0.0, 0.0);
///
/// // Project a point outside the ball
/// let outside_point = Vector::ZERO;
/// let proj = ball.project_point(&ball_pos, outside_point, true);
///
/// // Closest point on ball surface
/// assert_eq!(proj.point, Vector::new(5.0, 0.0, 0.0));
/// assert!(!proj.is_inside);
///
/// // Project a point inside the ball
/// let inside_point = Vector::new(10.0, 0.0, 0.0); // At center
/// let proj2 = ball.project_point(&ball_pos, inside_point, true);
/// assert!(proj2.is_inside);
/// # }
/// ```
#[derive(Copy, Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
pub struct PointProjection {
    /// Whether the query point was inside the shape.
    ///
    /// - `true`: Vector is in the interior (for solid shapes)
    /// - `false`: Vector is outside the shape
    pub is_inside: bool,

    /// The closest point on the shape's surface to the query point.
    ///
    /// If `is_inside = true`, this is the nearest point on the boundary.
    /// If `is_inside = false`, this is the nearest surface point.
    pub point: Vector,
}

impl PointProjection {
    /// Initializes a new `PointProjection`.
    pub fn new(is_inside: bool, point: Vector) -> Self {
        PointProjection { is_inside, point }
    }

    /// Transforms `self.point` by `pos`.
    pub fn transform_by(&self, pos: &Pose) -> Self {
        PointProjection {
            is_inside: self.is_inside,
            point: pos * self.point,
        }
    }

    /// Returns `true` if `Self::is_inside` is `true` or if the distance between the projected point and `point` is smaller than `min_dist`.
    pub fn is_inside_eps(&self, original_point: Vector, min_dist: Real) -> bool {
        self.is_inside || (original_point - self.point).length_squared() < min_dist * min_dist
    }
}

/// Trait for shapes that support point projection and containment queries.
///
/// This trait provides methods to:
/// - Project points onto the shape's surface
/// - Calculate distance from a point to the shape
/// - Test if a point is inside the shape
///
/// All major shapes implement this trait, making it easy to perform point queries
/// on any collision shape.
///
/// # Methods Overview
///
/// - **Projection**: `project_point()`, `project_local_point()`
/// - **Distance**: `distance_to_point()`, `distance_to_local_point()`
/// - **Containment**: `contains_point()`, `contains_local_point()`
///
/// # Local vs World Space
///
/// Methods with `local_` prefix work in the shape's local coordinate system:
/// - **Local methods**: Vector must be in shape's coordinate frame
/// - **World methods**: Vector in world space, shape transformation applied
///
/// # Solid Parameter
///
/// The `solid` parameter affects how interior points are handled:
/// - `solid = true`: Shape is filled (has interior volume/area)
/// - `solid = false`: Shape is hollow (surface only, no interior)
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::PointQuery;
/// use parry3d::shape::Cuboid;
/// use parry3d::math::{Vector, Pose};
///
/// let cuboid = Cuboid::new(Vector::splat(1.0));
/// let cuboid_pos = Pose::translation(5.0, 0.0, 0.0);
///
/// let query_point = Vector::ZERO;
///
/// // Project point onto cuboid surface
/// let projection = cuboid.project_point(&cuboid_pos, query_point, true);
/// println!("Closest point on cuboid: {:?}", projection.point);
/// println!("Is inside: {}", projection.is_inside);
///
/// // Calculate distance to cuboid
/// let distance = cuboid.distance_to_point(&cuboid_pos, query_point, true);
/// println!("Distance: {}", distance);
///
/// // Test if point is inside cuboid
/// let is_inside = cuboid.contains_point(&cuboid_pos, query_point);
/// println!("Contains: {}", is_inside);
/// # }
/// ```
pub trait PointQuery {
    /// Projects a point onto the shape, with a maximum distance limit.
    ///
    /// Returns `None` if the projection would be further than `max_dist` from the query point.
    /// This is useful for optimization when you only care about nearby projections.
    ///
    /// The point is in the shape's local coordinate system.
    ///
    /// # Arguments
    ///
    /// * `pt` - The point to project (in local space)
    /// * `solid` - Whether to treat the shape as solid (filled)
    /// * `max_dist` - Maximum distance to consider
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::PointQuery;
    /// use parry3d::shape::Ball;
    /// use parry3d::math::Vector;
    ///
    /// let ball = Ball::new(1.0);
    /// let far_point = Vector::new(100.0, 0.0, 0.0);
    ///
    /// // Projection exists but is far away
    /// assert!(ball.project_local_point(far_point, true).point.x > 0.0);
    ///
    /// // With max distance limit of 10, projection is rejected
    /// assert!(ball.project_local_point_with_max_dist(far_point, true, 10.0).is_none());
    ///
    /// // Nearby point is accepted
    /// let near_point = Vector::new(2.0, 0.0, 0.0);
    /// assert!(ball.project_local_point_with_max_dist(near_point, true, 10.0).is_some());
    /// # }
    /// ```
    fn project_local_point_with_max_dist(
        &self,
        pt: Vector,
        solid: bool,
        max_dist: Real,
    ) -> Option<PointProjection> {
        let proj = self.project_local_point(pt, solid);
        if (proj.point - pt).length() > max_dist {
            None
        } else {
            Some(proj)
        }
    }

    /// Projects a point on `self` transformed by `m`, unless the projection lies further than the given max distance.
    fn project_point_with_max_dist(
        &self,
        m: &Pose,
        pt: Vector,
        solid: bool,
        max_dist: Real,
    ) -> Option<PointProjection> {
        self.project_local_point_with_max_dist(m.inverse_transform_point(pt), solid, max_dist)
            .map(|proj| proj.transform_by(m))
    }

    /// Projects a point on `self`.
    ///
    /// The point is assumed to be expressed in the local-space of `self`.
    fn project_local_point(&self, pt: Vector, solid: bool) -> PointProjection;

    /// Projects a point on the boundary of `self` and returns the id of the
    /// feature the point was projected on.
    fn project_local_point_and_get_feature(&self, pt: Vector) -> (PointProjection, FeatureId);

    /// Computes the minimal distance between a point and `self`.
    fn distance_to_local_point(&self, pt: Vector, solid: bool) -> Real {
        let proj = self.project_local_point(pt, solid);
        let dist = (pt - proj.point).length();

        if solid || !proj.is_inside {
            dist
        } else {
            -dist
        }
    }

    /// Tests if the given point is inside of `self`.
    fn contains_local_point(&self, pt: Vector) -> bool {
        self.project_local_point(pt, true).is_inside
    }

    /// Projects a point on `self` transformed by `m`.
    fn project_point(&self, m: &Pose, pt: Vector, solid: bool) -> PointProjection {
        self.project_local_point(m.inverse_transform_point(pt), solid)
            .transform_by(m)
    }

    /// Computes the minimal distance between a point and `self` transformed by `m`.
    #[inline]
    fn distance_to_point(&self, m: &Pose, pt: Vector, solid: bool) -> Real {
        self.distance_to_local_point(m.inverse_transform_point(pt), solid)
    }

    /// Projects a point on the boundary of `self` transformed by `m` and returns the id of the
    /// feature the point was projected on.
    fn project_point_and_get_feature(&self, m: &Pose, pt: Vector) -> (PointProjection, FeatureId) {
        let res = self.project_local_point_and_get_feature(m.inverse_transform_point(pt));
        (res.0.transform_by(m), res.1)
    }

    /// Tests if the given point is inside of `self` transformed by `m`.
    #[inline]
    fn contains_point(&self, m: &Pose, pt: Vector) -> bool {
        self.contains_local_point(m.inverse_transform_point(pt))
    }
}

/// Returns shape-specific info in addition to generic projection information
///
/// One requirement for the `PointQuery` trait is to be usable as a trait
/// object. Unfortunately this precludes us from adding an associated type to it
/// that might allow us to return shape-specific information in addition to the
/// general information provided in `PointProjection`. This is where
/// `PointQueryWithLocation` comes in. It forgoes the ability to be used as a trait
/// object in exchange for being able to provide shape-specific projection
/// information.
///
/// Any shapes that implement `PointQuery` but are able to provide extra
/// information, can implement `PointQueryWithLocation` in addition and have their
/// `PointQuery::project_point` implementation just call out to
/// `PointQueryWithLocation::project_point_and_get_location`.
pub trait PointQueryWithLocation {
    /// Additional shape-specific projection information
    ///
    /// In addition to the generic projection information returned in
    /// `PointProjection`, implementations might provide shape-specific
    /// projection info. The type of this shape-specific information is defined
    /// by this associated type.
    type Location;

    /// Projects a point on `self`.
    fn project_local_point_and_get_location(
        &self,
        pt: Vector,
        solid: bool,
    ) -> (PointProjection, Self::Location);

    /// Projects a point on `self` transformed by `m`.
    fn project_point_and_get_location(
        &self,
        m: &Pose,
        pt: Vector,
        solid: bool,
    ) -> (PointProjection, Self::Location) {
        let res = self.project_local_point_and_get_location(m.inverse_transform_point(pt), solid);
        (res.0.transform_by(m), res.1)
    }

    /// Projects a point on `self`, with a maximum projection distance.
    fn project_local_point_and_get_location_with_max_dist(
        &self,
        pt: Vector,
        solid: bool,
        max_dist: Real,
    ) -> Option<(PointProjection, Self::Location)> {
        let (proj, location) = self.project_local_point_and_get_location(pt, solid);
        if (proj.point - pt).length() > max_dist {
            None
        } else {
            Some((proj, location))
        }
    }

    /// Projects a point on `self` transformed by `m`, with a maximum projection distance.
    fn project_point_and_get_location_with_max_dist(
        &self,
        m: &Pose,
        pt: Vector,
        solid: bool,
        max_dist: Real,
    ) -> Option<(PointProjection, Self::Location)> {
        self.project_local_point_and_get_location_with_max_dist(
            m.inverse_transform_point(pt),
            solid,
            max_dist,
        )
        .map(|res| (res.0.transform_by(m), res.1))
    }
}
