use crate::math::Real;
use crate::query::{
    DefaultQueryDispatcher, NonlinearRigidMotion, QueryDispatcher, ShapeCastHit, Unsupported,
};
use crate::shape::Shape;

/// Computes when two shapes moving with translation and rotation will first collide.
///
/// This function performs **nonlinear shape casting** - finding the time of impact (TOI) for
/// two shapes that are both translating and rotating. Unlike linear shape casting which only
/// handles straight-line motion, this accounts for the curved trajectories created by rotation.
///
/// # What This Function Does
///
/// Given two shapes with complete rigid body motion (position, linear velocity, and angular
/// velocity), this function finds:
/// - **When** they will first touch (time of impact)
/// - **Where** they touch (witness points)
/// - **How** they touch (contact normals)
///
/// The function solves a complex nonlinear root-finding problem to determine the exact moment
/// when the shapes' minimum distance reaches zero (or a target threshold).
///
/// # How It Differs from Linear Shape Casting
///
/// | Feature | Linear (`cast_shapes`) | Nonlinear (`cast_shapes_nonlinear`) |
/// |---------|------------------------|--------------------------------------|
/// | **Motion type** | Translation only | Translation + rotation |
/// | **Trajectory** | Straight line | Curved (helical path) |
/// | **Input** | Position + velocity | Full rigid motion (position + velocities + angular vel) |
/// | **Use case** | Sliding, sweeping | Spinning, tumbling, realistic physics |
/// | **Performance** | Faster | Slower (more complex) |
///
/// # Arguments
///
/// * `motion1` - Complete motion description for shape 1 ([`NonlinearRigidMotion`])
///   - Starting position and orientation
///   - Linear velocity (translation)
///   - Angular velocity (rotation)
///   - Local center of rotation
/// * `g1` - The first shape geometry
/// * `motion2` - Complete motion description for shape 2
/// * `g2` - The second shape geometry
/// * `start_time` - Beginning of time interval to check (typically `0.0`)
/// * `end_time` - End of time interval to check (e.g., your physics timestep)
/// * `stop_at_penetration` - Controls behavior when shapes start penetrating:
///   - `true`: Returns immediately with `time_of_impact = start_time`
///   - `false`: Checks if they're separating; if so, looks for later impacts
///
/// # The `stop_at_penetration` Parameter
///
/// This parameter is crucial for handling initially-penetrating shapes:
///
/// - **`true` (recommended for most cases)**: If shapes overlap at `start_time`, immediately
///   return a hit at `start_time`. This is safer and prevents tunneling.
///
/// - **`false` (advanced)**: If shapes overlap at `start_time` BUT are moving apart (separating
///   velocity), ignore this initial penetration and search for a later collision that could
///   cause tunneling. Use this when you have external penetration resolution and only care
///   about future impacts.
///
/// # Returns
///
/// * `Ok(Some(hit))` - Collision detected, see [`ShapeCastHit`] for impact details
///   - `time_of_impact`: When collision occurs (in `[start_time, end_time]`)
///   - `witness1`, `witness2`: Contact points on each shape (local space)
///   - `normal1`, `normal2`: Contact normals on each shape (local space)
///   - `status`: Algorithm convergence status
/// * `Ok(None)` - No collision within the time interval
/// * `Err(Unsupported)` - This shape pair is not supported for nonlinear casting
///
/// # Example: Basic Spinning Collision
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{cast_shapes_nonlinear, NonlinearRigidMotion};
/// use parry3d::shape::Ball;
/// use parry3d::math::{Pose, Vector};
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
///
/// // Ball 1: moving right AND spinning around Y axis
/// let motion1 = NonlinearRigidMotion::new(
///     Pose::translation(0.0, 0.0, 0.0), // start position
///     Vector::ZERO,                      // rotation center (local space)
///     Vector::new(2.0, 0.0, 0.0),          // moving right at speed 2
///     Vector::new(0.0, 10.0, 0.0),         // spinning around Y axis
/// );
///
/// // Ball 2: stationary at x=10
/// let motion2 = NonlinearRigidMotion::constant_position(
///     Pose::translation(10.0, 0.0, 0.0)
/// );
///
/// let result = cast_shapes_nonlinear(
///     &motion1, &ball1,
///     &motion2, &ball2,
///     0.0,   // start at t=0
///     10.0,  // check up to t=10
///     true,  // stop if initially penetrating
/// );
///
/// if let Ok(Some(hit)) = result {
///     println!("Collision at time: {}", hit.time_of_impact);
///     println!("Contact point on ball1: {:?}", hit.witness1);
///     println!("Contact normal on ball1: {:?}", hit.normal1);
/// }
/// # }
/// ```
///
/// # Example: Tumbling Box vs Stationary Wall
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{cast_shapes_nonlinear, NonlinearRigidMotion};
/// use parry3d::shape::Cuboid;
/// use parry3d::math::{Pose, Vector};
///
/// // A cuboid tumbling through space
/// let cube = Cuboid::new(Vector::new(0.5, 0.5, 0.5));
///
/// // Large wall (very wide cuboid)
/// let wall = Cuboid::new(Vector::new(10.0, 10.0, 0.1));
///
/// // Cube falling and tumbling
/// let motion_cube = NonlinearRigidMotion::new(
///     Pose::translation(0.0, 5.0, 0.0),  // starting 5 units above
///     Vector::ZERO,                        // rotate around center
///     Vector::new(0.0, -2.0, 0.0),           // falling down
///     Vector::new(1.0, 0.5, 2.0),            // tumbling (complex rotation)
/// );
///
/// // Wall is stationary
/// let motion_wall = NonlinearRigidMotion::constant_position(
///     Pose::translation(0.0, 0.0, 0.0)
/// );
///
/// let result = cast_shapes_nonlinear(
///     &motion_cube, &cube,
///     &motion_wall, &wall,
///     0.0,
///     5.0,  // check 5 seconds of motion
///     true,
/// );
///
/// if let Ok(Some(hit)) = result {
///     // Cube will hit wall while tumbling
///     // Time depends on both falling speed and rotation
///     println!("Tumbling cube hits wall at t = {}", hit.time_of_impact);
/// }
/// # }
/// ```
///
/// # Example: Handling Initial Penetration
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{cast_shapes_nonlinear, NonlinearRigidMotion};
/// use parry3d::shape::Ball;
/// use parry3d::math::{Pose, Vector};
///
/// let ball1 = Ball::new(2.0);
/// let ball2 = Ball::new(2.0);
///
/// // Balls overlapping (3 units apart, but radii sum to 4)
/// let motion1 = NonlinearRigidMotion::new(
///     Pose::translation(0.0, 0.0, 0.0),
///     Vector::ZERO,
///     Vector::new(-1.0, 0.0, 0.0),  // moving AWAY from ball2
///     Vector::ZERO,
/// );
///
/// let motion2 = NonlinearRigidMotion::constant_position(
///     Pose::translation(3.0, 0.0, 0.0)
/// );
///
/// // With stop_at_penetration = true
/// let result_stop = cast_shapes_nonlinear(
///     &motion1, &ball1, &motion2, &ball2,
///     0.0, 10.0, true,  // STOP at penetration
/// );
/// // Returns Some(hit) with time_of_impact = 0.0
///
/// // With stop_at_penetration = false
/// let result_continue = cast_shapes_nonlinear(
///     &motion1, &ball1, &motion2, &ball2,
///     0.0, 10.0, false,  // DON'T stop - they're separating
/// );
/// // Returns None - shapes are moving apart, no future impact
/// # }
/// ```
///
/// # When to Use This vs Linear Shape Casting
///
/// **Use `cast_shapes_nonlinear` when:**
/// - Objects have significant angular velocity
/// - Rotation accuracy matters (spinning blades, tumbling debris)
/// - Physics simulation with full 6-DOF motion
/// - Objects can rotate > 10-15 degrees during timestep
///
/// **Use `cast_shapes` (linear) when:**
/// - Objects don't rotate (angular velocity ≈ 0)
/// - Rotation is negligible for your timestep
/// - Performance is critical
/// - Implementing simple sweep/slide mechanics
///
/// # Performance Considerations
///
/// Nonlinear shape casting is significantly more expensive than linear:
/// - **Iterative root-finding**: Multiple evaluations to converge
/// - **Rotational transforms**: Matrix operations at each evaluation
/// - **Complex geometry**: Shape changes orientation during motion
///
/// Typical cost: 3-10x slower than linear shape casting, depending on:
/// - Shape complexity (balls faster than meshes)
/// - Angular velocity magnitude (faster rotation = more iterations)
/// - Time interval length (longer intervals may need more precision)
///
/// # Algorithm Notes
///
/// The implementation uses conservative root-finding algorithms that:
/// - Guarantee no false negatives (won't miss collisions)
/// - May require multiple iterations to converge
/// - Handle degenerate cases (parallel surfaces, grazing contact)
/// - Account for numerical precision limits
///
/// # Common Pitfalls
///
/// 1. **Units**: Ensure time, velocity, and angular velocity units match
///    - If time is in seconds, velocity should be units/second
///    - Angular velocity in radians/second (not degrees!)
///
/// 2. **Time interval**: Keep `end_time - start_time` reasonable
///    - Very large intervals may miss fast rotations
///    - Typical: use your physics timestep (e.g., 1/60 second)
///
/// 3. **Rotation center**: `local_center` in [`NonlinearRigidMotion`] must be in
///    the shape's local coordinate system, not world space
///
/// 4. **Initial penetration**: Understand `stop_at_penetration` behavior for your use case
///
/// # See Also
///
/// - [`NonlinearRigidMotion`] - Describes an object's complete motion
/// - [`cast_shapes`](crate::query::cast_shapes) - Linear shape casting (no rotation)
/// - [`ShapeCastHit`] - Result structure
/// - [`contact`](crate::query::contact::contact()) - Static contact queries (no motion)
pub fn cast_shapes_nonlinear(
    motion1: &NonlinearRigidMotion,
    g1: &dyn Shape,
    motion2: &NonlinearRigidMotion,
    g2: &dyn Shape,
    start_time: Real,
    end_time: Real,
    stop_at_penetration: bool,
) -> Result<Option<ShapeCastHit>, Unsupported> {
    DefaultQueryDispatcher.cast_shapes_nonlinear(
        motion1,
        g1,
        motion2,
        g2,
        start_time,
        end_time,
        stop_at_penetration,
    )
}
