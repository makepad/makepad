//! Application of the Separating Axis Theorem (SAT) for collision detection.
//!
//! # What is the Separating Axis Theorem?
//!
//! The **Separating Axis Theorem (SAT)** is a fundamental geometric principle used in collision
//! detection. It states that two convex shapes do **not** intersect if and only if there exists
//! an axis (a line) onto which the projections of the two shapes do not overlap.
//!
//! In simpler terms: if you can find a direction where, when you "shine a light" from that direction
//! and look at the shadows cast by both shapes, those shadows don't overlap, then the shapes
//! are not colliding.
//!
//! # How Does SAT Work?
//!
//! SAT works by testing a finite set of candidate axes to find a separating axis:
//!
//! 1. **Select candidate axes**: For polygons/polyhedra, these are typically:
//!    - Face normals (perpendicular to each face)
//!    - Edge cross products (perpendicular to pairs of edges from each shape, in 3D)
//!
//! 2. **Project both shapes onto each axis**: Find the extent (min and max) of each shape
//!    along the axis by computing support points (the furthest points in that direction).
//!
//! 3. **Check for overlap**: If the projections don't overlap on any axis, the shapes don't collide.
//!    If all axes show overlap, the shapes are intersecting.
//!
//! 4. **Find minimum separation**: The axis with the smallest overlap (or largest separation)
//!    is the "best" separating axis, useful for computing penetration depth and contact normals.
//!
//! # When is SAT Used?
//!
//! SAT is particularly effective for:
//!
//! - **Convex polygonal shapes**: Cuboids (boxes), triangles, convex polygons/polyhedra
//! - **Accurate contact information**: SAT can provide exact penetration depth and contact normals
//! - **Shallow penetrations**: Works best when shapes are just touching or slightly overlapping
//! - **Edge-edge contacts**: In 3D, SAT naturally handles edge-edge collisions between polyhedra
//!
//! Parry uses SAT alongside other algorithms:
//! - **GJK**: For general convex shapes (faster for distance queries, but less accurate for contacts)
//! - **EPA**: For penetration depth when GJK detects overlap (but SAT is often more accurate)
//! - **Specialized algorithms**: For specific shape pairs (sphere-sphere, etc.)
//!
//! # Example: Cuboid-Cuboid Collision
//!
//! ```rust
//! # #[cfg(all(feature = "dim3", feature = "f32"))] {
//! use parry3d::shape::Cuboid;
//! use parry3d::query::sat::*;
//! use parry3d::math::{Pose, Vector};
//!
//! // Create two boxes
//! let box1 = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
//! let box2 = Cuboid::new(Vector::new(0.5, 0.5, 0.5));
//!
//! // Position them close together
//! let pos12 = Pose::translation(1.5, 0.0, 0.0);
//!
//! // Test face normals from box1
//! let (separation, _normal) = cuboid_cuboid_find_local_separating_normal_oneway(
//!     &box1, &box2, &pos12
//! );
//!
//! if separation > 0.0 {
//!     println!("Boxes are separated by {}", separation);
//! } else {
//!     println!("Boxes are overlapping by {}", -separation);
//! }
//! # }
//! ```
//!
//! # Module Organization
//!
//! This module provides SAT implementations for common shape pairs:
//!
//! - **Cuboid-Cuboid**: Box vs box collision (2D and 3D)
//! - **Cuboid-Triangle**: Box vs triangle (2D and 3D)
//! - **Cuboid-Segment**: Box vs line segment (2D and 3D)
//! - **Cuboid-SupportMap**: Box vs any convex shape
//! - **Triangle-Segment**: Triangle vs line segment (3D only)
//! - **SupportMap-SupportMap**: Generic convex shape vs convex shape
//!
//! Each module provides functions to:
//! - Find the best separating normal (testing face normals)
//! - Find the best separating edge (testing edge cross products, 3D only)
//! - Compute separation distance along a given axis

pub use self::sat_cuboid_cuboid::*;
pub use self::sat_cuboid_point::*;
pub use self::sat_cuboid_segment::*;
pub use self::sat_cuboid_support_map::*;
pub use self::sat_cuboid_triangle::*;
pub use self::sat_support_map_support_map::*;
#[cfg(feature = "dim3")]
pub use self::sat_triangle_segment::*;
// pub use self::sat_polygon_polygon::*;

mod sat_cuboid_cuboid;
mod sat_cuboid_point;
mod sat_cuboid_segment;
mod sat_cuboid_support_map;
mod sat_cuboid_triangle;
mod sat_support_map_support_map;
#[cfg(feature = "dim3")]
mod sat_triangle_segment;
// mod sat_polygon_polygon;
