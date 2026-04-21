use crate::math::{Pose, Real, Vector};
use crate::shape::SupportMap;

/// A ball shape, also known as a sphere in 3D or a circle in 2D.
///
/// A ball is one of the simplest shapes in collision detection, defined by a single
/// parameter: its radius. The center of the ball is always at the origin of its local
/// coordinate system.
///
/// # Properties
///
/// - **In 2D**: Represents a circle (all points at distance `radius` from the center)
/// - **In 3D**: Represents a sphere (all points at distance `radius` from the center)
/// - **Convex**: Yes, balls are always convex shapes
/// - **Support mapping**: Extremely efficient (constant time)
///
/// # Use Cases
///
/// Balls are ideal for:
/// - Projectiles (bullets, cannonballs)
/// - Spherical objects (planets, marbles, balls)
/// - Bounding volumes for fast collision detection
/// - Dynamic objects that need to roll
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Ball;
/// use parry3d::math::Vector;
///
/// // Create a ball with radius 2.0
/// let ball = Ball::new(2.0);
/// assert_eq!(ball.radius, 2.0);
/// # }
/// ```
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "bytemuck", derive(bytemuck::Pod, bytemuck::Zeroable))]
#[cfg_attr(feature = "encase", derive(encase::ShaderType))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(C)]
pub struct Ball {
    /// The radius of the ball.
    ///
    /// This must be a positive value. A radius of 0.0 is valid but represents
    /// a degenerate ball (a single point).
    pub radius: Real,
}

impl Ball {
    /// Creates a new ball with the given radius.
    ///
    /// # Arguments
    ///
    /// * `radius` - The radius of the ball. Should be positive.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Ball;
    ///
    /// // Create a ball with radius 5.0
    /// let ball = Ball::new(5.0);
    /// assert_eq!(ball.radius, 5.0);
    ///
    /// // You can also create very small balls
    /// let tiny_ball = Ball::new(0.001);
    /// assert_eq!(tiny_ball.radius, 0.001);
    /// # }
    /// ```
    #[inline]
    pub fn new(radius: Real) -> Ball {
        Ball { radius }
    }

    /// Computes a scaled version of this ball.
    ///
    /// **Uniform scaling** (same scale factor on all axes) produces another ball.
    /// **Non-uniform scaling** (different scale factors) produces an ellipse, which
    /// is approximated as a convex polygon.
    ///
    /// # Arguments
    ///
    /// * `scale` - The scaling factors for each axis (x, y in 2D)
    /// * `nsubdivs` - Number of subdivisions for polygon approximation when scaling is non-uniform
    ///
    /// # Returns
    ///
    /// * `Some(Either::Left(Ball))` - If scaling is uniform, returns a scaled ball
    /// * `Some(Either::Right(ConvexPolygon))` - If scaling is non-uniform, returns a polygon approximation
    /// * `None` - If the approximation failed (e.g., zero scaling on an axis)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "alloc", feature = "f32"))] {
    /// use parry2d::shape::Ball;
    /// use parry2d::math::Vector;
    /// use either::Either;
    ///
    /// let ball = Ball::new(2.0);
    ///
    /// // Uniform scaling: produces another ball
    /// let uniform_scale = Vector::new(3.0, 3.0);
    /// if let Some(Either::Left(scaled_ball)) = ball.scaled(uniform_scale, 32) {
    ///     assert_eq!(scaled_ball.radius, 6.0); // 2.0 * 3.0
    /// }
    ///
    /// // Non-uniform scaling: produces a polygon (ellipse approximation)
    /// let non_uniform_scale = Vector::new(2.0, 1.0);
    /// if let Some(Either::Right(polygon)) = ball.scaled(non_uniform_scale, 32) {
    ///     // The polygon approximates an ellipse with radii 4.0 and 2.0
    ///     assert!(polygon.points().len() >= 32);
    /// }
    /// # }
    /// # #[cfg(all(feature = "dim2", feature = "alloc", feature = "f64"))] {
    /// use parry2d_f64::shape::Ball;
    /// use parry2d_f64::math::Vector;
    /// use either::Either;
    ///
    /// let ball = Ball::new(2.0);
    ///
    /// // Uniform scaling: produces another ball
    /// let uniform_scale = Vector::new(3.0, 3.0);
    /// if let Some(Either::Left(scaled_ball)) = ball.scaled(uniform_scale, 32) {
    ///     assert_eq!(scaled_ball.radius, 6.0); // 2.0 * 3.0
    /// }
    ///
    /// // Non-uniform scaling: produces a polygon (ellipse approximation)
    /// let non_uniform_scale = Vector::new(2.0, 1.0);
    /// if let Some(Either::Right(polygon)) = ball.scaled(non_uniform_scale, 32) {
    ///     // The polygon approximates an ellipse with radii 4.0 and 2.0
    ///     assert!(polygon.points().len() >= 32);
    /// }
    /// # }
    /// ```
    #[cfg(all(feature = "dim2", feature = "alloc"))]
    #[inline]
    pub fn scaled(
        self,
        scale: Vector,
        nsubdivs: u32,
    ) -> Option<either::Either<Self, super::ConvexPolygon>> {
        if scale.x != scale.y {
            // The scaled shape isn't a ball.
            let mut vtx = self.to_polyline(nsubdivs);
            vtx.iter_mut().for_each(|pt| *pt *= scale);
            Some(either::Either::Right(
                super::ConvexPolygon::from_convex_polyline(vtx)?,
            ))
        } else {
            let uniform_scale = scale.x;
            Some(either::Either::Left(Self::new(
                self.radius * uniform_scale.abs(),
            )))
        }
    }

    /// Computes a scaled version of this ball.
    ///
    /// **Uniform scaling** (same scale factor on all axes) produces another ball.
    /// **Non-uniform scaling** (different scale factors) produces an ellipsoid, which
    /// is approximated as a convex polyhedron.
    ///
    /// # Arguments
    ///
    /// * `scale` - The scaling factors for each axis (x, y, z in 3D)
    /// * `nsubdivs` - Number of subdivisions for polyhedron approximation when scaling is non-uniform
    ///
    /// # Returns
    ///
    /// * `Some(Either::Left(Ball))` - If scaling is uniform, returns a scaled ball
    /// * `Some(Either::Right(ConvexPolyhedron))` - If scaling is non-uniform, returns a polyhedron approximation
    /// * `None` - If the approximation failed (e.g., zero scaling on an axis)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32", feature = "alloc"))] {
    /// use parry3d::shape::Ball;
    /// use parry3d::math::Vector;
    /// use either::Either;
    ///
    /// let ball = Ball::new(5.0);
    ///
    /// // Uniform scaling: produces another ball
    /// let uniform_scale = Vector::new(2.0, 2.0, 2.0);
    /// if let Some(Either::Left(scaled_ball)) = ball.scaled(uniform_scale, 10) {
    ///     assert_eq!(scaled_ball.radius, 10.0); // 5.0 * 2.0
    /// }
    ///
    /// // Non-uniform scaling: produces a polyhedron (ellipsoid approximation)
    /// let non_uniform_scale = Vector::new(2.0, 1.0, 1.5);
    /// if let Some(Either::Right(polyhedron)) = ball.scaled(non_uniform_scale, 10) {
    ///     // The polyhedron approximates an ellipsoid
    ///     assert!(polyhedron.points().len() > 0);
    /// }
    /// # }
    /// ```
    #[cfg(all(feature = "dim3", feature = "alloc"))]
    #[inline]
    pub fn scaled(
        self,
        scale: Vector,
        nsubdivs: u32,
    ) -> Option<either::Either<Self, super::ConvexPolyhedron>> {
        if scale.x != scale.y || scale.x != scale.z || scale.y != scale.z {
            // The scaled shape isn't a ball.
            let (mut vtx, idx) = self.to_trimesh(nsubdivs, nsubdivs);
            vtx.iter_mut().for_each(|pt| *pt *= scale);
            Some(either::Either::Right(
                super::ConvexPolyhedron::from_convex_mesh(vtx, &idx)?,
            ))
        } else {
            let uniform_scale = scale.x;
            Some(either::Either::Left(Self::new(
                self.radius * uniform_scale.abs(),
            )))
        }
    }
}

impl SupportMap for Ball {
    #[inline]
    fn support_point(&self, m: &Pose, dir: Vector) -> Vector {
        self.support_point_toward(m, dir.normalize())
    }

    #[inline]
    fn support_point_toward(&self, m: &Pose, dir: Vector) -> Vector {
        m.translation + dir * self.radius
    }

    #[inline]
    fn local_support_point(&self, dir: Vector) -> Vector {
        self.local_support_point_toward(dir.normalize())
    }

    #[inline]
    fn local_support_point_toward(&self, dir: Vector) -> Vector {
        dir * self.radius
    }
}
