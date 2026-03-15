//! Nonlinear shape casting with rotation for continuous collision detection.
//!
//! This module provides **nonlinear shape casting** - detecting when and where two shapes
//! moving with both **translation and rotation** will first collide. This is essential for
//! accurate continuous collision detection (CCD) when objects are spinning or tumbling.
//!
//! # What is Nonlinear Shape Casting?
//!
//! While linear shape casting (see [`cast_shapes`](crate::query::cast_shapes)) handles objects
//! moving in straight lines, **nonlinear shape casting** accounts for rotational motion:
//!
//! - **Linear casting**: Objects translate only (constant velocity, straight path)
//! - **Nonlinear casting**: Objects both translate and rotate (curved trajectory)
//!
//! When objects rotate during motion, their effective path through space becomes nonlinear,
//! requiring more sophisticated algorithms to detect the first moment of contact.
//!
//! # When to Use Nonlinear vs Linear Shape Casting
//!
//! ## Use **Nonlinear** Shape Casting When:
//!
//! - Objects have **angular velocity** (spinning, rotating, tumbling)
//! - Accurate rotation is critical (e.g., rotating blades, tumbling debris)
//! - Objects can rotate significantly during the timestep
//! - You need physically accurate CCD for rotational motion
//!
//! ## Use **Linear** Shape Casting When:
//!
//! - Objects only translate (no rotation)
//! - Rotational effects are negligible for the timestep
//! - Performance is critical and rotation can be approximated
//! - You're implementing simple sweeping/sliding mechanics
//!
//! # Key Differences from Linear Shape Casting
//!
//! | Aspect | Linear (`cast_shapes`) | Nonlinear (`cast_shapes_nonlinear`) |
//! |--------|------------------------|--------------------------------------|
//! | **Motion** | Translation only | Translation + rotation |
//! | **Path** | Straight line | Curved (helical) trajectory |
//! | **Input** | Position + velocity | Position + velocity + angular velocity |
//! | **Complexity** | Lower (simpler math) | Higher (rotational transforms) |
//! | **Accuracy** | Perfect for non-rotating | Accurate for rotating objects |
//! | **Performance** | Faster | Slower (more iterations) |
//!
//! # Main Types
//!
//! - [`cast_shapes_nonlinear`] - Main function for nonlinear shape casting
//! - [`NonlinearRigidMotion`] - Describes an object's motion (position, velocities, rotation center)
//!
//! # Physics Background
//!
//! In rigid body physics, an object's motion has 6 degrees of freedom (3D):
//! - **3 translational**: Linear velocity in x, y, z
//! - **3 rotational**: Angular velocity around x, y, z axes
//!
//! Nonlinear shape casting simulates this complete motion to find collision times,
//! ensuring no tunneling occurs even when objects spin at high angular velocities.
//!
//! # Example: Rotating vs Non-Rotating Collision
//!
//! ```rust
//! # #[cfg(all(feature = "dim3", feature = "f32"))] {
//! use parry3d::query::{cast_shapes, cast_shapes_nonlinear, ShapeCastOptions, NonlinearRigidMotion};
//! use parry3d::shape::Cuboid;
//! use parry3d::math::{Pose, Vector};
//!
//! let cube1 = Cuboid::new(Vector::splat(1.0));
//! let cube2 = Cuboid::new(Vector::splat(0.5));
//!
//! let pos1 = Pose::translation(0.0, 0.0, 0.0);
//! let pos2 = Pose::translation(5.0, 0.0, 0.0);
//!
//! // Linear motion: cube moves right
//! let vel1 = Vector::new(1.0, 0.0, 0.0);
//! let options = ShapeCastOptions::default();
//!
//! let linear_hit = cast_shapes(&pos1, vel1, &cube1, &pos2, Vector::ZERO, &cube2, options);
//!
//! // Nonlinear motion: cube moves right AND spins around Y axis
//! let motion1 = NonlinearRigidMotion::new(
//!     pos1,
//!     Vector::ZERO, // rotation center
//!     vel1,                        // linear velocity
//!     Vector::new(0.0, 5.0, 0.0), // angular velocity (spinning fast)
//! );
//! let motion2 = NonlinearRigidMotion::constant_position(pos2);
//!
//! let nonlinear_hit = cast_shapes_nonlinear(
//!     &motion1,
//!     &cube1,
//!     &motion2,
//!     &cube2,
//!     0.0,  // start time
//!     10.0, // end time
//!     true, // stop at penetration
//! );
//!
//! // The spinning cube may collide at a different time due to rotation!
//! // Its corners sweep out a larger effective volume as it spins.
//! # }
//! ```
//!
//! # Performance Considerations
//!
//! Nonlinear shape casting is computationally more expensive than linear casting:
//!
//! - More iterations needed for convergence (curved paths harder to solve)
//! - Additional rotational transform calculations
//! - More complex geometry at each time step
//!
//! **Optimization Tips:**
//! - Use linear casting when angular velocity is near zero
//! - Keep time intervals (`end_time - start_time`) reasonably small
//! - Consider approximating slow rotations with linear motion
//!
//! # Common Use Cases
//!
//! 1. **Physics Simulations**: Accurate CCD for spinning rigid bodies
//! 2. **Game Mechanics**: Rotating blades, spinning projectiles, tumbling objects
//! 3. **Robotics**: Robotic arms with rotating joints
//! 4. **Vehicle Physics**: Rolling wheels, spinning debris from impacts
//! 5. **Animation**: Ensuring no intersections during animated rotations
//!
//! # See Also
//!
//! - [`cast_shapes`](crate::query::cast_shapes) - Linear shape casting (translation only)
//! - [`ShapeCastHit`](crate::query::ShapeCastHit) - Result structure with impact information
//! - [`contact`](crate::query::contact::contact()) - Static contact detection (no motion)

#[cfg(feature = "alloc")]
pub use self::nonlinear_shape_cast_composite_shape_shape::{
    cast_shapes_nonlinear_composite_shape_shape, cast_shapes_nonlinear_shape_composite_shape,
};
#[cfg(feature = "alloc")]
pub use self::nonlinear_shape_cast_voxels_shape::{
    cast_shapes_nonlinear_shape_voxels, cast_shapes_nonlinear_voxels_shape,
};
//pub use self::nonlinear_shape_cast_halfspace_support_map::{cast_shapes_nonlinear_halfspace_support_map, cast_shapes_nonlinear_support_map_halfspace};
pub use self::nonlinear_rigid_motion::NonlinearRigidMotion;
pub use self::nonlinear_shape_cast::cast_shapes_nonlinear;
pub use self::nonlinear_shape_cast_support_map_support_map::{
    cast_shapes_nonlinear_support_map_support_map, NonlinearShapeCastMode,
};

#[cfg(feature = "alloc")]
mod nonlinear_shape_cast_composite_shape_shape;
#[cfg(feature = "alloc")]
mod nonlinear_shape_cast_voxels_shape;
//mod cast_shapes_nonlinear_halfspace_support_map;
mod nonlinear_rigid_motion;
mod nonlinear_shape_cast;
mod nonlinear_shape_cast_support_map_support_map;
