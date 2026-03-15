//! Approximate Convex Decomposition using the V-HACD algorithm.
//!
//! # What is Convex Decomposition?
//!
//! Convex decomposition is the process of breaking down a complex, possibly concave shape into
//! multiple simpler convex shapes. A **convex** shape is one where any line segment connecting
//! two points inside the shape lies entirely within the shape (think of shapes without dents
//! or holes). For example:
//! - Convex: sphere, box, cone, tetrahedron
//! - Concave: bowl, donut, 'L' shape, 'C' shape, star
//!
//! # Why Use Convex Decomposition?
//!
//! Convex shapes are much more efficient for collision detection than concave shapes because:
//! 1. **Performance**: Collision algorithms like GJK work directly with convex shapes
//! 2. **Physics Simulation**: Physics engines (like Rapier) can only handle convex shapes
//!    for dynamic objects
//! 3. **Accuracy**: Better than using a single bounding volume or triangle mesh for complex shapes
//!
//! **Example use cases:**
//! - Game character models with complex geometry (humanoids, creatures)
//! - Architectural elements (stairs, furniture, buildings)
//! - Vehicles (cars, planes, spaceships)
//! - Complex terrain features
//!
//! # When to Use Each Approach
//!
//! | Shape Type | Best Approach | Example |
//! |------------|---------------|---------|
//! | Already convex | Use [`convex_hull`](crate::transformation::convex_hull) | Box, sphere, simple pyramid |
//! | Simple concave | Use [`Compound`](crate::shape::Compound) of basic shapes | Two boxes in an 'L' shape |
//! | Complex concave | Use convex decomposition | Character mesh, furniture |
//! | Very detailed mesh | Use [`TriMesh`](crate::shape::TriMesh) for static objects | Detailed terrain, static level geometry |
//!
//! # The V-HACD Algorithm
//!
//! V-HACD (Volumetric Hierarchical Approximate Convex Decomposition) works by:
//! 1. **Voxelization**: Converting the input mesh into a 3D grid of voxels (or 2D pixels)
//! 2. **Decomposition**: Recursively splitting the voxelized shape along planes that minimize concavity
//! 3. **Convex Hull**: Computing the convex hull of each resulting part
//!
//! The algorithm balances:
//! - **Quality**: How closely the parts approximate the original shape (controlled by `concavity`)
//! - **Count**: How many convex parts to generate (controlled by `max_convex_hulls`)
//! - **Performance**: How long the decomposition takes (controlled by `resolution`)
//!
//! # Basic Usage
//!
//! ## Quick Start (Default Parameters)
//!
//! The simplest way to decompose a mesh is using default parameters:
//!
//! ```no_run
//! # #[cfg(all(feature = "dim3", feature = "f32"))] {
//! use parry3d::math::Vector;
//! use parry3d::transformation::vhacd::VHACD;
//! use parry3d::transformation::vhacd::VHACDParameters;
//!
//! // Define a simple concave mesh (an 'L' shape made of triangles)
//! let vertices = vec![
//!     Vector::new(0.0, 0.0, 0.0), Vector::new(2.0, 0.0, 0.0),
//!     Vector::new(2.0, 1.0, 0.0), Vector::new(1.0, 1.0, 0.0),
//!     Vector::new(1.0, 2.0, 0.0), Vector::new(0.0, 2.0, 0.0),
//!     // Add corresponding back face vertices
//!     Vector::new(0.0, 0.0, 1.0), Vector::new(2.0, 0.0, 1.0),
//!     Vector::new(2.0, 1.0, 1.0), Vector::new(1.0, 1.0, 1.0),
//!     Vector::new(1.0, 2.0, 1.0), Vector::new(0.0, 2.0, 1.0),
//! ];
//! let indices = vec![
//!     // Front face triangles
//!     [0, 1, 2], [0, 2, 3], [0, 3, 4], [0, 4, 5],
//!     // Back face triangles
//!     [6, 8, 7], [6, 9, 8], [6, 10, 9], [6, 11, 10],
//!     // Connect front and back
//!     [0, 6, 7], [0, 7, 1], [1, 7, 8], [1, 8, 2],
//!     [2, 8, 9], [2, 9, 3], [3, 9, 10], [3, 10, 4],
//!     [4, 10, 11], [4, 11, 5], [5, 11, 6], [5, 6, 0],
//! ];
//!
//! // Decompose with default parameters
//! let decomposition = VHACD::decompose(
//!     &VHACDParameters::default(),
//!     &vertices,
//!     &indices,
//!     false, // don't keep voxel-to-primitive mapping
//! );
//!
//! // Get the voxelized convex parts
//! let parts = decomposition.voxel_parts();
//! println!("Generated {} convex parts", parts.len());
//!
//! // Compute the convex hulls (for collision detection)
//! let convex_hulls = decomposition.compute_convex_hulls(4);
//! for (i, (vertices, indices)) in convex_hulls.iter().enumerate() {
//!     println!("Part {}: {} vertices, {} triangles", i, vertices.len(), indices.len());
//! }
//! # }
//! ```
//!
//! ## Customizing Parameters
//!
//! For more control over the decomposition quality and performance:
//!
//! ```no_run
//! # #[cfg(all(feature = "dim3", feature = "f32"))] {
//! use parry3d::math::Vector;
//! use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
//! use parry3d::transformation::voxelization::FillMode;
//!
//! # let vertices = vec![
//! #     Vector::new(0.0, 0.0, 0.0), Vector::new(1.0, 0.0, 0.0),
//! #     Vector::new(0.5, 1.0, 0.0), Vector::new(0.5, 0.0, 1.0),
//! # ];
//! # let indices = vec![[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];
//! #
//! // Configure parameters for higher quality decomposition
//! let params = VHACDParameters {
//!     resolution: 128,          // Higher = more detail (but slower)
//!     concavity: 0.001,         // Lower = more parts but better fit
//!     max_convex_hulls: 32,     // Maximum number of convex parts
//!     plane_downsampling: 4,    // Precision of plane search
//!     convex_hull_downsampling: 4, // Precision of convex hull generation
//!     alpha: 0.05,              // Bias toward symmetrical splits
//!     beta: 0.05,               // Bias toward revolution axis splits
//!     convex_hull_approximation: true, // Approximate for speed
//!     fill_mode: FillMode::FloodFill {
//!         detect_cavities: false,
//!     },
//! };
//!
//! let decomposition = VHACD::decompose(&params, &vertices, &indices, false);
//! # }
//! ```
//!
//! ## Working with Original Mesh Geometry
//!
//! By default, the convex hulls are computed from the voxelized representation. To get more
//! accurate hulls based on the original mesh:
//!
//! ```no_run
//! # #[cfg(all(feature = "dim3", feature = "f32"))] {
//! use parry3d::math::Vector;
//! use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
//!
//! # let vertices = vec![
//! #     Vector::new(0.0, 0.0, 0.0), Vector::new(1.0, 0.0, 0.0),
//! #     Vector::new(0.5, 1.0, 0.0), Vector::new(0.5, 0.0, 1.0),
//! # ];
//! # let indices = vec![[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];
//! #
//! // Enable voxel-to-primitive mapping
//! let decomposition = VHACD::decompose(
//!     &VHACDParameters::default(),
//!     &vertices,
//!     &indices,
//!     true, // <-- Keep mapping to original mesh primitives
//! );
//!
//! // Compute exact convex hulls using original mesh geometry
//! let exact_hulls = decomposition.compute_exact_convex_hulls(&vertices, &indices);
//! println!("Generated {} exact convex hulls", exact_hulls.len());
//! # }
//! ```
//!
//! ## 2D Convex Decomposition
//!
//! The same API works in 2D for decomposing polylines:
//!
//! ```no_run
//! # #[cfg(all(feature = "dim2", feature = "f32"))] {
//! use parry2d::math::Vector;
//! use parry2d::transformation::vhacd::{VHACD, VHACDParameters};
//!
//! // Define a concave polyline (e.g., an 'L' shape)
//! let vertices = vec![
//!     Vector::new(0.0, 0.0), Vector::new(2.0, 0.0),
//!     Vector::new(2.0, 1.0), Vector::new(1.0, 1.0),
//!     Vector::new(1.0, 2.0), Vector::new(0.0, 2.0),
//! ];
//! let indices = vec![
//!     [0, 1], [1, 2], [2, 3], [3, 4], [4, 5], [5, 0],
//! ];
//!
//! let decomposition = VHACD::decompose(
//!     &VHACDParameters::default(),
//!     &vertices,
//!     &indices,
//!     false,
//! );
//!
//! // Get convex polygons
//! let convex_polygons = decomposition.compute_convex_hulls(4);
//! for (i, polygon) in convex_polygons.iter().enumerate() {
//!     println!("Polygon {}: {} vertices", i, polygon.len());
//! }
//! # }
//! ```
//!
//! # Integration with Physics Engines
//!
//! The decomposed convex parts can be used directly with physics engines:
//!
//! ```no_run
//! # #[cfg(all(feature = "dim3", feature = "f32"))] {
//! use parry3d::math::Vector;
//! use parry3d::shape::{SharedShape, Compound};
//! use parry3d::transformation::vhacd::VHACDParameters;
//! use parry3d::math::Pose;
//!
//! # let vertices = vec![
//! #     Vector::new(0.0, 0.0, 0.0), Vector::new(1.0, 0.0, 0.0),
//! #     Vector::new(0.5, 1.0, 0.0), Vector::new(0.5, 0.0, 1.0),
//! # ];
//! # let indices = vec![[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];
//! #
//! // Use the high-level API for direct integration
//! let compound_shape = SharedShape::convex_decomposition(&vertices, &indices);
//!
//! // Or with custom parameters
//! let params = VHACDParameters {
//!     concavity: 0.001,
//!     max_convex_hulls: 32,
//!     ..Default::default()
//! };
//! let compound_shape = SharedShape::convex_decomposition_with_params(
//!     &vertices,
//!     &indices,
//!     &params,
//! );
//!
//! // The resulting compound can be used as a collider shape
//! println!("Created compound shape with convex parts");
//! # }
//! ```
//!
//! # Performance Tips
//!
//! 1. **Resolution**: Start with lower values (32-64) for fast iteration, increase for final quality
//! 2. **Concavity**: Higher values (0.01-0.1) = fewer parts = faster but less accurate
//! 3. **Max Convex Hulls**: Limit to reasonable values (8-32) for game objects
//! 4. **Approximation**: Keep `convex_hull_approximation: true` for better performance
//! 5. **Preprocessing**: Simplify your mesh before decomposition if it has excessive detail
//!
//! # Debugging Tips
//!
//! If the decomposition doesn't look right:
//! - Visualize the voxelized parts using `voxel_parts()` to understand the algorithm's view
//! - Try adjusting `resolution` if details are lost or voxelization is too coarse
//! - Increase `max_convex_hulls` if the shape is still too concave
//! - Decrease `concavity` for tighter fitting convex parts
//! - Check that your mesh is manifold and has consistent winding order
//!
//! # Algorithm Details
//!
//! The V-HACD algorithm was developed by Khaled Mamou and is described in:
//! > "Volumetric Hierarchical Approximate Convex Decomposition"
//! > Khaled Mamou and Faouzi Ghorbel
//!
//! Implementation based on: <https://github.com/kmammou/v-hacd>
//!
//! # See Also
//!
//! - [`VHACDParameters`](crate::transformation::vhacd::VHACDParameters): Configuration parameters for the algorithm
//! - [`VHACD`](crate::transformation::vhacd::VHACD): The main decomposition structure
//! - [`SharedShape::convex_decomposition`](crate::shape::SharedShape::convex_decomposition): High-level API
//! - [`convex_hull`](crate::transformation::convex_hull): For computing convex hulls directly
//! - [`Compound`](crate::shape::Compound): For manually combining convex shapes

pub use self::parameters::VHACDParameters;
pub use self::vhacd::VHACD;

pub(crate) use self::vhacd::CutPlane;

mod parameters;
mod vhacd;
