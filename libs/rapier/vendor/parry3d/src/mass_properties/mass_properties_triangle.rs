use crate::mass_properties::MassProperties;
use crate::math::{Real, Vector};
use crate::shape::Triangle;

impl MassProperties {
    /// Computes the mass properties of a triangle.
    ///
    /// A triangle is the simplest polygon, defined by three vertices. In 2D, this represents
    /// a filled triangular region. In 3D, this represents a flat triangular surface with
    /// negligible thickness (useful for thin sheets or as building blocks for meshes).
    ///
    /// # Arguments
    ///
    /// * `density` - The material density (mass per unit area in both 2D and 3D)
    ///   - Units are typically kg/m² (surface density)
    ///   - For 3D triangles, this represents the density of a thin sheet
    /// * `a`, `b`, `c` - The three vertices of the triangle
    ///
    /// # Returns
    ///
    /// A `MassProperties` struct containing:
    /// - **mass**: Total mass calculated from area and density
    /// - **local_com**: Center of mass at the centroid (average of three vertices)
    /// - **inv_principal_inertia**: Inverse angular inertia
    ///
    /// # Physics Background
    ///
    /// Triangles have specific geometric properties:
    /// - Area (2D): Using cross product of edge vectors
    /// - Area (3D): Same formula, treating triangle as flat surface
    /// - Center of mass: Always at centroid = (a + b + c) / 3
    /// - Angular inertia: Depends on vertex positions relative to centroid
    /// - Degenerate cases: Zero-area triangles (collinear points) return zero mass
    ///
    /// # Example (2D) - Right Triangle
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::mass_properties::MassProperties;
    /// use parry2d::math::Vector;
    ///
    /// // Create a right triangle with legs of 3m and 4m
    /// let a = Vector::ZERO;
    /// let b = Vector::new(3.0, 0.0);
    /// let c = Vector::new(0.0, 4.0);
    /// let density = 100.0; // kg/m²
    ///
    /// let triangle_props = MassProperties::from_triangle(density, a, b, c);
    ///
    /// // Area = (1/2) × base × height = (1/2) × 3 × 4 = 6 m²
    /// // Mass = area × density = 600 kg
    /// let mass = triangle_props.mass();
    /// assert!((mass - 600.0).abs() < 0.1);
    ///
    /// // Center of mass at centroid: (0+3+0)/3, (0+0+4)/3 = (1, 1.333)
    /// let com = triangle_props.local_com;
    /// assert!((com.x - 1.0).abs() < 0.01);
    /// assert!((com.y - 4.0/3.0).abs() < 0.01);
    ///
    /// println!("Triangle mass: {:.2} kg", mass);
    /// println!("Center of mass: ({:.2}, {:.2})", com.x, com.y);
    /// # }
    /// ```
    ///
    /// # Example (2D) - Equilateral Triangle
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::mass_properties::MassProperties;
    /// use parry2d::math::Vector;
    ///
    /// // Equilateral triangle with side length 2m
    /// let side = 2.0;
    /// let height = side * (3.0_f32.sqrt() / 2.0);
    ///
    /// let a = Vector::ZERO;
    /// let b = Vector::new(side, 0.0);
    /// let c = Vector::new(side / 2.0, height);
    /// let density = 50.0;
    ///
    /// let tri_props = MassProperties::from_triangle(density, a, b, c);
    ///
    /// // For equilateral triangle: Area = (side² × √3) / 4
    /// let expected_area = side * side * 3.0_f32.sqrt() / 4.0;
    /// let mass = tri_props.mass();
    /// assert!((mass - expected_area * density).abs() < 0.1);
    ///
    /// println!("Equilateral triangle mass: {:.2} kg", mass);
    /// # }
    /// ```
    ///
    /// # Example (3D) - Triangle as Thin Sheet
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::math::Vector;
    ///
    /// // Triangle in 3D space (e.g., a metal plate or sail)
    /// let a = Vector::ZERO;
    /// let b = Vector::new(2.0, 0.0, 0.0);
    /// let c = Vector::new(1.0, 2.0, 0.0);
    /// let density = 200.0; // kg/m² (sheet metal)
    ///
    /// let plate_props = MassProperties::from_triangle(density, a, b, c);
    ///
    /// // Area = 2 m² (base=2, height=2, area=(1/2)×2×2=2)
    /// // Mass = 400 kg
    /// let mass = plate_props.mass();
    /// assert!((mass - 400.0).abs() < 0.1);
    ///
    /// println!("Metal plate mass: {:.2} kg", mass);
    /// # }
    /// ```
    ///
    /// # Example - Degenerate Triangle (Collinear Vectors)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::mass_properties::MassProperties;
    /// use parry2d::math::Vector;
    ///
    /// // Three points on a line (no area)
    /// let a = Vector::ZERO;
    /// let b = Vector::new(1.0, 1.0);
    /// let c = Vector::new(2.0, 2.0);
    /// let density = 100.0;
    ///
    /// let degenerate = MassProperties::from_triangle(density, a, b, c);
    ///
    /// // Zero area means zero mass
    /// assert_eq!(degenerate.mass(), 0.0);
    /// println!("Degenerate triangle has zero mass");
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Mesh building blocks**: Triangles are the basis for triangle meshes
    /// - **Thin surfaces**: Sails, flags, sheets of material
    /// - **Terrain patches**: Small triangular ground segments
    /// - **Simple shapes**: Quick prototyping with basic geometry
    /// - **2D games**: Triangular platforms, obstacles, or decorations
    ///
    /// # Vertex Order
    ///
    /// - The order of vertices (a, b, c) matters for orientation
    /// - Counter-clockwise order is conventional in 2D
    /// - In 3D, vertex order determines the normal direction (right-hand rule)
    /// - However, for mass properties, the orientation doesn't affect the result
    ///
    /// # Common Use with Meshes
    ///
    /// For complex shapes, use `from_trimesh()` instead, which handles multiple triangles:
    ///
    /// ```ignore
    /// // For a single triangle, use from_triangle
    /// let props = MassProperties::from_triangle(density, &a, &b, &c);
    ///
    /// // For multiple triangles, use from_trimesh
    /// let vertices = vec![a, b, c, d, e, f];
    /// let indices = vec![[0, 1, 2], [3, 4, 5]];
    /// let mesh_props = MassProperties::from_trimesh(density, &vertices, &indices);
    /// ```
    ///
    /// # Performance Note
    ///
    /// Computing triangle mass properties is very fast (constant time) and involves
    /// only basic geometric calculations (area, centroid, and moment of inertia).
    pub fn from_triangle(density: Real, a: Vector, b: Vector, c: Vector) -> MassProperties {
        let triangle = Triangle::new(a, b, c);
        let area = triangle.area();
        let com = triangle.center();

        if area == 0.0 {
            return MassProperties::new(com, 0.0, 0.0);
        }

        let ipart = triangle.unit_angular_inertia();

        Self::new(com, area * density, ipart * area * density)
    }
}
