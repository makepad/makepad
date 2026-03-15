//! The Gilbert-Johnson-Keerthi distance algorithm.
//!
//! # What is GJK?
//!
//! The **Gilbert-Johnson-Keerthi (GJK)** algorithm is a fundamental geometric algorithm used
//! to compute the distance between two convex shapes. It's one of the most important algorithms
//! in collision detection and is used extensively in physics engines, robotics, and computer graphics.
//!
//! ## How GJK Works (Simplified)
//!
//! GJK works by operating on the **Minkowski difference** (also called Configuration Space Obstacle or CSO)
//! of two shapes. Instead of directly comparing the shapes, GJK:
//!
//! 1. Constructs a simplex (triangle in 2D, tetrahedron in 3D) within the Minkowski difference
//! 2. Iteratively refines this simplex to find the point closest to the origin
//! 3. The distance from the origin to this closest point equals the distance between the shapes
//!
//! If the origin is **inside** the Minkowski difference, the shapes are intersecting.
//! If the origin is **outside**, the distance to the closest point gives the separation distance.
//!
//! ## When is GJK Used?
//!
//! GJK is used whenever you need to:
//! - **Check if two convex shapes intersect** (collision detection)
//! - **Find the minimum distance** between two convex shapes
//! - **Compute closest points** on two shapes
//! - **Cast a shape along a direction** to find the time of impact (continuous collision detection)
//!
//! ## Key Advantages of GJK
//!
//! - Works with **any convex shape** that can provide a support function
//! - Does **not require the full geometry** of the shapes (only support points)
//! - Very **fast convergence** in most practical cases
//! - Forms the basis for many collision detection systems
//!
//! ## Limitations
//!
//! - Only works with **convex shapes** (use convex decomposition for concave shapes)
//! - When shapes are penetrating, GJK can only detect intersection but not penetration depth
//!   (use EPA - Expanding Polytope Algorithm - for penetration depth)
//!
//! # Main Functions in This Module
//!
//! - [`closest_points`] - The core GJK algorithm for finding distance and closest points
//! - [`project_origin`] - Projects the origin onto a shape's boundary
//! - [`cast_local_ray`] - Casts a ray against a shape (used for raycasting)
//! - [`directional_distance`] - Computes how far a shape can move before touching another
//!
//! # Example
//!
//! See individual function documentation for usage examples.

use crate::math::ComplexField;

use crate::query::gjk::{ConstantOrigin, CsoPoint, VoronoiSimplex};
use crate::shape::SupportMap;
// use query::Proximity;
use crate::math::{Pose, Real, Vector, DIM};
use crate::query::{self, Ray};

use num::{Bounded, Zero};

/// Results of the GJK algorithm.
///
/// This enum represents the different outcomes when running the GJK algorithm to find
/// the distance between two shapes. The result depends on whether the shapes are intersecting,
/// how far apart they are, and what information was requested.
///
/// # Understanding the Results
///
/// - **Intersection**: The shapes are overlapping. The origin lies inside the Minkowski difference.
/// - **ClosestPoints**: The exact closest points on both shapes were computed, along with the
///   separation direction.
/// - **Proximity**: The shapes are close but not intersecting. Only an approximate separation
///   direction is provided (used when exact distance computation is not needed).
/// - **NoIntersection**: The shapes are too far apart (beyond the specified `max_dist` threshold).
///
/// # Coordinate Spaces
///
/// All points and vectors in this result are expressed in the **local-space of the first shape**
/// (the shape passed as `g1` to the GJK functions). This is important when working with
/// transformed shapes.
#[derive(Clone, Debug, PartialEq)]
pub enum GJKResult {
    /// The shapes are intersecting (overlapping).
    ///
    /// This means the origin is inside the Minkowski difference of the two shapes.
    /// GJK cannot compute penetration depth; use the EPA (Expanding Polytope Algorithm)
    /// for that purpose.
    Intersection,

    /// The closest points on both shapes were found.
    ///
    /// # Fields
    ///
    /// - First `Vector`: The closest point on the first shape (in local-space of shape 1)
    /// - Second `Vector`: The closest point on the second shape (in local-space of shape 1)
    /// - `Vector`: The unit direction vector from shape 1 to shape 2 (separation axis)
    ///
    /// This variant is returned when `exact_dist` is `true` in the GJK algorithm and the
    /// shapes are not intersecting.
    ClosestPoints(Vector, Vector, Vector),

    /// The shapes are in proximity (close but not intersecting).
    ///
    /// # Fields
    ///
    /// - `Vector`: An approximate separation axis (unit direction from shape 1 to shape 2)
    ///
    /// This variant is returned when `exact_dist` is `false` and the algorithm determines
    /// the shapes are close but not intersecting. It's faster than computing exact closest
    /// points when you only need to know if shapes are nearby.
    Proximity(Vector),

    /// The shapes are too far apart.
    ///
    /// # Fields
    ///
    /// - `Vector`: A separation axis (unit direction from shape 1 to shape 2)
    ///
    /// This variant is returned when the minimum distance between the shapes exceeds
    /// the `max_dist` parameter passed to the GJK algorithm.
    NoIntersection(Vector),
}

/// The absolute tolerance used by the GJK algorithm.
///
/// This function returns the epsilon (tolerance) value that GJK uses to determine when
/// it has converged to a solution. The tolerance affects:
///
/// - When two points are considered "close enough" to be the same
/// - When the algorithm decides it has found the minimum distance
/// - Numerical stability in edge cases
///
/// The returned value is 10 times the default machine epsilon for the current floating-point
/// precision (f32 or f64). This provides a balance between accuracy and robustness.
///
/// # Returns
///
/// The absolute tolerance value (10 * DEFAULT_EPSILON)
pub fn eps_tol() -> Real {
    let _eps = crate::math::DEFAULT_EPSILON;
    _eps * 10.0
}

/// Projects the origin onto the boundary of the given shape.
///
/// This function finds the point on the shape's surface that is closest to the origin (0, 0)
/// in 2D or (0, 0, 0) in 3D. This is useful for distance queries and collision detection
/// when you need to know the closest point on a shape.
///
/// # Important: Origin Must Be Outside
///
/// **The origin is assumed to be outside of the shape.** If the origin is inside the shape,
/// this function returns `None`. For penetrating cases, use the EPA (Expanding Polytope Algorithm)
/// instead.
///
/// # Parameters
///
/// - `m`: The position and orientation (isometry) of the shape in world space
/// - `g`: The shape to project onto (must implement `SupportMap`)
/// - `simplex`: A reusable simplex structure for the GJK algorithm. Initialize with
///   `VoronoiSimplex::new()` before first use.
///
/// # Returns
///
/// - `Some(Vector)`: The closest point on the shape's boundary, in the shape's **local space**
/// - `None`: If the origin is inside the shape
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Ball;
/// use parry3d::query::gjk::{project_origin, VoronoiSimplex};
/// use parry3d::math::Pose;
///
/// // Create a ball at position (5, 0, 0)
/// let ball = Ball::new(1.0);
/// let position = Pose::translation(5.0, 0.0, 0.0);
///
/// // Project the origin onto the ball
/// let mut simplex = VoronoiSimplex::new();
/// if let Some(closest_point) = project_origin(&position, &ball, &mut simplex) {
///     println!("Closest point on ball: {:?}", closest_point);
///     // The point will be approximately (-1, 0, 0) in local space
///     // which is the left side of the ball facing the origin
/// }
/// # }
/// ```
///
/// # Performance Note
///
/// The `simplex` parameter can be reused across multiple calls to avoid allocations.
/// This is particularly beneficial when performing many projection queries.
pub fn project_origin<G: ?Sized + SupportMap>(
    m: &Pose,
    g: &G,
    simplex: &mut VoronoiSimplex,
) -> Option<Vector> {
    match closest_points(
        &m.inverse(),
        g,
        &ConstantOrigin,
        Real::max_value(),
        true,
        simplex,
    ) {
        GJKResult::Intersection => None,
        GJKResult::ClosestPoints(p, _, _) => Some(p),
        _ => unreachable!(),
    }
}

/*
 * Separating Axis GJK
 */
/// Computes the closest points between two shapes using the GJK algorithm.
///
/// This is the **core function** of the GJK implementation in Parry. It can compute:
/// - Whether two shapes are intersecting
/// - The distance between two separated shapes
/// - The closest points on both shapes
/// - An approximate separation axis when exact distance isn't needed
///
/// # How It Works
///
/// The algorithm operates on the Minkowski difference (CSO) of the two shapes and iteratively
/// builds a simplex that approximates the point closest to the origin. The algorithm terminates
/// when:
/// - The shapes are proven to intersect (origin is inside the CSO)
/// - The minimum distance is found within the tolerance
/// - The shapes are proven to be farther than `max_dist` apart
///
/// # Parameters
///
/// - `pos12`: The relative position of shape 2 with respect to shape 1. This is the isometry
///   that transforms from shape 1's space to shape 2's space.
/// - `g1`: The first shape (must implement `SupportMap`)
/// - `g2`: The second shape (must implement `SupportMap`)
/// - `max_dist`: Maximum distance to check. If shapes are farther than this, the algorithm
///   returns `GJKResult::NoIntersection` early. Use `Real::max_value()` to disable this check.
/// - `exact_dist`: Whether to compute exact closest points:
///   - `true`: Computes exact distance and returns `GJKResult::ClosestPoints`
///   - `false`: May return `GJKResult::Proximity` with only an approximate separation axis,
///     which is faster when you only need to know if shapes are nearby
/// - `simplex`: A reusable simplex structure. Initialize with `VoronoiSimplex::new()` before
///   first use. Can be reused across calls for better performance.
///
/// # Returns
///
/// Returns a [`GJKResult`] which can be:
/// - `Intersection`: The shapes are overlapping
/// - `ClosestPoints(p1, p2, normal)`: The closest points on each shape (when `exact_dist` is true)
/// - `Proximity(axis)`: An approximate separation axis (when `exact_dist` is false)
/// - `NoIntersection(axis)`: The shapes are farther than `max_dist` apart
///
/// # Example: Basic Distance Query
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Ball, Cuboid};
/// use parry3d::query::gjk::{closest_points, VoronoiSimplex};
/// use parry3d::math::{Pose, Vector};
///
/// // Create two shapes
/// let ball = Ball::new(1.0);
/// let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
///
/// // Position them in space
/// let pos1 = Pose::translation(0.0, 0.0, 0.0);
/// let pos2 = Pose::translation(5.0, 0.0, 0.0);
///
/// // Compute relative position
/// let pos12 = pos1.inv_mul(&pos2);
///
/// // Run GJK
/// let mut simplex = VoronoiSimplex::new();
/// let result = closest_points(
///     &pos12,
///     &ball,
///     &cuboid,
///     f32::MAX,  // No distance limit
///     true,      // Compute exact distance
///     &mut simplex,
/// );
///
/// match result {
///     parry3d::query::gjk::GJKResult::ClosestPoints(p1, p2, normal) => {
///         println!("Closest point on ball: {:?}", p1);
///         println!("Closest point on cuboid: {:?}", p2);
///         println!("Separation direction: {:?}", normal);
///         let distance = (p2 - p1).length();
///         println!("Distance: {}", distance);
///     }
///     parry3d::query::gjk::GJKResult::Intersection => {
///         println!("Shapes are intersecting!");
///     }
///     _ => {}
/// }
/// # }
/// ```
///
/// # Example: Fast Proximity Check
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Ball;
/// use parry3d::query::gjk::{closest_points, VoronoiSimplex};
/// use parry3d::math::Pose;
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
/// let pos12 = Pose::translation(3.0, 0.0, 0.0);
///
/// let mut simplex = VoronoiSimplex::new();
/// let result = closest_points(
///     &pos12,
///     &ball1,
///     &ball2,
///     5.0,       // Only check up to distance 5.0
///     false,     // Don't compute exact distance
///     &mut simplex,
/// );
///
/// match result {
///     parry3d::query::gjk::GJKResult::Proximity(_axis) => {
///         println!("Shapes are close but not intersecting");
///     }
///     parry3d::query::gjk::GJKResult::Intersection => {
///         println!("Shapes are intersecting");
///     }
///     parry3d::query::gjk::GJKResult::NoIntersection(_) => {
///         println!("Shapes are too far apart (> 5.0 units)");
///     }
///     _ => {}
/// }
/// # }
/// ```
///
/// # Performance Tips
///
/// 1. Reuse the `simplex` across multiple queries to avoid allocations
/// 2. Set `exact_dist` to `false` when you only need proximity information
/// 3. Use a reasonable `max_dist` to allow early termination
/// 4. GJK converges fastest when shapes are well-separated
///
/// # Notes
///
/// - All returned points and vectors are in the local-space of shape 1
/// - The algorithm typically converges in 5-10 iterations for well-separated shapes
/// - Maximum iteration count is 100 to prevent infinite loops
pub fn closest_points<G1, G2>(
    pos12: &Pose,
    g1: &G1,
    g2: &G2,
    max_dist: Real,
    exact_dist: bool,
    simplex: &mut VoronoiSimplex,
) -> GJKResult
where
    G1: ?Sized + SupportMap,
    G2: ?Sized + SupportMap,
{
    let _eps = crate::math::DEFAULT_EPSILON;
    let _eps_tol: Real = eps_tol();
    let _eps_rel: Real = <Real as ComplexField>::sqrt(_eps_tol);

    // TODO: reset the simplex if it is empty?
    let mut proj = simplex.project_origin_and_reduce();

    let mut old_dir;

    if let Some(proj_dir) = proj.try_normalize() {
        old_dir = -proj_dir;
    } else {
        return GJKResult::Intersection;
    }

    let mut max_bound = Real::max_value();
    let mut dir;
    let mut niter = 0;

    loop {
        let old_max_bound = max_bound;

        let neg_proj_coords = -proj;
        let (normalized, dist) = neg_proj_coords.normalize_and_length();
        if dist > _eps_tol {
            dir = normalized;
            max_bound = dist;
        } else {
            // The origin is on the simplex.
            return GJKResult::Intersection;
        }

        if max_bound >= old_max_bound {
            if exact_dist {
                let (p1, p2) = result(simplex, true);
                return GJKResult::ClosestPoints(p1, p2, old_dir); // upper bounds inconsistencies
            } else {
                return GJKResult::Proximity(old_dir);
            }
        }

        let cso_point = CsoPoint::from_shapes(pos12, g1, g2, dir);
        let min_bound = -dir.dot(cso_point.point);

        assert!(min_bound.is_finite());

        if min_bound > max_dist {
            return GJKResult::NoIntersection(dir);
        } else if !exact_dist && min_bound > 0.0 && max_bound <= max_dist {
            return GJKResult::Proximity(old_dir);
        } else if max_bound - min_bound <= _eps_rel * max_bound {
            if exact_dist {
                let (p1, p2) = result(simplex, false);
                return GJKResult::ClosestPoints(p1, p2, dir); // the distance found has a good enough precision
            } else {
                return GJKResult::Proximity(dir);
            }
        }

        if !simplex.add_point(cso_point) {
            if exact_dist {
                let (p1, p2) = result(simplex, false);
                return GJKResult::ClosestPoints(p1, p2, dir);
            } else {
                return GJKResult::Proximity(dir);
            }
        }

        old_dir = dir;
        proj = simplex.project_origin_and_reduce();

        if simplex.dimension() == DIM {
            if min_bound >= _eps_tol {
                if exact_dist {
                    let (p1, p2) = result(simplex, true);
                    return GJKResult::ClosestPoints(p1, p2, old_dir);
                } else {
                    // NOTE: previous implementation used old_proj here.
                    return GJKResult::Proximity(old_dir);
                }
            } else {
                return GJKResult::Intersection; // Vector inside of the cso.
            }
        }
        niter += 1;

        if niter == 100 {
            return GJKResult::NoIntersection(Vector::X);
        }
    }
}

/// Casts a ray against a shape using the GJK algorithm.
///
/// This function performs raycasting by testing a ray against a shape to find if and where
/// the ray intersects the shape. It uses a specialized version of GJK that works with rays.
///
/// # What is Raycasting?
///
/// Raycasting shoots a ray (infinite line starting from a point in a direction) and finds
/// where it first hits a shape. This is essential for:
/// - Mouse picking in 3D scenes
/// - Line-of-sight checks
/// - Projectile collision detection
/// - Laser/scanner simulations
///
/// # Parameters
///
/// - `shape`: The shape to cast the ray against (must implement `SupportMap`)
/// - `simplex`: A reusable simplex structure. Initialize with `VoronoiSimplex::new()`.
/// - `ray`: The ray to cast, containing an origin point and direction vector
/// - `max_time_of_impact`: Maximum distance along the ray to check. The ray will be treated
///   as a line segment of length `max_time_of_impact * ray.dir.length()`.
///
/// # Returns
///
/// - `Some((toi, normal))`: If the ray hits the shape
///   - `toi`: Time of impact - multiply by `ray.dir.length()` to get the actual distance
///   - `normal`: Surface normal at the hit point
/// - `None`: If the ray doesn't hit the shape within the maximum distance
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Ball;
/// use parry3d::query::{Ray, gjk::{cast_local_ray, VoronoiSimplex}};
/// use parry3d::math::Vector;
///
/// // Create a ball at the origin
/// let ball = Ball::new(1.0);
///
/// // Create a ray starting at (0, 0, -5) pointing toward +Z
/// let ray = Ray::new(
///     Vector::new(0.0, 0.0, -5.0),
///     Vector::new(0.0, 0.0, 1.0)
/// );
///
/// let mut simplex = VoronoiSimplex::new();
/// if let Some((toi, normal)) = cast_local_ray(&ball, &mut simplex, &ray, f32::MAX) {
///     let hit_point = ray.point_at(toi);
///     println!("Ray hit at: {:?}", hit_point);
///     println!("Surface normal: {:?}", normal);
///     println!("Distance: {}", toi);
/// } else {
///     println!("Ray missed the shape");
/// }
/// # }
/// ```
///
/// # Notes
///
/// - The ray is specified in the local-space of the shape
/// - The returned normal points outward from the shape
/// - For normalized ray directions, `toi` equals the distance to the hit point
/// - This function is typically called by higher-level raycasting APIs
pub fn cast_local_ray<G: ?Sized + SupportMap>(
    shape: &G,
    simplex: &mut VoronoiSimplex,
    ray: &Ray,
    max_time_of_impact: Real,
) -> Option<(Real, Vector)> {
    let g2 = ConstantOrigin;
    minkowski_ray_cast(
        &Pose::IDENTITY,
        shape,
        &g2,
        ray,
        max_time_of_impact,
        simplex,
    )
}

/// Computes how far a shape can move in a direction before touching another shape.
///
/// This function answers the question: "If I move shape 1 along this direction, how far
/// can it travel before it touches shape 2?" This is useful for:
/// - Continuous collision detection (CCD)
/// - Movement planning and obstacle avoidance
/// - Computing time-of-impact for moving objects
/// - Safe navigation distances
///
/// # How It Works
///
/// The function casts shape 1 along the given direction vector and finds the first point
/// where it would contact shape 2. It returns:
/// - The distance that can be traveled
/// - The contact normal at the point of first contact
/// - The witness points (closest points) on both shapes at contact
///
/// # Parameters
///
/// - `pos12`: The relative position of shape 2 with respect to shape 1 (isometry from
///   shape 1's space to shape 2's space)
/// - `g1`: The first shape being moved (must implement `SupportMap`)
/// - `g2`: The second shape (static target, must implement `SupportMap`)
/// - `dir`: The direction vector to move shape 1 in (in local-space of shape 1)
/// - `simplex`: A reusable simplex structure. Initialize with `VoronoiSimplex::new()`.
///
/// # Returns
///
/// - `Some((distance, normal, witness1, witness2))`: If contact would occur
///   - `distance`: How far shape 1 can travel before touching shape 2
///   - `normal`: The contact normal at the point of first contact
///   - `witness1`: The contact point on shape 1 (in local-space of shape 1)
///   - `witness2`: The contact point on shape 2 (in local-space of shape 1)
/// - `None`: If no contact would occur (shapes don't intersect along this direction)
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Ball;
/// use parry3d::query::gjk::{directional_distance, VoronoiSimplex};
/// use parry3d::math::{Pose, Vector};
///
/// // Two balls: one at origin, one at (10, 0, 0)
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
/// let pos12 = Pose::translation(10.0, 0.0, 0.0);
///
/// // Move ball1 toward ball2 along the +X axis
/// let direction = Vector::new(1.0, 0.0, 0.0);
///
/// let mut simplex = VoronoiSimplex::new();
/// if let Some((distance, normal, w1, w2)) = directional_distance(
///     &pos12,
///     &ball1,
///     &ball2,
///     direction,
///     &mut simplex
/// ) {
///     println!("Ball1 can move {} units before contact", distance);
///     println!("Contact normal: {:?}", normal);
///     println!("Contact point on ball1: {:?}", w1);
///     println!("Contact point on ball2: {:?}", w2);
///     // Expected: distance ≈ 8.0 (10.0 - 1.0 - 1.0)
/// }
/// # }
/// ```
///
/// # Use Cases
///
/// **1. Continuous Collision Detection:**
/// ```ignore
/// let movement_dir = velocity * time_step;
/// if let Some((toi, normal, _, _)) = directional_distance(...) {
///     if toi < 1.0 {
///         // Collision will occur during this timestep
///         let collision_time = toi * time_step;
///     }
/// }
/// ```
///
/// **2. Safe Movement Distance:**
/// ```ignore
/// let desired_movement = Vector::new(5.0, 0.0, 0.0);
/// if let Some((max_safe_dist, _, _, _)) = directional_distance(...) {
///     let actual_movement = desired_movement.normalize() * max_safe_dist.min(5.0);
/// }
/// ```
///
/// # Notes
///
/// - All inputs and outputs are in the local-space of shape 1
/// - If the shapes are already intersecting, the returned distance is 0.0 and witness
///   points are undefined (set to origin)
/// - The direction vector does not need to be normalized
/// - This function internally uses GJK raycasting on the Minkowski difference
pub fn directional_distance<G1, G2>(
    pos12: &Pose,
    g1: &G1,
    g2: &G2,
    dir: Vector,
    simplex: &mut VoronoiSimplex,
) -> Option<(Real, Vector, Vector, Vector)>
where
    G1: ?Sized + SupportMap,
    G2: ?Sized + SupportMap,
{
    let ray = Ray::new(Vector::ZERO, dir);
    minkowski_ray_cast(pos12, g1, g2, &ray, Real::max_value(), simplex).map(
        |(time_of_impact, normal)| {
            let witnesses = if !time_of_impact.is_zero() {
                result(simplex, simplex.dimension() == DIM)
            } else {
                // If there is penetration, the witness points
                // are undefined.
                (Vector::ZERO, Vector::ZERO)
            };

            (time_of_impact, normal, witnesses.0, witnesses.1)
        },
    )
}

// Ray-cast on the Minkowski Difference `g1 - pos12 * g2`.
fn minkowski_ray_cast<G1, G2>(
    pos12: &Pose,
    g1: &G1,
    g2: &G2,
    ray: &Ray,
    max_time_of_impact: Real,
    simplex: &mut VoronoiSimplex,
) -> Option<(Real, Vector)>
where
    G1: ?Sized + SupportMap,
    G2: ?Sized + SupportMap,
{
    let _eps = crate::math::DEFAULT_EPSILON;
    let _eps_tol: Real = eps_tol();
    let _eps_rel: Real = <Real as ComplexField>::sqrt(_eps_tol);

    let ray_length = ray.dir.length();

    if relative_eq!(ray_length, 0.0) {
        return None;
    }

    let mut ltoi = 0.0;
    let mut curr_ray = Ray::new(ray.origin, ray.dir / ray_length);
    let dir = -curr_ray.dir;
    let mut ldir = dir;

    // Initialize the simplex.
    let support_point = CsoPoint::from_shapes(pos12, g1, g2, dir);
    simplex.reset(support_point.translate(-curr_ray.origin));

    // TODO: reset the simplex if it is empty?
    let mut proj = simplex.project_origin_and_reduce();
    let mut max_bound = Real::max_value();
    let mut dir;
    let mut niter = 0;
    let mut last_chance = false;

    loop {
        let old_max_bound = max_bound;

        let neg_proj_coords = -proj;
        let (normalized, dist) = neg_proj_coords.normalize_and_length();
        if dist > _eps_tol {
            dir = normalized;
            max_bound = dist;
        } else {
            return Some((ltoi / ray_length, ldir));
        }

        let support_point = if max_bound >= old_max_bound {
            // Upper bounds inconsistencies. Consider the projection as a valid support point.
            last_chance = true;
            CsoPoint::single_point(proj + curr_ray.origin)
        } else {
            CsoPoint::from_shapes(pos12, g1, g2, dir)
        };

        if last_chance && ltoi > 0.0 {
            // last_chance && ltoi > 0.0 && (support_point.point - curr_ray.origin).dot(ldir) >= 0.0 {
            return Some((ltoi / ray_length, ldir));
        }

        // Clip the ray on the support halfspace (None <=> t < 0)
        // The configurations are:
        //   dir.dot(curr_ray.dir)  |   t   |               Action
        // −−−−−−−−−−−−−−−−−−−−-----+−−−−−−−+−−−−−−−−−−−−−−−−−−−−−−−−−−−−−−−−−−−−
        //          < 0             |  < 0  | Continue.
        //          < 0             |  > 0  | New lower bound, move the origin.
        //          > 0             |  < 0  | Miss. No intersection.
        //          > 0             |  > 0  | New higher bound.
        match query::details::ray_toi_with_halfspace(support_point.point, dir, &curr_ray) {
            Some(t) => {
                if dir.dot(curr_ray.dir) < 0.0 && t > 0.0 {
                    // new lower bound
                    ldir = dir;
                    ltoi += t;

                    // NOTE: we divide by ray_length instead of doing max_time_of_impact * ray_length
                    // because the multiplication may cause an overflow if max_time_of_impact is set
                    // to Real::max_value() by users that want to have an infinite ray.
                    if ltoi / ray_length > max_time_of_impact {
                        return None;
                    }

                    let shift = curr_ray.dir * t;
                    curr_ray.origin += shift;
                    max_bound = Real::max_value();
                    simplex.modify_pnts(&|pt| pt.translate_mut(-shift));
                    last_chance = false;
                }
            }
            None => {
                if dir.dot(curr_ray.dir) > _eps_tol {
                    // miss
                    return None;
                }
            }
        }

        if last_chance {
            return None;
        }

        let min_bound = -dir.dot(support_point.point - curr_ray.origin);

        assert!(min_bound.is_finite());

        if max_bound - min_bound <= _eps_rel * max_bound {
            // This is needed when using fixed-points to avoid missing
            // some castes.
            // TODO: I feel like we should always return `Some` in
            // this case, even with floating-point numbers. Though it
            // has not been sufficiently tested with floats yet to be sure.
            if cfg!(feature = "improved_fixed_point_support") {
                return Some((ltoi / ray_length, ldir));
            } else {
                return None;
            }
        }

        let _ = simplex.add_point(support_point.translate(-curr_ray.origin));
        proj = simplex.project_origin_and_reduce();

        if simplex.dimension() == DIM {
            if min_bound >= _eps_tol {
                return None;
            } else {
                return Some((ltoi / ray_length, ldir)); // Vector inside of the cso.
            }
        }

        niter += 1;
        if niter == 100 {
            return None;
        }
    }
}

fn result(simplex: &VoronoiSimplex, prev: bool) -> (Vector, Vector) {
    let mut res = (Vector::ZERO, Vector::ZERO);
    if prev {
        for i in 0..simplex.prev_dimension() + 1 {
            let coord = simplex.prev_proj_coord(i);
            let point = simplex.prev_point(i);
            res.0 += point.orig1 * coord;
            res.1 += point.orig2 * coord;
        }

        res
    } else {
        for i in 0..simplex.dimension() + 1 {
            let coord = simplex.proj_coord(i);
            let point = simplex.point(i);
            res.0 += point.orig1 * coord;
            res.1 += point.orig2 * coord;
        }

        res
    }
}
