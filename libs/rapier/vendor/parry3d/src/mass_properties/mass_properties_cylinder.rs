use crate::mass_properties::MassProperties;
use crate::math::{PrincipalAngularInertia, Real, Vector};
#[cfg(feature = "dim3")]
use crate::math::{RealField, Rotation};

impl MassProperties {
    pub(crate) fn cylinder_y_volume_unit_inertia(
        half_height: Real,
        radius: Real,
    ) -> (Real, PrincipalAngularInertia) {
        #[cfg(feature = "dim2")]
        {
            Self::cuboid_volume_unit_inertia(Vector::new(radius, half_height))
        }

        #[cfg(feature = "dim3")]
        {
            let volume = half_height * radius * radius * Real::pi() * 2.0;
            let sq_radius = radius * radius;
            let sq_height = half_height * half_height * 4.0;
            let off_principal = (sq_radius * 3.0 + sq_height) / 12.0;

            let inertia = Vector::new(off_principal, sq_radius / 2.0, off_principal);
            (volume, inertia)
        }
    }

    /// Computes the mass properties of a cylinder (3D only).
    ///
    /// A cylinder is a 3D shape with circular cross-section and flat ends, aligned along
    /// the Y-axis and centered at the origin. Unlike a capsule, a cylinder has sharp edges
    /// at the top and bottom.
    ///
    /// **Note**: In 2D, this function is used internally but cylinders don't exist as a
    /// distinct 2D shape (rectangles are used instead).
    ///
    /// # Arguments
    ///
    /// * `density` - The material density in kg/m³ (e.g., aluminum = 2700, plastic = 950)
    /// * `half_height` - Half the total height of the cylinder (center to top/bottom)
    /// * `radius` - The radius of the circular cross-section
    ///
    /// # Returns
    ///
    /// A `MassProperties` struct containing:
    /// - **mass**: Total mass calculated from volume and density
    /// - **local_com**: Center of mass at the origin (cylinders are symmetric)
    /// - **inv_principal_inertia**: Inverse angular inertia (different for each axis)
    /// - **principal_inertia_local_frame**: Identity rotation (aligned with Y-axis)
    ///
    /// # Physics Background
    ///
    /// Cylinders have rotational symmetry around the Y-axis:
    /// - Volume = π × radius² × height
    /// - Center of mass is at the geometric center (origin)
    /// - Angular inertia around Y-axis (spinning like a top): I_y = (1/2) × mass × radius²
    /// - Angular inertia around X/Z axes (tipping over): I_x = I_z = (1/12) × mass × (3×radius² + height²)
    /// - Easier to spin around the central axis than to tip over
    ///
    /// # Example - Aluminum Can
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::math::Vector;
    ///
    /// // Standard soda can: 12.3cm tall, 6.6cm diameter
    /// // Aluminum density: ~2700 kg/m³
    /// let half_height = 0.0615; // 6.15 cm in meters
    /// let radius = 0.033;        // 3.3 cm in meters
    /// let density = 2700.0;
    ///
    /// let can_props = MassProperties::from_cylinder(density, half_height, radius);
    ///
    /// let mass = can_props.mass();
    /// println!("Can mass: {:.2} kg", mass); // Approximately 0.15 kg (150 grams)
    ///
    /// // Center of mass at origin
    /// assert_eq!(can_props.local_com, Vector::ZERO);
    ///
    /// // Check inertia differences
    /// let inertia = can_props.principal_inertia();
    /// println!("Spin inertia (Y): {:.6}", inertia.y); // Low (easy to spin)
    /// println!("Tip inertia (X): {:.6}", inertia.x);  // Higher (harder to tip)
    /// assert!(inertia.y < inertia.x); // Easier to spin than tip
    /// # }
    /// ```
    ///
    /// # Example - Concrete Column
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    ///
    /// // Concrete support column: 3m tall, 0.5m diameter
    /// // Concrete density: ~2400 kg/m³
    /// let half_height = 1.5;  // 3m / 2
    /// let radius = 0.25;      // 0.5m / 2
    /// let density = 2400.0;
    ///
    /// let column_props = MassProperties::from_cylinder(density, half_height, radius);
    ///
    /// // Volume = π × (0.25)² × 3 = 0.589 m³
    /// // Mass = 0.589 × 2400 = 1414 kg
    /// let mass = column_props.mass();
    /// assert!((mass - 1414.0).abs() < 10.0);
    ///
    /// println!("Column mass: {:.0} kg", mass);
    /// # }
    /// ```
    ///
    /// # Example - Comparing Cylinder vs Capsule
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::math::Vector;
    ///
    /// let half_height = 1.0;
    /// let radius = 0.5;
    /// let density = 1000.0;
    ///
    /// // Cylinder has flat ends (sharp edges)
    /// let cylinder = MassProperties::from_cylinder(density, half_height, radius);
    ///
    /// // Capsule has rounded ends (smooth)
    /// let a = Vector::new(0.0, -half_height, 0.0);
    /// let b = Vector::new(0.0, half_height, 0.0);
    /// let capsule = MassProperties::from_capsule(density, a, b, radius);
    ///
    /// // Capsule has more mass due to hemispherical caps
    /// println!("Cylinder mass: {:.2} kg", cylinder.mass());
    /// println!("Capsule mass: {:.2} kg", capsule.mass());
    /// assert!(capsule.mass() > cylinder.mass());
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Structural elements**: Columns, pillars, posts
    /// - **Containers**: Cans, drums, barrels, tanks
    /// - **Mechanical parts**: Shafts, pistons, rollers
    /// - **Tree trunks**: Natural cylindrical objects
    /// - **Wheels**: When viewed from the side (use with proper orientation)
    ///
    /// # Cylinder vs Capsule
    ///
    /// **Use Cylinder when**:
    /// - Sharp edges are acceptable or desired
    /// - Object is truly flat-ended (cans, pipes)
    /// - Static/kinematic objects (don't need smooth rolling)
    ///
    /// **Use Capsule when**:
    /// - Smooth collision response is needed
    /// - Object needs to roll or slide smoothly
    /// - Character controllers or dynamic objects
    ///
    /// # Common Mistakes
    ///
    /// - **Wrong axis**: Cylinders are aligned with Y-axis by default. Use
    ///   `transform_by()` or create with proper orientation if you need X or Z alignment.
    /// - **Half height confusion**: Total height is `2 × half_height`, not just `half_height`
    ///
    /// # Performance Note
    ///
    /// Cylinder collision detection is more expensive than capsules due to sharp edges,
    /// but still reasonably efficient. For dynamic objects, prefer capsules.
    #[cfg(feature = "dim3")]
    pub fn from_cylinder(density: Real, half_height: Real, radius: Real) -> Self {
        let (cyl_vol, cyl_unit_i) = Self::cylinder_y_volume_unit_inertia(half_height, radius);
        let cyl_mass = cyl_vol * density;

        Self::with_principal_inertia_frame(
            Vector::ZERO,
            cyl_mass,
            cyl_unit_i * cyl_mass,
            Rotation::IDENTITY,
        )
    }
}
