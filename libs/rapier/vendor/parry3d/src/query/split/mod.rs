//! Shape splitting and plane intersection operations.
//!
//! # Overview
//!
//! This module provides functionality for splitting geometric shapes with planes and computing
//! plane-shape intersections. These operations are fundamental for various geometric algorithms
//! including:
//!
//! - **Spatial partitioning**: Dividing space with axis-aligned or oriented planes
//! - **CSG (Constructive Solid Geometry)**: Boolean operations on meshes
//! - **Mesh processing**: Cutting and slicing operations on 3D models
//! - **Cross-sections**: Computing 2D slices of 3D geometry
//! - **BVH construction**: Building spatial acceleration structures
//!
//! # What is Shape Splitting?
//!
//! Shape splitting is the process of dividing a geometric shape into two or more pieces using
//! a plane. The plane acts as a "cutting tool" that partitions the shape based on which side
//! of the plane each part lies on:
//!
//! - **Negative half-space**: Vectors where the dot product with the plane normal is less than the bias
//! - **Positive half-space**: Vectors where the dot product with the plane normal is greater than the bias
//! - **On the plane**: Vectors that lie exactly on the plane (within an epsilon tolerance)
//!
//! ## Plane Definition
//!
//! A plane is defined by:
//! - A **normal vector** (unit vector perpendicular to the plane)
//! - A **bias** (signed distance from the origin along the normal)
//!
//! The plane equation is: `normal · point = bias`
//!
//! For canonical splits, the normal is aligned with a coordinate axis (e.g., X, Y, or Z axis),
//! making the operation axis-aligned.
//!
//! # When to Use Shape Splitting
//!
//! Shape splitting is useful in many scenarios:
//!
//! ## 1. Spatial Partitioning
//! When building spatial data structures like octrees or k-d trees, you need to divide space
//! recursively. Shape splitting determines which objects or parts of objects belong in each
//! partition.
//!
//! ```rust
//! # #[cfg(all(feature = "dim3", feature = "f32"))] {
//! use parry3d::bounding_volume::Aabb;
//! use parry3d::math::Vector;
//! use parry3d::query::SplitResult;
//!
//! // Split an AABB along the X-axis at x = 5.0
//! let aabb = Aabb::new(Vector::new(0.0, 0.0, 0.0), Vector::new(10.0, 10.0, 10.0));
//! match aabb.canonical_split(0, 5.0, 1e-6) {
//!     SplitResult::Pair(left, right) => {
//!         // AABB was split into two pieces
//!         // left contains points with x < 5.0
//!         // right contains points with x > 5.0
//!     }
//!     SplitResult::Negative => {
//!         // Entire AABB is on the negative side (x < 5.0)
//!     }
//!     SplitResult::Positive => {
//!         // Entire AABB is on the positive side (x > 5.0)
//!     }
//! }
//! # }
//! ```
//!
//! ## 2. Mesh Slicing
//! For 3D modeling, CAD applications, or game engines, you might need to cut meshes with
//! planes. This is useful for effects like slicing objects with swords, laser cuts, or
//! architectural cross-sections.
//!
//! ```rust
//! # #[cfg(all(feature = "dim3", feature = "spade", feature = "f32"))]
//! # {
//! use parry3d::shape::TriMesh;
//! use parry3d::math::Vector;
//! use parry3d::query::SplitResult;
//!
//! # let vertices = vec![
//! #     Vector::new(0.0, 0.0, 0.0),
//! #     Vector::new(1.0, 0.0, 0.0),
//! #     Vector::new(0.0, 1.0, 0.0),
//! # ];
//! # let indices = vec![[0u32, 1, 2]];
//! # let mesh = TriMesh::new(vertices, indices).unwrap();
//! // Split a mesh along the Z-axis at z = 0.5
//! match mesh.canonical_split(2, 0.5, 1e-6) {
//!     SplitResult::Pair(bottom_half, top_half) => {
//!         // Mesh was split into two separate meshes
//!         // bottom_half contains the part with z < 0.5
//!         // top_half contains the part with z > 0.5
//!         // Both meshes are "capped" with new triangles on the cutting plane
//!     }
//!     SplitResult::Negative => {
//!         // Entire mesh is below z = 0.5
//!     }
//!     SplitResult::Positive => {
//!         // Entire mesh is above z = 0.5
//!     }
//! }
//! # }
//! ```
//!
//! ## 3. Cross-Section Computation
//! Computing the intersection of a mesh with a plane gives you a 2D cross-section, useful
//! for visualization, analysis, or generating contours.
//!
// FIXME: this loops indefinitely.
// //! ```rust no_run
// //! # #[cfg(all(feature = "dim3", feature = "spade", feature = "f32"))]
// //! # {
// //! use parry3d::shape::TriMesh;
// //! use parry3d::query::IntersectResult;
// //!
// //! # let vertices = vec![
// //! #     parry3d::math::Vector::ZERO,
// //! #     parry3d::math::Vector::new(1.0, 0.0, 0.0),
// //! #     parry3d::math::Vector::new(0.5, 1.0, 0.5),
// //! #     parry3d::math::Vector::new(0.5, 0.0, 1.0),
// //! # ];
// //! # let indices = vec![[0u32, 1, 2], [0, 1, 3]];
// //! # let mesh = TriMesh::new(vertices, indices).unwrap();
// //! // Get the cross-section of a mesh at z = 0.5
// //! match mesh.canonical_intersection_with_plane(2, 0.5, 1e-6) {
// //!     IntersectResult::Intersect(polyline) => {
// //!         // polyline contains the 2D outline of the mesh at z = 0.5
// //!         // This can have multiple connected components if the mesh
// //!         // has holes or multiple separate pieces at this height
// //!     }
// //!     IntersectResult::Negative => {
// //!         // Mesh doesn't intersect the plane; it's entirely on the negative side
// //!     }
// //!     IntersectResult::Positive => {
// //!         // Mesh doesn't intersect the plane; it's entirely on the positive side
// //!     }
// //! }
// //! # }
// //! ```
//!
//! # Supported Shapes
//!
//! Currently, the following shapes support splitting operations:
//!
//! - [`Aabb`](crate::bounding_volume::Aabb) - Axis-aligned bounding boxes
//! - [`Segment`](crate::shape::Segment) - Line segments (in 2D and 3D)
//! - [`TriMesh`](crate::shape::TriMesh) - Triangle meshes (3D only, requires `spade` feature)
//!
//! # Result Types
//!
//! The module provides two result types to represent the outcomes of splitting operations:
//!
//! - [`SplitResult`]: For operations that split a shape into pieces
//! - [`IntersectResult`]: For operations that compute the intersection with a plane
//!
//! Both types use an enum to efficiently represent the three possible outcomes without
//! unnecessary allocations when a split doesn't occur.
//!
//! # Epsilon Tolerance
//!
//! All splitting operations accept an `epsilon` parameter to handle floating-point precision
//! issues. Vectors within `epsilon` distance of the plane are considered to lie on the plane.
//! A typical value is `1e-6` for single precision (`f32`) or `1e-10` for double precision (`f64`).
//!
//! Choosing an appropriate epsilon is important:
//! - **Too small**: May cause numerical instability or incorrect classifications
//! - **Too large**: May merge distinct features or produce incorrect topology
//!
//! # Implementation Notes
//!
//! - **Segment splitting** is implemented for both 2D and 3D
//! - **AABB splitting** preserves axis-alignment and is very efficient
//! - **TriMesh splitting** (3D only) uses Delaunay triangulation to properly cap the mesh
//!   where it's cut, ensuring the result is a valid closed mesh if the input was closed
//! - All operations are deterministic given the same input and epsilon value

pub use self::split::{IntersectResult, SplitResult};

mod split;
mod split_aabb;
mod split_segment;

#[cfg(all(feature = "dim3", feature = "spade"))]
mod split_trimesh;
