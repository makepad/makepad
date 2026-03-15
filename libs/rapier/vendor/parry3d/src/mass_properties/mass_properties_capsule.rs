use crate::mass_properties::MassProperties;
use crate::math::{Real, Vector};
#[cfg(feature = "dim3")]
use crate::shape::Capsule;

impl MassProperties {
    /// Computes the mass properties of a capsule (pill-shaped object).
    ///
    /// A capsule is a cylinder with hemispherical caps on both ends. It's defined by two
    /// endpoint centers (a line segment) and a radius. Capsules are commonly used for
    /// character controllers and elongated objects because they provide smooth collision
    /// handling without sharp edges.
    ///
    /// # Arguments
    ///
    /// * `density` - The material density (mass per unit volume/area). Higher values make heavier objects.
    ///   - In 3D: units are typically kg/m³
    ///   - In 2D: units are typically kg/m² (mass per unit area)
    /// * `a` - First endpoint of the capsule's central axis (center of first hemisphere)
    /// * `b` - Second endpoint of the capsule's central axis (center of second hemisphere)
    /// * `radius` - The radius of the capsule (distance from the axis to the surface)
    ///
    /// # Returns
    ///
    /// A `MassProperties` struct containing:
    /// - **mass**: Total mass calculated from volume and density
    /// - **local_com**: Center of mass at the midpoint between `a` and `b`
    /// - **inv_principal_inertia**: Inverse angular inertia (varies by axis)
    /// - **principal_inertia_local_frame** (3D only): Rotation aligning capsule axis with Y
    ///
    /// # Physics Background
    ///
    /// A capsule consists of:
    /// - A cylindrical body connecting the two endpoints
    /// - Two hemispherical caps (which together form one complete sphere)
    /// - The total length is: `distance(a, b) + 2 * radius`
    /// - Mass and inertia are computed by combining cylinder + sphere components
    ///
    /// # Example (3D) - Character Controller
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::math::Vector;
    ///
    /// // Create a capsule for a standing character (height ~2m, radius 0.3m)
    /// // Endpoints at (0, 0, 0) and (0, 2, 0) form vertical capsule
    /// let a = Vector::ZERO;
    /// let b = Vector::new(0.0, 2.0, 0.0);
    /// let radius = 0.3;
    /// let density = 985.0; // Similar to human body density
    ///
    /// let character_props = MassProperties::from_capsule(density, a, b, radius);
    ///
    /// // Center of mass is at the midpoint
    /// let expected_com = Vector::new(0.0, 1.0, 0.0);
    /// assert!((character_props.local_com - expected_com).length() < 0.01);
    ///
    /// let mass = character_props.mass();
    /// println!("Character mass: {:.2} kg", mass); // Approximately 70-80 kg
    ///
    /// // Inertia is higher around horizontal axes (harder to tip over)
    /// let inertia = character_props.principal_inertia();
    /// println!("Inertia X: {:.2}, Y: {:.2}, Z: {:.2}", inertia.x, inertia.y, inertia.z);
    /// # }
    /// ```
    ///
    /// # Example (3D) - Horizontal Capsule (Lying Down)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::math::Vector;
    ///
    /// // Create a horizontal capsule along the X-axis
    /// let a = Vector::new(-1.0, 0.0, 0.0);
    /// let b = Vector::new(1.0, 0.0, 0.0);
    /// let radius = 0.5;
    /// let density = 1000.0;
    ///
    /// let capsule_props = MassProperties::from_capsule(density, a, b, radius);
    ///
    /// // Center of mass at midpoint (origin)
    /// assert_eq!(capsule_props.local_com, Vector::ZERO);
    ///
    /// // Total length = distance + 2*radius = 2.0 + 1.0 = 3.0 meters
    /// println!("Mass: {:.2} kg", capsule_props.mass());
    /// # }
    /// ```
    ///
    /// # Example (2D) - Stadium Shape
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::mass_properties::MassProperties;
    /// use parry2d::math::Vector;
    ///
    /// // Create a horizontal 2D capsule (stadium/discorectangle shape)
    /// let a = Vector::new(-2.0, 0.0);
    /// let b = Vector::new(2.0, 0.0);
    /// let radius = 1.0;
    /// let density = 100.0; // kg/m²
    ///
    /// let stadium_props = MassProperties::from_capsule(density, a, b, radius);
    ///
    /// println!("Stadium mass: {:.2} kg", stadium_props.mass());
    /// println!("Moment of inertia: {:.2}", stadium_props.principal_inertia());
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Character controllers**: Humanoid characters, NPCs
    /// - **Vehicles**: Simplified car or boat bodies
    /// - **Projectiles**: Bullets, missiles, arrows
    /// - **Limbs**: Arms, legs in ragdoll physics
    /// - **Cylinders with rounded ends**: Pipes, rods, poles
    ///
    /// # Common Mistakes
    ///
    /// - **Total length confusion**: The visual length is `distance(a, b) + 2 * radius`,
    ///   not just `distance(a, b)`. The hemispheres add extra length.
    /// - **Endpoint placement**: Vectors `a` and `b` are centers of the hemispherical caps,
    ///   not the extreme ends of the capsule.
    ///
    /// # Performance Note
    ///
    /// Capsules are very efficient for collision detection (almost as fast as spheres)
    /// and provide smooth rolling behavior. They're preferred over cylinders for
    /// dynamic objects that need to move smoothly.
    pub fn from_capsule(density: Real, a: Vector, b: Vector, radius: Real) -> Self {
        let half_height = (b - a).length() / 2.0;
        let (cyl_vol, cyl_unit_i) = Self::cylinder_y_volume_unit_inertia(half_height, radius);
        let (ball_vol, ball_unit_i) = Self::ball_volume_unit_angular_inertia(radius);
        let cap_vol = cyl_vol + ball_vol;
        let cap_mass = cap_vol * density;
        let mut cap_i = (cyl_unit_i * cyl_vol + ball_unit_i * ball_vol) * density;
        let local_com = a.midpoint(b);

        #[cfg(feature = "dim2")]
        {
            let h = half_height * 2.0;
            let extra = (h * h * 0.25 + h * radius * 3.0 / 8.0) * ball_vol * density;
            cap_i += extra;
            Self::new(local_com, cap_mass, cap_i)
        }

        #[cfg(feature = "dim3")]
        {
            let h = half_height * 2.0;
            let extra = (h * h * 0.25 + h * radius * 3.0 / 8.0) * ball_vol * density;
            cap_i.x += extra;
            cap_i.z += extra;
            let local_frame = Capsule::new(a, b, radius).rotation_wrt_y();
            Self::with_principal_inertia_frame(local_com, cap_mass, cap_i, local_frame)
        }
    }
}
