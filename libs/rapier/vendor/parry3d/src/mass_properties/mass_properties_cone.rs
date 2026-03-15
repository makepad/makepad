use crate::mass_properties::MassProperties;
use crate::math::{PrincipalAngularInertia, Real, RealField, Rotation, Vector};

impl MassProperties {
    pub(crate) fn cone_y_volume_unit_inertia(
        half_height: Real,
        radius: Real,
    ) -> (Real, PrincipalAngularInertia) {
        let volume = radius * radius * Real::pi() * half_height * 2.0 / 3.0;
        let sq_radius = radius * radius;
        let sq_height = half_height * half_height * 4.0;
        let off_principal = sq_radius * 3.0 / 20.0 + sq_height * 3.0 / 80.0;
        let principal = sq_radius * 3.0 / 10.0;

        (volume, Vector::new(off_principal, principal, off_principal))
    }

    /// Computes the mass properties of a cone (3D only).
    ///
    /// A cone is a 3D shape with a circular base and a point (apex) at the top, aligned
    /// along the Y-axis. The base is at y = -half_height, the apex is at y = +half_height,
    /// and the center of the base is at the origin.
    ///
    /// # Arguments
    ///
    /// * `density` - The material density in kg/m³ (mass per unit volume)
    /// * `half_height` - Half the total height of the cone (center to apex/base)
    /// * `radius` - The radius of the circular base
    ///
    /// # Returns
    ///
    /// A `MassProperties` struct containing:
    /// - **mass**: Total mass calculated from volume and density
    /// - **local_com**: Center of mass at `(0, -half_height/2, 0)` - shifted toward the base
    /// - **inv_principal_inertia**: Inverse angular inertia (varies by axis)
    /// - **principal_inertia_local_frame**: Identity rotation (aligned with Y-axis)
    ///
    /// # Physics Background
    ///
    /// Cones have unique mass distribution due to tapering shape:
    /// - Volume = (1/3) × π × radius² × height
    /// - Center of mass is NOT at the geometric center
    /// - Center of mass is located 1/4 of the height from the base (toward the base)
    /// - The wider base contains more mass than the narrow top
    /// - Angular inertia around Y-axis (spinning): I_y = (3/10) × mass × radius²
    /// - Angular inertia around X/Z axes: includes both base radius and height terms
    ///
    /// # Example - Traffic Cone
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::math::Vector;
    ///
    /// // Standard traffic cone: 70cm tall, 30cm base diameter
    /// // Made of flexible plastic, density ~950 kg/m³
    /// let half_height = 0.35; // 70cm / 2 = 35cm
    /// let radius = 0.15;      // 30cm / 2 = 15cm
    /// let density = 950.0;
    ///
    /// let cone_props = MassProperties::from_cone(density, half_height, radius);
    ///
    /// let mass = cone_props.mass();
    /// println!("Traffic cone mass: {:.3} kg", mass); // About 1-2 kg
    ///
    /// // Center of mass is shifted toward the base (negative Y)
    /// let com_y = cone_props.local_com.y;
    /// println!("Center of mass Y: {:.3} m", com_y);
    /// assert!(com_y < 0.0, "COM should be below origin, toward the base");
    /// assert!((com_y - (-half_height / 2.0)).abs() < 0.01);
    /// # }
    /// ```
    ///
    /// # Example - Ice Cream Cone
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    ///
    /// // Small ice cream cone (wafer): 12cm tall, 5cm diameter
    /// let half_height = 0.06; // 6cm
    /// let radius = 0.025;     // 2.5cm
    /// let density = 400.0;    // Wafer is light and porous
    ///
    /// let wafer_props = MassProperties::from_cone(density, half_height, radius);
    ///
    /// // Volume = (1/3) × π × (0.025)² × 0.12 ≈ 0.0000785 m³
    /// let mass = wafer_props.mass();
    /// println!("Wafer mass: {:.1} grams", mass * 1000.0); // About 30 grams
    /// # }
    /// ```
    ///
    /// # Example - Center of Mass Position
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    ///
    /// // Demonstrate that COM is 1/4 height from base
    /// let half_height = 2.0;
    /// let radius = 1.0;
    /// let density = 1000.0;
    ///
    /// let cone_props = MassProperties::from_cone(density, half_height, radius);
    ///
    /// // Base is at y = -2.0, apex is at y = +2.0
    /// // COM should be at y = -2.0 + (4.0 / 4.0) = -1.0
    /// // Or equivalently: y = -half_height / 2 = -1.0
    /// let expected_com_y = -half_height / 2.0;
    /// assert!((cone_props.local_com.y - expected_com_y).abs() < 0.001);
    /// println!("Base at: {}", -half_height);
    /// println!("Apex at: {}", half_height);
    /// println!("COM at: {:.3}", cone_props.local_com.y);
    /// # }
    /// ```
    ///
    /// # Example - Cone vs Cylinder Comparison
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    ///
    /// let half_height = 1.0;
    /// let radius = 0.5;
    /// let density = 1000.0;
    ///
    /// let cone = MassProperties::from_cone(density, half_height, radius);
    /// let cylinder = MassProperties::from_cylinder(density, half_height, radius);
    ///
    /// // Cone has 1/3 the volume of a cylinder with same dimensions
    /// println!("Cone mass: {:.2} kg", cone.mass());
    /// println!("Cylinder mass: {:.2} kg", cylinder.mass());
    /// assert!((cylinder.mass() / cone.mass() - 3.0).abs() < 0.1);
    ///
    /// // Cone's COM is offset, cylinder's is at origin
    /// println!("Cone COM Y: {:.3}", cone.local_com.y);
    /// println!("Cylinder COM Y: {:.3}", cylinder.local_com.y);
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Traffic cones**: Road safety markers
    /// - **Funnels**: Pouring devices
    /// - **Party hats**: Conical decorations
    /// - **Volcanic mountains**: Natural cone-shaped terrain
    /// - **Rocket noses**: Aerodynamic cone shapes
    /// - **Drills and bits**: Conical tool tips
    ///
    /// # Important Notes
    ///
    /// - **Orientation**: Cone points upward (+Y), base is downward (-Y)
    /// - **Asymmetric COM**: Unlike cylinder or ball, center of mass is NOT at origin
    /// - **Volume**: Remember it's only 1/3 of an equivalent cylinder's volume
    /// - **Base position**: The base center is at the origin, not the geometric center
    ///
    /// # Common Mistakes
    ///
    /// - **Expecting COM at origin**: The center of mass is shifted toward the base by
    ///   `half_height/2` in the negative Y direction
    /// - **Confusing orientation**: The apex points in +Y direction, base faces -Y
    /// - **Volume estimation**: Cone volume is much smaller than you might expect
    ///   (only 1/3 of a cylinder with the same dimensions)
    ///
    /// # Performance Note
    ///
    /// Cone collision detection is moderately expensive due to the tapered shape and
    /// curved surface. For simpler simulations, consider using a cylinder or compound
    /// shape approximation.
    pub fn from_cone(density: Real, half_height: Real, radius: Real) -> Self {
        let (cyl_vol, cyl_unit_i) = Self::cone_y_volume_unit_inertia(half_height, radius);
        let cyl_mass = cyl_vol * density;

        Self::with_principal_inertia_frame(
            Vector::new(0.0, -half_height / 2.0, 0.0),
            cyl_mass,
            cyl_unit_i * cyl_mass,
            Rotation::IDENTITY,
        )
    }
}
