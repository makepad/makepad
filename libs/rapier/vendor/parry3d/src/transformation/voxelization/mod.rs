//! Voxelization of 2D and 3D shapes.
//!
//! # What is Voxelization?
//!
//! Voxelization is the process of converting a continuous geometric shape (like a triangle mesh in 3D
//! or a polyline in 2D) into a discrete grid of cubic cells called **voxels**. Think of voxels as
//! 3D pixels - just like pixels make up 2D images, voxels make up 3D volumes. In 2D, voxels are
//! actually square cells (sometimes called "pixels" in other contexts).
//!
//! This is similar to how a photograph discretizes a continuous scene into pixels, except voxelization
//! works in 3D space (or 2D for 2D shapes).
//!
//! # When Should You Use Voxelization?
//!
//! Voxelization is useful when you need to:
//!
//! - **Approximate complex shapes** with a simpler grid-based representation
//! - **Compute volume** of complex meshes (by counting filled voxels)
//! - **Perform spatial queries** efficiently using a regular grid structure
//! - **Generate collision proxies** for physics simulations (using the V-HACD algorithm)
//! - **Simplify mesh operations** like boolean operations or distance field computation
//! - **Analyze shape properties** like connectivity, cavities, or internal structure
//!
//! # Key Concepts
//!
//! ## Voxel Grid
//!
//! A voxel grid divides space into a regular array of cubic (or square in 2D) cells. Each cell is
//! identified by integer coordinates `(i, j)` in 2D or `(i, j, k)` in 3D. The physical size of each
//! voxel is controlled by the `scale` parameter.
//!
//! ## Resolution vs Voxel Size
//!
//! You can control the granularity of voxelization in two ways:
//!
//! - **By resolution**: Specify how many subdivisions to make along the longest axis. The voxel size
//!   is automatically computed to maintain cubic voxels.
//! - **By voxel size**: Directly specify the physical size of each voxel. The resolution is
//!   automatically computed based on the shape's bounding box.
//!
//! Higher resolution (or smaller voxel size) gives more accurate approximation but uses more memory.
//!
//! ## Fill Modes
//!
//! The [`FillMode`](crate::transformation::voxelization::FillMode) enum controls which voxels are considered "filled":
//!
//! - **`SurfaceOnly`**: Only voxels that intersect the surface boundary are marked as filled.
//!   This gives you a hollow shell representation.
//!
//! - **`FloodFill`**: Fills voxels on the surface AND all voxels inside the volume. This uses a
//!   flood-fill algorithm starting from outside the shape. Options include:
//!   - `detect_cavities`: Properly handles internal holes/cavities
//!   - `detect_self_intersections` (2D only): Attempts to handle self-intersecting boundaries
//!
//! # Main Types
//!
//! - [`VoxelSet`](crate::transformation::voxelization::VoxelSet): A sparse set containing only filled voxels (memory-efficient output format)
//! - [`VoxelizedVolume`](crate::transformation::voxelization::VoxelizedVolume): A dense 3D grid of all voxels (intermediate format used during voxelization)
//! - [`Voxel`](crate::transformation::voxelization::Voxel): A single voxel with its grid coordinates and metadata
//! - [`FillMode`](crate::transformation::voxelization::FillMode): Configuration for how to determine filled vs empty voxels
//! - [`VoxelValue`](crate::transformation::voxelization::VoxelValue): The state of a voxel (inside, outside, or on surface)
//!
//! # Examples
//!
//! ## Basic 3D Voxelization (Surface Only)
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))]
//! # {
//! use parry3d::transformation::voxelization::{FillMode, VoxelSet};
//! use parry3d::shape::Cuboid;
//! use parry3d::math::Vector;
//!
//! // Create a simple cuboid and convert it to a triangle mesh
//! let cuboid = Cuboid::new(Vector::new(1.0, 0.5, 0.3));
//! let (vertices, indices) = cuboid.to_trimesh();
//!
//! // Voxelize with resolution of 10 voxels along the longest axis
//! let voxels = VoxelSet::voxelize(
//!     &vertices,
//!     &indices,
//!     10,                          // resolution
//!     FillMode::SurfaceOnly,       // only surface voxels
//!     false,                       // don't track voxel-to-triangle mapping
//! );
//!
//! println!("Created {} surface voxels", voxels.len());
//! println!("Each voxel has volume: {}", voxels.voxel_volume());
//!
//! // Access individual voxels
//! for voxel in voxels.voxels() {
//!     println!("Voxel at grid position: {:?}", voxel);
//!     println!("  World position: {:?}", voxels.get_voxel_point(voxel));
//!     println!("  On surface: {}", voxel.is_on_surface);
//! }
//! # }
//! ```
//!
//! ## 3D Voxelization with Interior Fill
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))]
//! # {
//! use parry3d::transformation::voxelization::{FillMode, VoxelSet};
//! use parry3d::shape::Ball;
//! use parry3d::math::Vector;
//!
//! // Create a sphere mesh
//! let ball = Ball::new(1.0);
//! let (vertices, indices) = ball.to_trimesh(20, 20); // subdivisions for sphere
//!
//! // Voxelize with flood-fill to get solid interior
//! let voxels = VoxelSet::voxelize(
//!     &vertices,
//!     &indices,
//!     15,                                              // resolution
//!     FillMode::FloodFill {
//!         detect_cavities: false,                      // simple solid shape, no cavities
//!     },
//!     false,
//! );
//!
//! // Compute the approximate volume
//! let volume = voxels.compute_volume();
//! let expected_volume = 4.0 / 3.0 * std::f32::consts::PI * 1.0_f32.powi(3);
//! println!("Voxel volume: {}, Expected: {}", volume, expected_volume);
//!
//! // Count surface vs interior voxels
//! let surface_count = voxels.voxels().iter().filter(|v| v.is_on_surface).count();
//! let interior_count = voxels.voxels().iter().filter(|v| !v.is_on_surface).count();
//! println!("Surface voxels: {}, Interior voxels: {}", surface_count, interior_count);
//! # }
//! ```
//!
//! ## 2D Voxelization of a Polygon
//!
//! ```
//! # #[cfg(all(feature = "dim2", feature = "f32"))]
//! # {
//! use parry2d::transformation::voxelization::{FillMode, VoxelSet};
//! use parry2d::math::Vector;
//!
//! // Define a simple square as a polyline (boundary)
//! let vertices = vec![
//!     Vector::ZERO,
//!     Vector::new(2.0, 0.0),
//!     Vector::new(2.0, 2.0),
//!     Vector::new(0.0, 2.0),
//! ];
//!
//! // Create index buffer for the polyline segments (closed loop)
//! let indices = vec![
//!     [0, 1],  // bottom edge
//!     [1, 2],  // right edge
//!     [2, 3],  // top edge
//!     [3, 0],  // left edge
//! ];
//!
//! // Voxelize with specific voxel size
//! let voxels = VoxelSet::with_voxel_size(
//!     &vertices,
//!     &indices,
//!     0.2,                                             // voxel size
//!     FillMode::FloodFill {
//!         detect_cavities: false,
//!         detect_self_intersections: false,
//!     },
//!     false,
//! );
//!
//! println!("2D voxel area: {}", voxels.compute_volume());
//! # }
//! ```
//!
//! ## Computing Convex Hull from Voxels
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))]
//! # {
//! use parry3d::transformation::voxelization::{FillMode, VoxelSet};
//! use parry3d::shape::Cuboid;
//! use parry3d::math::Vector;
//!
//! let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
//! let (vertices, indices) = cuboid.to_trimesh();
//!
//! let voxels = VoxelSet::voxelize(
//!     &vertices,
//!     &indices,
//!     8,
//!     FillMode::SurfaceOnly,
//!     false,
//! );
//!
//! // Compute convex hull from the voxel centers
//! // sampling=1 means use every voxel, higher values skip voxels for performance
//! let (hull_vertices, hull_indices) = voxels.compute_convex_hull(1);
//!
//! println!("Convex hull has {} vertices and {} triangles",
//!          hull_vertices.len(), hull_indices.len());
//! # }
//! ```
//!
//! ## Tracking Which Triangles Intersect Each Voxel
//!
//! ```
//! # #[cfg(all(feature = "dim3", feature = "f32"))]
//! # {
//! use parry3d::transformation::voxelization::{FillMode, VoxelSet};
//! use parry3d::shape::Cuboid;
//! use parry3d::math::Vector;
//!
//! let cuboid = Cuboid::new(Vector::new(1.0, 1.0, 1.0));
//! let (vertices, indices) = cuboid.to_trimesh();
//!
//! // Enable voxel-to-primitives mapping
//! let voxels = VoxelSet::voxelize(
//!     &vertices,
//!     &indices,
//!     10,
//!     FillMode::SurfaceOnly,
//!     true,  // keep_voxel_to_primitives_map = true
//! );
//!
//! // Now we can compute exact intersections between voxels and triangles
//! let intersection_points = voxels.compute_primitive_intersections(&vertices, &indices);
//! println!("Generated {} intersection points", intersection_points.len());
//!
//! // Or compute a more accurate convex hull using the triangle intersections
//! let (hull_verts, hull_indices) = voxels.compute_exact_convex_hull(&vertices, &indices);
//! println!("Exact convex hull: {} vertices", hull_verts.len());
//! # }
//! ```
//!
//! # Performance Considerations
//!
//! - **Resolution**: Doubling the resolution increases memory usage by 4× in 2D or 8× in 3D
//! - **VoxelSet vs VoxelizedVolume**: `VoxelSet` is sparse (only stores filled voxels), while
//!   `VoxelizedVolume` is dense (stores all grid cells). For sparse shapes, `VoxelSet` is much
//!   more memory-efficient.
//! - **Voxel-to-primitives map**: Setting `keep_voxel_to_primitives_map = true` adds memory overhead
//!   but enables more precise operations like `compute_exact_convex_hull()`
//! - **Flood-fill**: Using `FloodFill` mode is more expensive than `SurfaceOnly` but gives you
//!   solid volumes instead of hollow shells
//!
//! # Use in Convex Decomposition
//!
//! The voxelization system is used internally by Parry's V-HACD implementation for approximate
//! convex decomposition. See the [`vhacd`](crate::transformation::vhacd) module for details.

pub use self::voxel_set::{Voxel, VoxelSet};
pub use self::voxelized_volume::{FillMode, VoxelValue, VoxelizedVolume};

mod voxel_set;
mod voxelized_volume;
