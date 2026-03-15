use crate::mass_properties::MassProperties;
use crate::math::{Real, Vector, DIM};

impl MassProperties {
    /// Computes the mass properties of a convex polyhedron (3D) or polygon (2D).
    ///
    /// A convex polyhedron is a 3D solid where all faces are flat and all vertices point
    /// outward. This is a convenience function that delegates to `from_trimesh()`, which
    /// handles the actual computation by treating the polyhedron as a triangle mesh.
    ///
    /// # Arguments
    ///
    /// * `density` - The material density
    ///   - In 3D: kg/m³ (mass per unit volume)
    ///   - In 2D: kg/m² (mass per unit area)
    /// * `vertices` - Array of vertex positions defining the polyhedron
    /// * `indices` - Array of triangle indices
    ///   - In 3D: Each element is `[u32; 3]` indexing into vertices array
    ///   - In 2D: Each element is `[u32; 2]` for line segments
    ///
    /// # Returns
    ///
    /// A `MassProperties` struct containing:
    /// - **mass**: Total mass calculated from volume/area and density
    /// - **local_com**: Center of mass (volume-weighted centroid)
    /// - **inv_principal_inertia**: Inverse angular inertia tensor
    ///
    /// # Example (3D) - Tetrahedron
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::math::Vector;
    ///
    /// // Create a regular tetrahedron (4 vertices, 4 triangular faces)
    /// let vertices = vec![
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0),
    ///     Vector::ZERO,
    /// ];
    ///
    /// let indices = vec![
    ///     [0, 1, 2], // Face 1
    ///     [0, 1, 3], // Face 2
    ///     [0, 2, 3], // Face 3
    ///     [1, 2, 3], // Face 4
    /// ];
    ///
    /// let density = 1000.0;
    /// let tetra_props = MassProperties::from_convex_polyhedron(density, &vertices, &indices);
    ///
    /// println!("Tetrahedron mass: {:.4} kg", tetra_props.mass());
    /// println!("Center of mass: {:?}", tetra_props.local_com);
    /// # }
    /// ```
    ///
    /// # Example (3D) - Octahedron
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::math::Vector;
    ///
    /// // Regular octahedron (6 vertices, 8 triangular faces)
    /// let vertices = vec![
    ///     Vector::new(1.0, 0.0, 0.0),   // +X
    ///     Vector::new(-1.0, 0.0, 0.0),  // -X
    ///     Vector::new(0.0, 1.0, 0.0),   // +Y
    ///     Vector::new(0.0, -1.0, 0.0),  // -Y
    ///     Vector::new(0.0, 0.0, 1.0),   // +Z
    ///     Vector::new(0.0, 0.0, -1.0),  // -Z
    /// ];
    ///
    /// let indices = vec![
    ///     [0, 2, 4], [0, 4, 3], [0, 3, 5], [0, 5, 2],  // Right hemisphere
    ///     [1, 4, 2], [1, 3, 4], [1, 5, 3], [1, 2, 5],  // Left hemisphere
    /// ];
    ///
    /// let density = 800.0;
    /// let octa_props = MassProperties::from_convex_polyhedron(density, &vertices, &indices);
    ///
    /// println!("Octahedron mass: {:.2} kg", octa_props.mass());
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Custom 3D shapes**: Game objects with specific polyhedral geometry
    /// - **Crystalline structures**: Geometric solids (tetrahedra, octahedra, dodecahedra)
    /// - **Simplified models**: Convex approximations of complex shapes
    /// - **Gems and jewels**: Faceted objects
    /// - **Dice**: Polyhedral game dice (d4, d6, d8, d12, d20)
    ///
    /// # Requirements
    ///
    /// - **Must be convex**: All faces must be flat and point outward
    /// - **Closed mesh**: Triangles must form a watertight volume
    /// - **Consistent winding**: Triangle vertices should follow consistent order
    ///   (counter-clockwise when viewed from outside)
    /// - **Valid indices**: All index values must be < vertices.len()
    ///
    /// # Convex vs Non-Convex
    ///
    /// This function assumes the shape is convex. For non-convex (concave) meshes:
    /// - The result may be incorrect
    /// - Consider using `from_compound()` with a convex decomposition
    /// - Or use `from_trimesh()` directly (handles both convex and concave)
    ///
    /// # Implementation Note
    ///
    /// This function is a thin wrapper around `from_trimesh()`. Both produce identical
    /// results. Use `from_convex_polyhedron()` when you know the shape is convex to
    /// make intent clear in code.
    ///
    /// # See Also
    ///
    /// - `from_trimesh()`: For general triangle meshes (convex or concave)
    /// - `from_convex_polygon()`: For 2D convex polygons
    /// - `from_compound()`: For combining multiple convex shapes
    pub fn from_convex_polyhedron(
        density: Real,
        vertices: &[Vector],
        indices: &[[u32; DIM]],
    ) -> MassProperties {
        Self::from_trimesh(density, vertices, indices)
    }
}
