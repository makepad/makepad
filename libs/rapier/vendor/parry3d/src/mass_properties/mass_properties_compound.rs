use crate::mass_properties::MassProperties;
use crate::math::{Pose, Real};
use crate::shape::SharedShape;

impl MassProperties {
    /// Computes the mass properties of a compound shape (combination of multiple shapes).
    ///
    /// A compound shape is a collection of sub-shapes, each with its own position and
    /// orientation. This function computes the mass properties of each sub-shape,
    /// transforms them to their local positions, and combines them using the parallel
    /// axis theorem to get the total mass properties.
    ///
    /// # Arguments
    ///
    /// * `density` - The material density applied to all sub-shapes
    ///   - In 3D: kg/m³ (mass per unit volume)
    ///   - In 2D: kg/m² (mass per unit area)
    /// * `shapes` - Array of (position, shape) pairs
    ///   - Each shape has an `Pose` (position + rotation)
    ///   - Shapes can be any type implementing the `Shape` trait
    ///
    /// # Returns
    ///
    /// A `MassProperties` struct containing:
    /// - **mass**: Sum of all sub-shape masses
    /// - **local_com**: Combined center of mass (mass-weighted average)
    /// - **inv_principal_inertia**: Combined angular inertia
    ///
    /// # Physics Background
    ///
    /// The parallel axis theorem is used to shift inertia tensors:
    /// - Each shape's mass properties are computed in its local frame
    /// - Properties are transformed to the compound's coordinate system
    /// - Center of mass is the mass-weighted average of all sub-shapes
    /// - Angular inertia accounts for both local rotation and offset from COM
    ///
    /// # Example (3D) - Dumbbell
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::shape::{Ball, SharedShape};
    /// use parry3d::math::{Pose, Vector};
    ///
    /// // Create a dumbbell: two balls connected by a bar
    /// let ball = SharedShape::new(Ball::new(0.5));
    /// let bar = SharedShape::new(parry3d::shape::Cuboid::new(Vector::new(0.1, 1.0, 0.1)));
    ///
    /// let shapes = vec![
    ///     (Pose::translation(0.0, -1.0, 0.0), ball.clone()),  // Left ball
    ///     (Pose::identity(), bar),                             // Center bar
    ///     (Pose::translation(0.0, 1.0, 0.0), ball),            // Right ball
    /// ];
    ///
    /// let density = 1000.0;
    /// let dumbbell_props = MassProperties::from_compound(density, &shapes);
    ///
    /// println!("Dumbbell mass: {:.2} kg", dumbbell_props.mass());
    /// println!("Center of mass: {:?}", dumbbell_props.local_com);
    ///
    /// // Dumbbell has high inertia around X and Z (hard to spin end-over-end)
    /// // but low inertia around Y (easy to spin along the bar)
    /// let inertia = dumbbell_props.principal_inertia();
    /// println!("Inertia: X={:.3}, Y={:.3}, Z={:.3}", inertia.x, inertia.y, inertia.z);
    /// # }
    /// ```
    ///
    /// # Example (2D) - Table
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::mass_properties::MassProperties;
    /// use parry2d::shape::{Cuboid, SharedShape};
    /// use parry2d::math::{Pose, Vector};
    ///
    /// // Create a simple table: top surface + legs
    /// let top = SharedShape::new(Cuboid::new(Vector::new(2.0, 0.1)));    // Wide, thin top
    /// let leg = SharedShape::new(Cuboid::new(Vector::new(0.1, 0.5)));    // Narrow, tall leg
    ///
    /// let shapes = vec![
    ///     (Pose::translation(0.0, 0.6), top),                   // Table top
    ///     (Pose::translation(-1.5, 0.0), leg.clone()),          // Left leg
    ///     (Pose::translation(1.5, 0.0), leg),                   // Right leg
    /// ];
    ///
    /// let density = 500.0; // Wood
    /// let table_props = MassProperties::from_compound(density, &shapes);
    ///
    /// println!("Table mass: {:.2} kg", table_props.mass());
    /// # }
    /// ```
    ///
    /// # Example (3D) - Robot Arm
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::shape::{Capsule, Cuboid, SharedShape};
    /// use parry3d::math::{Pose, Vector};
    ///
    /// // Simple robot arm with multiple segments
    /// let base = SharedShape::new(Cuboid::new(Vector::new(0.3, 0.2, 0.3)));
    /// let upper_arm = SharedShape::new(Capsule::new(
    ///     Vector::ZERO,
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     0.1
    /// ));
    /// let forearm = SharedShape::new(Capsule::new(
    ///     Vector::ZERO,
    ///     Vector::new(0.0, 0.8, 0.0),
    ///     0.08
    /// ));
    ///
    /// let shapes = vec![
    ///     (Pose::identity(), base),
    ///     (Pose::translation(0.0, 0.2, 0.0), upper_arm),
    ///     (Pose::translation(0.0, 1.2, 0.0), forearm),
    /// ];
    ///
    /// let density = 2700.0; // Aluminum
    /// let arm_props = MassProperties::from_compound(density, &shapes);
    ///
    /// println!("Robot arm mass: {:.2} kg", arm_props.mass());
    /// println!("Arm center of mass: {:?}", arm_props.local_com);
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Complex objects**: Multi-part objects (tables, chairs, vehicles)
    /// - **Articulated bodies**: Robot arms, character skeletons
    /// - **Assemblies**: Combining simple shapes into complex forms
    /// - **Non-convex shapes**: Convex decomposition results
    /// - **Hierarchical structures**: Nested compound shapes
    ///
    /// # Different Densities
    ///
    /// To use different densities for different parts:
    ///
    /// ```ignore
    /// // Compute each part separately with its own density
    /// let heavy_part = ball_shape.mass_properties(5000.0).transform_by(&pos1);
    /// let light_part = cuboid_shape.mass_properties(100.0).transform_by(&pos2);
    ///
    /// // Combine manually
    /// let total = heavy_part + light_part;
    /// ```
    ///
    /// # Performance Note
    ///
    /// The computation time is O(n) where n is the number of sub-shapes. Each shape's
    /// mass properties are computed once and then combined. This is efficient even for
    /// large numbers of shapes.
    ///
    /// # See Also
    ///
    /// - `MassProperties::transform_by()`: Transform mass properties to a new frame
    /// - `Add` trait: Combine mass properties with `+` operator
    /// - `Sum` trait: Sum an iterator of mass properties
    pub fn from_compound(density: Real, shapes: &[(Pose, SharedShape)]) -> Self {
        shapes
            .iter()
            .map(|s| s.1.mass_properties(density).transform_by(&s.0))
            .sum()
    }
}
