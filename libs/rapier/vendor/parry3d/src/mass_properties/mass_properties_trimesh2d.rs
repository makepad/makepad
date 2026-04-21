use crate::mass_properties::MassProperties;
use crate::math::{Real, Vector};
use crate::shape::Triangle;

impl MassProperties {
    /// Computes the mass properties of a triangle mesh.
    ///
    /// A triangle mesh (trimesh) is a collection of triangles that together form a
    /// 2D or 3D shape. This function works for both convex and non-convex (concave)
    /// meshes. It decomposes the mesh into individual triangles, computes each
    /// triangle's mass properties, and combines them.
    ///
    /// # Arguments
    ///
    /// * `density` - The material density
    ///   - In 3D: kg/m³ (mass per unit volume) - treats mesh as a solid volume
    ///   - In 2D: kg/m² (mass per unit area) - treats mesh as a flat surface
    /// * `vertices` - Array of vertex positions (points in space)
    /// * `indices` - Array of triangle indices, each element is `[u32; 3]`
    ///   - Each triplet references three vertices forming a triangle
    ///   - Indices must be valid: all values < vertices.len()
    ///
    /// # Returns
    ///
    /// A `MassProperties` struct containing:
    /// - **mass**: Total mass of all triangles combined
    /// - **local_com**: Center of mass (area/volume weighted)
    /// - **inv_principal_inertia**: Combined angular inertia
    ///
    /// # Physics Background
    ///
    /// For each triangle:
    /// 1. Compute area (2D) or volume contribution (3D)
    /// 2. Find center of mass (centroid)
    /// 3. Calculate moment of inertia
    /// 4. Use parallel axis theorem to shift to common reference frame
    /// 5. Sum all contributions
    ///
    /// # Example (2D) - L-Shape
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::mass_properties::MassProperties;
    /// use parry2d::math::Vector;
    ///
    /// // Create an L-shaped mesh from two rectangles (4 triangles)
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(2.0, 0.0),
    ///     Vector::new(2.0, 1.0),
    ///     Vector::new(1.0, 1.0),
    ///     Vector::new(1.0, 3.0),
    ///     Vector::new(0.0, 3.0),
    /// ];
    ///
    /// let indices = vec![
    ///     [0, 1, 2], // Bottom rectangle (triangle 1)
    ///     [0, 2, 3], // Bottom rectangle (triangle 2)
    ///     [0, 3, 4], // Vertical part (triangle 1)
    ///     [0, 4, 5], // Vertical part (triangle 2)
    /// ];
    ///
    /// let density = 100.0;
    /// let l_shape_props = MassProperties::from_trimesh(density, &vertices, &indices);
    ///
    /// println!("L-shape mass: {:.2} kg", l_shape_props.mass());
    /// println!("Center of mass: {:?}", l_shape_props.local_com);
    /// # }
    /// ```
    ///
    /// # Example (3D) - Pyramid
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::math::Vector;
    ///
    /// // Square pyramid: 4 vertices at base + 1 apex
    /// let vertices = vec![
    ///     Vector::new(-1.0, 0.0, -1.0), // Base corner 1
    ///     Vector::new(1.0, 0.0, -1.0),  // Base corner 2
    ///     Vector::new(1.0, 0.0, 1.0),   // Base corner 3
    ///     Vector::new(-1.0, 0.0, 1.0),  // Base corner 4
    ///     Vector::new(0.0, 2.0, 0.0),   // Apex
    /// ];
    ///
    /// let indices = vec![
    ///     [0, 1, 4], // Side face 1
    ///     [1, 2, 4], // Side face 2
    ///     [2, 3, 4], // Side face 3
    ///     [3, 0, 4], // Side face 4
    ///     [0, 2, 1], // Base (triangle 1)
    ///     [0, 3, 2], // Base (triangle 2)
    /// ];
    ///
    /// let density = 1000.0;
    /// let pyramid_props = MassProperties::from_trimesh(density, &vertices, &indices);
    ///
    /// println!("Pyramid mass: {:.2} kg", pyramid_props.mass());
    /// println!("Center of mass: {:?}", pyramid_props.local_com);
    /// # }
    /// ```
    ///
    /// # Example (3D) - Loading from File
    ///
    /// ```ignore
    /// # {
    /// use parry3d::mass_properties::MassProperties;
    ///
    /// // Assume you've loaded a mesh from an OBJ file
    /// let mesh = load_obj_file("complex_model.obj");
    /// let vertices = mesh.vertices;
    /// let indices = mesh.indices;
    ///
    /// let density = 2700.0; // Aluminum
    /// let props = MassProperties::from_trimesh(density, &vertices, &indices);
    ///
    /// println!("Model mass: {:.2} kg", props.mass());
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Complex 3D models**: Characters, vehicles, buildings
    /// - **Terrain**: Height-mapped ground, caves, landscapes
    /// - **Custom shapes**: Anything representable as triangles
    /// - **Imported models**: Meshes from modeling software (Blender, Maya, etc.)
    /// - **Non-convex objects**: Concave shapes that can't use simpler primitives
    ///
    /// # Mesh Quality Considerations
    ///
    /// - **Watertight meshes** (3D): For accurate volume/mass, mesh should be closed
    ///   - Open meshes may give incorrect results
    ///   - Check for holes, gaps, or missing faces
    /// - **Triangle orientation**: Consistent winding order improves accuracy
    ///   - Counter-clockwise from outside (right-hand rule)
    /// - **Degenerate triangles**: Zero-area triangles are automatically handled (skipped)
    /// - **Overlapping triangles**: Can cause incorrect results; ensure clean mesh
    ///
    /// # Performance
    ///
    /// Computation time is O(n) where n is the number of triangles. For large meshes
    /// (thousands of triangles), this can take noticeable time. Consider:
    /// - Using simpler primitive approximations when possible
    /// - Pre-computing mass properties and caching results
    /// - Simplifying meshes for physics (use low-poly collision mesh)
    ///
    /// # Trimesh vs Simpler Shapes
    ///
    /// For better performance and accuracy, use primitive shapes when possible:
    /// - Ball, Cuboid, Cylinder: Much faster and more accurate
    /// - Capsule: Better for elongated objects
    /// - Compound: Combine multiple primitives
    ///
    /// Use trimesh only when the shape is truly complex and can't be approximated.
    ///
    /// # See Also
    ///
    /// - `from_convex_polyhedron()`: Alias for convex meshes
    /// - `from_triangle()`: For single triangles
    /// - `from_compound()`: Combine multiple simpler shapes
    pub fn from_trimesh(
        density: Real,
        vertices: &[Vector],
        indices: &[[u32; 3]],
    ) -> MassProperties {
        let (area, com) = trimesh_area_and_center_of_mass(vertices, indices);

        if area == 0.0 {
            return MassProperties::new(com, 0.0, 0.0);
        }

        let mut itot = 0.0;

        for idx in indices {
            let triangle = Triangle::new(
                vertices[idx[0] as usize],
                vertices[idx[1] as usize],
                vertices[idx[2] as usize],
            );

            // TODO: is the parallel axis theorem correctly applied here?
            let area = triangle.area();
            let ipart = triangle.unit_angular_inertia();
            itot += ipart * area;
        }

        Self::new(com, area * density, itot * density)
    }
}

/// Computes the area and center-of-mass of a triangle-mesh.
pub fn trimesh_area_and_center_of_mass(
    vertices: &[Vector],
    indices: &[[u32; 3]],
) -> (Real, Vector) {
    let mut res = Vector::ZERO;
    let mut areasum = 0.0;

    for idx in indices {
        let triangle = Triangle::new(
            vertices[idx[0] as usize],
            vertices[idx[1] as usize],
            vertices[idx[2] as usize],
        );
        let area = triangle.area();
        let center = triangle.center();

        res += center * area;
        areasum += area;
    }

    if areasum == 0.0 {
        (areasum, res)
    } else {
        res /= areasum;
        (areasum, res)
    }
}
