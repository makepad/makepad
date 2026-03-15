use crate::mass_properties::MassProperties;
use crate::math::{Real, Vector};
use crate::shape::Triangle;

impl MassProperties {
    /// Computes the mass properties of a convex polygon (2D only).
    ///
    /// A convex polygon is a 2D shape where all interior angles are less than 180 degrees
    /// and all vertices point outward. This function decomposes the polygon into triangles
    /// from the center of mass and sums their mass properties.
    ///
    /// # Arguments
    ///
    /// * `density` - The material density in kg/m² (mass per unit area)
    /// * `vertices` - A slice of points defining the polygon vertices
    ///   - Must form a convex shape
    ///   - Vertices should be ordered counter-clockwise
    ///   - At least 3 vertices required
    ///
    /// # Returns
    ///
    /// A `MassProperties` struct containing:
    /// - **mass**: Total mass calculated from area and density
    /// - **local_com**: Center of mass (weighted average of triangle centroids)
    /// - **inv_principal_inertia**: Inverse angular inertia (scalar in 2D)
    ///
    /// # Physics Background
    ///
    /// The algorithm:
    /// 1. Computes the geometric center of all vertices
    /// 2. Creates triangles from the center to each edge
    /// 3. Calculates area and inertia for each triangle
    /// 4. Combines results using weighted averages
    ///
    /// # Example - Pentagon
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::mass_properties::MassProperties;
    /// use parry2d::math::Vector;
    /// use std::f32::consts::PI;
    ///
    /// // Create a regular pentagon with radius 1.0
    /// let mut vertices = Vec::new();
    /// for i in 0..5 {
    ///     let angle = (i as f32) * 2.0 * PI / 5.0;
    ///     vertices.push(Vector::new(angle.cos(), angle.sin()));
    /// }
    ///
    /// let density = 100.0;
    /// let pentagon_props = MassProperties::from_convex_polygon(density, &vertices);
    ///
    /// println!("Pentagon mass: {:.2} kg", pentagon_props.mass());
    /// println!("Center of mass: {:?}", pentagon_props.local_com);
    ///
    /// // For a regular polygon centered at origin, COM should be near origin
    /// assert!(pentagon_props.local_com.length() < 0.01);
    /// # }
    /// ```
    ///
    /// # Example - Trapezoid
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::mass_properties::MassProperties;
    /// use parry2d::math::Vector;
    ///
    /// // Create a trapezoid (4 vertices)
    /// let vertices = vec![
    ///     Vector::ZERO,  // Bottom left
    ///     Vector::new(4.0, 0.0),  // Bottom right
    ///     Vector::new(3.0, 2.0),  // Top right
    ///     Vector::new(1.0, 2.0),  // Top left
    /// ];
    ///
    /// let density = 50.0;
    /// let trap_props = MassProperties::from_convex_polygon(density, &vertices);
    ///
    /// // Area of trapezoid = ((b1 + b2) / 2) × h = ((4 + 2) / 2) × 2 = 6 m²
    /// // Mass = area × density = 300 kg
    /// let mass = trap_props.mass();
    /// println!("Trapezoid mass: {:.2} kg", mass);
    /// # }
    /// ```
    ///
    /// # Example - Custom Convex Shape
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::mass_properties::MassProperties;
    /// use parry2d::math::Vector;
    ///
    /// // Arbitrary convex polygon
    /// let vertices = vec![
    ///     Vector::ZERO,
    ///     Vector::new(2.0, 0.0),
    ///     Vector::new(3.0, 1.0),
    ///     Vector::new(2.0, 2.5),
    ///     Vector::new(0.0, 2.0),
    /// ];
    ///
    /// let density = 200.0;
    /// let props = MassProperties::from_convex_polygon(density, &vertices);
    ///
    /// println!("Custom polygon mass: {:.2} kg", props.mass());
    /// println!("Moment of inertia: {:.2}", props.principal_inertia());
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Custom 2D shapes**: Game objects with specific geometry
    /// - **Simplified collision**: Convex approximations of complex shapes
    /// - **Platforms**: Angled or irregular platforms in 2D games
    /// - **Polygonal wheels**: Multi-sided rotating objects
    /// - **Terrain segments**: Ground pieces with varying slopes
    ///
    /// # Important Requirements
    ///
    /// - **Must be convex**: Non-convex (concave) polygons will produce incorrect results
    /// - **Vertex ordering**: Counter-clockwise order is conventional but either works
    /// - **Minimum vertices**: At least 3 vertices (forms a triangle)
    /// - **No self-intersection**: Vertices must not cross each other
    ///
    /// # For Non-Convex Shapes
    ///
    /// If your polygon is concave (not convex):
    /// 1. Use convex decomposition algorithms to break it into convex parts
    /// 2. Compute mass properties for each convex part
    /// 3. Combine using `from_compound()` or by summing MassProperties
    ///
    /// Alternatively, use `from_trimesh()` with a triangulated version of the shape.
    ///
    /// # Performance Note
    ///
    /// Computation time is O(n) where n is the number of vertices. The polygon is
    /// decomposed into n triangles, each processed independently.
    pub fn from_convex_polygon(density: Real, vertices: &[Vector]) -> MassProperties {
        let (area, com) = convex_polygon_area_and_center_of_mass(vertices);

        if area == 0.0 {
            return MassProperties::new(com, 0.0, 0.0);
        }

        let mut itot = 0.0;

        let mut iterpeek = vertices.iter().peekable();
        let first_element = *iterpeek.peek().unwrap(); // store first element to close the cycle in the end with unwrap_or
        while let Some(elem) = iterpeek.next() {
            let triangle = Triangle::new(com, *elem, **iterpeek.peek().unwrap_or(&first_element));
            let area = triangle.area();
            let ipart = triangle.unit_angular_inertia();
            itot += ipart * area;
        }

        Self::new(com, area * density, itot * density)
    }
}

/// Computes the area and center-of-mass of a convex polygon.
pub fn convex_polygon_area_and_center_of_mass(convex_polygon: &[Vector]) -> (Real, Vector) {
    let mut geometric_center = convex_polygon.iter().fold(Vector::ZERO, |e1, e2| e1 + e2);
    geometric_center /= convex_polygon.len() as Real;
    let mut res = Vector::ZERO;
    let mut areasum = 0.0;

    let mut iterpeek = convex_polygon.iter().peekable();
    let first_element = *iterpeek.peek().unwrap(); // Stores first element to close the cycle in the end with unwrap_or.
    while let Some(elem) = iterpeek.next() {
        let (a, b, c) = (
            elem,
            iterpeek.peek().unwrap_or(&first_element),
            &geometric_center,
        );
        let area = Triangle::new(*a, **b, *c).area();
        let center = (*a + **b + *c) / 3.0;

        res += center * area;
        areasum += area;
    }

    if areasum == 0.0 {
        (areasum, geometric_center)
    } else {
        res /= areasum;
        (areasum, res)
    }
}
