use crate::mass_properties::MassProperties;
use crate::math::{PrincipalAngularInertia, Real, Vector};

impl MassProperties {
    pub(crate) fn cuboid_volume_unit_inertia(
        half_extents: Vector,
    ) -> (Real, PrincipalAngularInertia) {
        #[cfg(feature = "dim2")]
        {
            let volume = half_extents.x * half_extents.y * 4.0;
            let ix = (half_extents.x * half_extents.x) / 3.0;
            let iy = (half_extents.y * half_extents.y) / 3.0;

            (volume, ix + iy)
        }

        #[cfg(feature = "dim3")]
        {
            let volume = half_extents.x * half_extents.y * half_extents.z * 8.0;
            let ix = (half_extents.x * half_extents.x) / 3.0;
            let iy = (half_extents.y * half_extents.y) / 3.0;
            let iz = (half_extents.z * half_extents.z) / 3.0;

            (volume, Vector::new(iy + iz, ix + iz, ix + iy))
        }
    }

    /// Computes the mass properties of a cuboid (box in 3D, rectangle in 2D).
    ///
    /// A cuboid is a box-shaped object with three dimensions (or two in 2D), where each
    /// dimension can have a different size. The cuboid is centered at the origin, and
    /// `half_extents` define the distance from the center to each face.
    ///
    /// # Arguments
    ///
    /// * `density` - The material density (mass per unit volume/area). Higher values make heavier objects.
    ///   - In 3D: units are typically kg/m³ (e.g., wood = 500-900, concrete = 2400)
    ///   - In 2D: units are typically kg/m² (mass per unit area)
    /// * `half_extents` - Half the size along each axis (center to face distance).
    ///   - In 3D: `Vector::new(hx, hy, hz)` creates a box with dimensions 2hx × 2hy × 2hz
    ///   - In 2D: `Vector::new(hx, hy)` creates a rectangle with dimensions 2hx × 2hy
    ///
    /// # Returns
    ///
    /// A `MassProperties` struct containing:
    /// - **mass**: Total mass calculated from volume and density
    /// - **local_com**: Center of mass at the origin (cuboids are symmetric)
    /// - **inv_principal_inertia**: Inverse angular inertia along each axis
    ///
    /// # Physics Background
    ///
    /// Cuboids have axis-aligned mass distribution:
    /// - Center of mass is at the geometric center (origin)
    /// - Angular inertia varies per axis based on dimensions
    /// - Longer dimensions increase inertia around perpendicular axes
    /// - In 3D, each axis has different inertia: I_x depends on y and z extents, etc.
    ///
    /// # Example (3D) - Wooden Crate
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::math::Vector;
    ///
    /// // Create a wooden crate: 2m × 1m × 1m (half_extents = 1.0, 0.5, 0.5)
    /// // Wood density: approximately 600 kg/m³
    /// let half_extents = Vector::new(1.0, 0.5, 0.5);
    /// let density = 600.0;
    /// let crate_props = MassProperties::from_cuboid(density, half_extents);
    ///
    /// // Volume = (2 * 1.0) × (2 * 0.5) × (2 * 0.5) = 2 m³
    /// // Mass = volume × density = 1200 kg
    /// let mass = crate_props.mass();
    /// assert!((mass - 1200.0).abs() < 0.1);
    ///
    /// // Longer dimension (x-axis) means higher inertia around y and z axes
    /// let inertia = crate_props.principal_inertia();
    /// println!("Inertia around x-axis: {:.2}", inertia.x); // Lowest (easier to spin around length)
    /// println!("Inertia around y-axis: {:.2}", inertia.y); // Higher
    /// println!("Inertia around z-axis: {:.2}", inertia.z); // Higher
    ///
    /// // Center of mass is at the origin
    /// assert_eq!(crate_props.local_com, Vector::ZERO);
    /// # }
    /// ```
    ///
    /// # Example (3D) - Cube
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::math::Vector;
    ///
    /// // Create a 1m × 1m × 1m cube (half_extents = 0.5 on all axes)
    /// let half_extents = Vector::new(0.5, 0.5, 0.5);
    /// let density = 1000.0; // Water density
    /// let cube_props = MassProperties::from_cuboid(density, half_extents);
    ///
    /// // Volume = 1 m³, Mass = 1000 kg
    /// assert!((cube_props.mass() - 1000.0).abs() < 0.1);
    ///
    /// // For a cube, all axes have equal inertia (symmetric)
    /// let inertia = cube_props.principal_inertia();
    /// assert!((inertia.x - inertia.y).abs() < 0.01);
    /// assert!((inertia.y - inertia.z).abs() < 0.01);
    /// # }
    /// ```
    ///
    /// # Example (2D) - Rectangular Platform
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::mass_properties::MassProperties;
    /// use parry2d::math::Vector;
    ///
    /// // Create a 4m × 2m rectangular platform (half_extents = 2.0, 1.0)
    /// let half_extents = Vector::new(2.0, 1.0);
    /// let density = 500.0; // kg/m²
    /// let platform_props = MassProperties::from_cuboid(density, half_extents);
    ///
    /// // Area = (2 * 2.0) × (2 * 1.0) = 8 m²
    /// // Mass = area × density = 4000 kg
    /// let mass = platform_props.mass();
    /// assert!((mass - 4000.0).abs() < 0.1);
    ///
    /// println!("Platform mass: {:.2} kg", mass);
    /// println!("Moment of inertia: {:.2}", platform_props.principal_inertia());
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Boxes and crates**: Storage containers, shipping boxes
    /// - **Building blocks**: Walls, floors, platforms
    /// - **Vehicles**: Simplified car or truck bodies
    /// - **Furniture**: Tables, chairs, cabinets
    /// - **Terrain**: Rectangular ground segments
    ///
    /// # Common Mistakes
    ///
    /// - **Wrong dimensions**: Remember that `half_extents` are HALF the total size.
    ///   For a 2m × 2m × 2m box, use `Vector::new(1.0, 1.0, 1.0)`, not `(2.0, 2.0, 2.0)`
    /// - **Unit confusion**: Ensure density units match your distance units
    ///   (kg/m³ with meters, kg/cm³ with centimeters, etc.)
    ///
    /// # Performance Note
    ///
    /// This is a very fast computation (constant time). Cuboids are the second simplest
    /// shape after balls and are highly efficient for collision detection.
    pub fn from_cuboid(density: Real, half_extents: Vector) -> Self {
        let (vol, unit_i) = Self::cuboid_volume_unit_inertia(half_extents);
        let mass = vol * density;
        Self::new(Vector::ZERO, mass, unit_i * mass)
    }
}
