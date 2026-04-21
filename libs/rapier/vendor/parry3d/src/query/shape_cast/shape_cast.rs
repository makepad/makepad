use crate::math::{Pose, Real, Vector};
use crate::query::{DefaultQueryDispatcher, QueryDispatcher, Unsupported};
use crate::shape::Shape;

#[cfg(feature = "alloc")]
use crate::partitioning::BvhLeafCost;

/// The status of the time-of-impact computation algorithm.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ShapeCastStatus {
    /// The shape-casting algorithm ran out of iterations before achieving convergence.
    ///
    /// The content of the `ShapeCastHit` will still be a conservative approximation of the actual result so
    /// it is often fine to interpret this case as a success.
    OutOfIterations,
    /// The shape-casting algorithm converged successfully.
    Converged,
    /// Something went wrong during the shape-casting, likely due to numerical instabilities.
    ///
    /// The content of the `ShapeCastHit` will still be a conservative approximation of the actual result so
    /// it is often fine to interpret this case as a success.
    Failed,
    /// The two shape already overlap, or are separated by a distance smaller than
    /// [`ShapeCastOptions::target_distance`] at the time 0.
    ///
    /// The witness points and normals provided by the `ShapeCastHit` will have unreliable values unless
    /// [`ShapeCastOptions::compute_impact_geometry_on_penetration`] was set to `true` when calling
    /// the time-of-impact function.
    PenetratingOrWithinTargetDist,
}

/// The result of a shape casting..
#[derive(Copy, Clone, Debug)]
pub struct ShapeCastHit {
    /// The time at which the objects touch.
    pub time_of_impact: Real,
    /// The local-space closest point on the first shape at the time of impact.
    ///
    /// This value is unreliable if `status` is [`ShapeCastStatus::PenetratingOrWithinTargetDist`]
    /// and [`ShapeCastOptions::compute_impact_geometry_on_penetration`] was set to `false`.
    pub witness1: Vector,
    /// The local-space closest point on the second shape at the time of impact.
    ///
    /// This value is unreliable if `status` is [`ShapeCastStatus::PenetratingOrWithinTargetDist`]
    /// and both [`ShapeCastOptions::compute_impact_geometry_on_penetration`] was set to `false`
    /// when calling the time-of-impact function.
    pub witness2: Vector,
    /// The local-space outward normal on the first shape at the time of impact.
    ///
    /// This value is unreliable if `status` is [`ShapeCastStatus::PenetratingOrWithinTargetDist`]
    /// and both [`ShapeCastOptions::compute_impact_geometry_on_penetration`] was set to `false`
    /// when calling the time-of-impact function.
    pub normal1: Vector,
    /// The local-space outward normal on the second shape at the time of impact.
    ///
    /// This value is unreliable if `status` is [`ShapeCastStatus::PenetratingOrWithinTargetDist`]
    /// and both [`ShapeCastOptions::compute_impact_geometry_on_penetration`] was set to `false`
    /// when calling the time-of-impact function.
    pub normal2: Vector,
    /// The way the shape-casting algorithm terminated.
    pub status: ShapeCastStatus,
}

impl ShapeCastHit {
    /// Swaps every data of this shape-casting result such that the role of both shapes are swapped.
    ///
    /// In practice, this makes it so that `self.witness1` and `self.normal1` are swapped with
    /// `self.witness2` and `self.normal2`.
    pub fn swapped(self) -> Self {
        Self {
            time_of_impact: self.time_of_impact,
            witness1: self.witness2,
            witness2: self.witness1,
            normal1: self.normal2,
            normal2: self.normal1,
            status: self.status,
        }
    }

    /// Transform `self.witness1` and `self.normal1` by `pos`.
    pub fn transform1_by(&self, pos: &Pose) -> Self {
        Self {
            time_of_impact: self.time_of_impact,
            witness1: pos * self.witness1,
            witness2: self.witness2,
            normal1: pos.rotation * self.normal1,
            normal2: self.normal2,
            status: self.status,
        }
    }
}

#[cfg(feature = "alloc")]
impl BvhLeafCost for ShapeCastHit {
    #[inline]
    fn cost(&self) -> Real {
        self.time_of_impact
    }
}

/// Configuration for controlling the behavior of time-of-impact (i.e. shape-casting) calculations.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ShapeCastOptions {
    /// The maximum time-of-impacts that can be computed.
    ///
    /// Any impact occurring after this time will be ignored.
    pub max_time_of_impact: Real,
    /// The shapes will be considered as impacting as soon as their distance is smaller or
    /// equal to this target distance. Must be positive or zero.
    ///
    /// If the shapes are separated by a distance smaller than `target_distance` at time 0, the
    /// calculated witness points and normals are only reliable if
    /// [`Self::compute_impact_geometry_on_penetration`] is set to `true`.
    pub target_distance: Real,
    /// If `false`, the time-of-impact algorithm will automatically discard any impact at time
    /// 0 where the velocity is separating (i.e., the relative velocity is such that the distance
    /// between the objects projected on the impact normal is increasing through time).
    pub stop_at_penetration: bool,
    /// If `true`, witness points and normals will be calculated even when the time-of-impact is 0.
    pub compute_impact_geometry_on_penetration: bool,
}

impl ShapeCastOptions {
    // Constructor for the most common use-case.
    /// Crates a [`ShapeCastOptions`] with the default values except for the maximum time of impact.
    pub fn with_max_time_of_impact(max_time_of_impact: Real) -> Self {
        Self {
            max_time_of_impact,
            ..Default::default()
        }
    }
}

impl Default for ShapeCastOptions {
    fn default() -> Self {
        Self {
            max_time_of_impact: Real::MAX,
            target_distance: 0.0,
            stop_at_penetration: true,
            compute_impact_geometry_on_penetration: true,
        }
    }
}

/// Computes when two moving shapes will collide (shape casting / swept collision detection).
///
/// This function determines the **time of impact** when two shapes moving with constant
/// linear velocities will first touch. This is essential for **continuous collision detection**
/// (CCD) to prevent fast-moving objects from tunneling through each other.
///
/// # What is Shape Casting?
///
/// Shape casting extends ray casting to arbitrary shapes:
/// - **Ray casting**: Vector moving in a direction (infinitely thin)
/// - **Shape casting**: Full shape moving in a direction (has volume)
///
/// The shapes move linearly (no rotation) from their initial positions along their
/// velocities until they touch or the time limit is reached.
///
/// # Behavior
///
/// - **Will collide**: Returns `Some(hit)` with time of first impact
/// - **Already touching**: Returns `Some(hit)` with `time_of_impact = 0.0`
/// - **Won't collide**: Returns `None` (no impact within time range)
/// - **Moving apart**: May return `None` depending on `stop_at_penetration` option
///
/// # Arguments
///
/// * `pos1` - Initial position and orientation of the first shape
/// * `vel1` - Linear velocity of the first shape (units per time)
/// * `g1` - The first shape
/// * `pos2` - Initial position and orientation of the second shape
/// * `vel2` - Linear velocity of the second shape
/// * `g2` - The second shape
/// * `options` - Configuration options (max time, target distance, etc.)
///
/// # Options
///
/// Configure behavior with [`ShapeCastOptions`]:
/// - `max_time_of_impact`: Maximum time to check (ignore later impacts)
/// - `target_distance`: Consider "close enough" when within this distance
/// - `stop_at_penetration`: Stop if initially penetrating and moving apart
/// - `compute_impact_geometry_on_penetration`: Compute reliable witnesses at t=0
///
/// # Returns
///
/// * `Ok(Some(hit))` - Impact found, see [`ShapeCastHit`] for details
/// * `Ok(None)` - No impact within time range
/// * `Err(Unsupported)` - This shape pair is not supported
///
/// # Example: Basic Shape Casting
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{cast_shapes, ShapeCastOptions};
/// use parry3d::shape::Ball;
/// use parry3d::math::{Pose, Vector};
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
///
/// // Ball 1 at origin, moving right at speed 2.0
/// let pos1 = Pose::translation(0.0, 0.0, 0.0);
/// let vel1 = Vector::new(2.0, 0.0, 0.0);
///
/// // Ball 2 at x=10, stationary
/// let pos2 = Pose::translation(10.0, 0.0, 0.0);
/// let vel2 = Vector::ZERO;
///
/// let options = ShapeCastOptions::default();
///
/// if let Ok(Some(hit)) = cast_shapes(&pos1, vel1, &ball1, &pos2, vel2, &ball2, options) {
///     // Time when surfaces touch
///     // Distance to cover: 10.0 - 1.0 (radius) - 1.0 (radius) = 8.0
///     // Speed: 2.0, so time = 8.0 / 2.0 = 4.0
///     assert_eq!(hit.time_of_impact, 4.0);
///
///     // Position at impact
///     let impact_pos1 = pos1.translation + vel1 * hit.time_of_impact;
///     // Ball 1 moved 8 units to x=8.0, touching ball 2 at x=10.0
/// }
/// # }
/// ```
///
/// # Example: Already Penetrating
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{cast_shapes, ShapeCastOptions, ShapeCastStatus};
/// use parry3d::shape::Ball;
/// use parry3d::math::{Pose, Vector};
///
/// let ball1 = Ball::new(2.0);
/// let ball2 = Ball::new(2.0);
///
/// // Overlapping balls (centers 3 units apart, radii sum to 4)
/// let pos1 = Pose::translation(0.0, 0.0, 0.0);
/// let pos2 = Pose::translation(3.0, 0.0, 0.0);
/// let vel1 = Vector::X;
/// let vel2 = Vector::ZERO;
///
/// let options = ShapeCastOptions::default();
///
/// if let Ok(Some(hit)) = cast_shapes(&pos1, vel1, &ball1, &pos2, vel2, &ball2, options) {
///     // Already penetrating
///     assert_eq!(hit.time_of_impact, 0.0);
///     assert_eq!(hit.status, ShapeCastStatus::PenetratingOrWithinTargetDist);
/// }
/// # }
/// ```
///
/// # Use Cases
///
/// - **Continuous collision detection**: Prevent tunneling at high speeds
/// - **Predictive collision**: Know when collision will occur
/// - **Sweep tests**: Moving platforms, sliding objects
/// - **Bullet physics**: Fast projectiles that need CCD
///
/// # Performance
///
/// Shape casting is more expensive than static queries:
/// - Uses iterative root-finding algorithms
/// - Multiple distance/contact queries per iteration
/// - Complexity depends on shape types and relative velocities
///
/// # See Also
///
/// - [`cast_shapes_nonlinear`](crate::query::cast_shapes_nonlinear()) - For rotating shapes
/// - [`Ray::cast_ray`](crate::query::RayCast::cast_ray) - For point-like casts
/// - [`ShapeCastOptions`] - Configuration options
/// - [`ShapeCastHit`] - Result structure
pub fn cast_shapes(
    pos1: &Pose,
    vel1: Vector,
    g1: &dyn Shape,
    pos2: &Pose,
    vel2: Vector,
    g2: &dyn Shape,
    options: ShapeCastOptions,
) -> Result<Option<ShapeCastHit>, Unsupported> {
    let pos12 = pos1.inv_mul(pos2);
    let vel12 = pos1.rotation.inverse() * vel2 - vel1;
    DefaultQueryDispatcher.cast_shapes(&pos12, vel12, g1, g2, options)
}
