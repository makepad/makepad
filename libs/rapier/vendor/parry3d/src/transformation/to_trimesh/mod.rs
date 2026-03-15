//! Triangle mesh generation from geometric shapes.
//!
//! This module provides functionality to convert various geometric shapes into triangle meshes
//! (also called "trimesh" in Parry's nomenclature). A triangle mesh is represented as a pair
//! of vectors: `(Vec<Vector>, Vec<[u32; 3]>)` containing vertices and triangle indices.
//!
//! # Overview
//!
//! Each shape in Parry implements a `to_trimesh()` method that discretizes its boundary into
//! a triangle mesh. This is useful for:
//! - **Visualization**: Rendering shapes in graphics applications
//! - **Export**: Converting shapes to standard mesh formats (OBJ, STL, etc.)
//! - **Physics simulation**: Converting analytical shapes to mesh-based collision detection
//! - **Mesh processing**: Using shape primitives as building blocks for complex geometry
//!
//! # Supported Shapes
//!
//! ## 2D and 3D Shapes
//! - **Cuboid**: Axis-aligned box with rectangular faces
//! - **Aabb**: Axis-aligned bounding box (uses `Cuboid` internally)
//!
//! ## 3D-Only Shapes
//! - **Ball**: Sphere discretized into triangular patches
//! - **Capsule**: Cylinder with hemispherical caps
//! - **Cylinder**: Circular cylinder with flat caps
//! - **Cone**: Circular cone with flat base
//! - **ConvexPolyhedron**: Already a mesh, returns its triangles
//! - **HeightField**: Terrain height map converted to mesh
//! - **Voxels**: Voxel grid converted to boundary mesh
//!
//! # Mesh Quality Control
//!
//! Most curved shapes (Ball, Capsule, Cylinder, Cone) accept subdivision parameters that
//! control the mesh resolution. Higher subdivision values produce smoother but heavier meshes:
//! - **Low subdivision** (4-8): Fast, angular approximation
//! - **Medium subdivision** (16-32): Good balance for most uses
//! - **High subdivision** (64+): Smooth curves, high polygon count
//!
//! # Examples
//!
//! ## Basic Mesh Generation
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))]
//! # {
//! use parry3d::shape::{Ball, Cuboid};
//! use parry3d::math::Vector;
//!
//! // Convert a cuboid to a triangle mesh
//! let cuboid = Cuboid::new(Vector::new(1.0, 2.0, 3.0));
//! let (vertices, indices) = cuboid.to_trimesh();
//!
//! // A cuboid has 8 vertices and 12 triangles (2 per face × 6 faces)
//! assert_eq!(vertices.len(), 8);
//! assert_eq!(indices.len(), 12);
//!
//! // Convert a sphere with medium resolution
//! let ball = Ball::new(5.0);
//! let (vertices, indices) = ball.to_trimesh(16, 32);
//! // 16 theta subdivisions × 32 phi subdivisions
//! println!("Ball mesh: {} vertices, {} triangles",
//!          vertices.len(), indices.len());
//! # }
//! ```
//!
//! ## Controlling Subdivision Quality
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))]
//! # {
//! use parry3d::shape::Cylinder;
//!
//! let cylinder = Cylinder::new(2.0, 1.0); // half-height=2.0, radius=1.0
//!
//! // Low quality - faster, fewer triangles
//! let (vertices_low, indices_low) = cylinder.to_trimesh(8);
//! println!("Low quality: {} triangles", indices_low.len());
//!
//! // High quality - slower, more triangles, smoother appearance
//! let (vertices_high, indices_high) = cylinder.to_trimesh(64);
//! println!("High quality: {} triangles", indices_high.len());
//! # }
//! ```
//!
//! ## Converting Height Fields
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))]
//! # {
//! use parry3d::shape::HeightField;
//! use parry3d::math::Vector;
//! use parry3d::utils::Array2;
//!
//! // Create a simple 3×3 height field
//! // Column-major order: column 0 first, then column 1, then column 2
//! let heights = Array2::new(3, 3, vec![
//!     0.0, 1.0, 0.0,  // column 0
//!     1.0, 2.0, 1.0,  // column 1
//!     0.0, 1.0, 0.0,  // column 2
//! ]);
//!
//! let heightfield = HeightField::new(heights, Vector::new(10.0, 10.0, 1.0));
//! let (vertices, indices) = heightfield.to_trimesh();
//!
//! // Height fields generate 2 triangles per grid cell
//! // (3-1) × (3-1) cells × 2 triangles = 8 triangles
//! assert_eq!(indices.len(), 8);
//! # }
//! ```
//!
//! # Return Format
//!
//! All `to_trimesh()` methods return a tuple `(Vec<Vector>, Vec<[u32; 3]>)`:
//!
//! - **Vertices** (`Vec<Vector>`): Array of 3D points (or 2D for `dim2` feature)
//! - **Indices** (`Vec<[u32; 3]>`): Array of triangle indices, where each `[u32; 3]` contains
//!   three indices into the vertices array
//!
//! The triangles follow a **counter-clockwise winding order** when viewed from outside the shape,
//! which is the standard convention for outward-facing normals.
//!
//! # Performance Considerations
//!
//! - Triangle mesh generation allocates new memory each time
//! - For dynamic scenes, consider caching generated meshes
//! - Subdivision parameters have quadratic or cubic impact on triangle count
//! - Simple shapes (Cuboid, ConvexPolyhedron) have negligible generation cost
//! - Complex shapes (Ball, Capsule with high subdivision) can be expensive
//!
//! # See Also
//!
//! - \`to_polyline\` - 2D shape to polyline conversion (internal, not public API)
#![cfg_attr(
    feature = "dim3",
    doc = "//! - `to_outline` - 3D shape outline generation (edge wireframes)"
)]
//! - [`TriMesh`](crate::shape::TriMesh) - Triangle mesh shape type

#[cfg(feature = "dim3")]
mod ball_to_trimesh;
#[cfg(feature = "dim3")]
mod capsule_to_trimesh;
#[cfg(feature = "dim3")]
mod cone_to_trimesh;
#[cfg(feature = "dim3")]
mod convex_polyhedron_to_trimesh;
mod cuboid_to_trimesh;
#[cfg(feature = "dim3")]
mod cylinder_to_trimesh;
#[cfg(feature = "dim3")]
mod heightfield_to_trimesh;
#[cfg(feature = "dim3")]
mod voxels_to_trimesh;
