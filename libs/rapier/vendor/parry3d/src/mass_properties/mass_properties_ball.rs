use crate::mass_properties::MassProperties;
use crate::math::{PrincipalAngularInertia, Real, RealField, Vector};

impl MassProperties {
    pub(crate) fn ball_volume_unit_angular_inertia(
        radius: Real,
    ) -> (Real, PrincipalAngularInertia) {
        #[cfg(feature = "dim2")]
        {
            let volume = Real::pi() * radius * radius;
            let i = radius * radius / 2.0;
            (volume, i)
        }
        #[cfg(feature = "dim3")]
        {
            let volume = Real::pi() * radius * radius * radius * 4.0 / 3.0;
            let i = radius * radius * 2.0 / 5.0;

            (volume, Vector::splat(i))
        }
    }

    /// Computes the mass properties of a ball (sphere in 3D, circle in 2D).
    ///
    /// A ball is a perfectly round geometric shape defined by its radius. This function
    /// calculates the physical properties needed for physics simulation, including mass,
    /// center of mass, and angular inertia (resistance to rotation).
    ///
    /// # Arguments
    ///
    /// * `density` - The material density (mass per unit volume/area). Higher values make heavier objects.
    ///   - In 3D: units are typically kg/m³ (e.g., water = 1000, steel = 7850)
    ///   - In 2D: units are typically kg/m² (mass per unit area)
    /// * `radius` - The radius of the ball (distance from center to surface)
    ///
    /// # Returns
    ///
    /// A `MassProperties` struct containing:
    /// - **mass**: Total mass calculated from volume and density
    /// - **local_com**: Center of mass at the origin (balls are perfectly symmetric)
    /// - **inv_principal_inertia**: Inverse angular inertia (resistance to spinning)
    ///
    /// # Physics Background
    ///
    /// Balls have uniform density and perfect symmetry, which means:
    /// - The center of mass is at the geometric center (origin)
    /// - All rotational axes have the same angular inertia (isotropic)
    /// - In 3D: moment of inertia = (2/5) * mass * radius²
    /// - In 2D: moment of inertia = (1/2) * mass * radius²
    ///
    /// # Example (3D)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::math::Vector;
    ///
    /// // Create mass properties for a 0.5m radius ball with density 1000 kg/m³ (water density)
    /// let radius = 0.5;
    /// let density = 1000.0;
    /// let ball_props = MassProperties::from_ball(density, radius);
    ///
    /// // Volume of sphere: (4/3) * π * r³ = 0.524 m³
    /// // Mass: volume * density = 524 kg
    /// let mass = ball_props.mass();
    /// assert!((mass - 523.6).abs() < 1.0); // Approximately 524 kg
    ///
    /// // Center of mass is at the origin for symmetric shapes
    /// assert_eq!(ball_props.local_com, Vector::ZERO);
    ///
    /// // Check if object can be moved (finite mass)
    /// assert!(ball_props.inv_mass > 0.0, "Ball has finite mass and can move");
    /// # }
    /// ```
    ///
    /// # Example (2D)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::mass_properties::MassProperties;
    /// use parry2d::math::Vector;
    ///
    /// // Create a circular disc with 1.0m radius and density 100 kg/m²
    /// let radius = 1.0;
    /// let density = 100.0;
    /// let circle_props = MassProperties::from_ball(density, radius);
    ///
    /// // Area of circle: π * r² = 3.14 m²
    /// // Mass: area * density = 314 kg
    /// let mass = circle_props.mass();
    /// assert!((mass - 314.159).abs() < 0.1); // Approximately 314 kg
    ///
    /// println!("Circle mass: {:.2} kg", mass);
    /// println!("Moment of inertia: {:.2}", circle_props.principal_inertia());
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Sports balls**: Soccer balls, basketballs, bowling balls
    /// - **Planets and celestial bodies**: Spherical approximations
    /// - **Particles**: Vector-like objects with rotational inertia
    /// - **Wheels and gears**: Cylindrical objects in 2D simulations
    ///
    /// # Performance Note
    ///
    /// This is a very fast computation (constant time) as it only involves basic arithmetic
    /// with the radius and density. Balls are the simplest shape for collision detection
    /// and physics simulation.
    pub fn from_ball(density: Real, radius: Real) -> Self {
        let (vol, unit_i) = Self::ball_volume_unit_angular_inertia(radius);
        let mass = vol * density;
        Self::new(Vector::ZERO, mass, unit_i * mass)
    }
}
