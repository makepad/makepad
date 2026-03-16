use crate::mass_properties::MassProperties;
#[cfg(feature = "dim3")]
use crate::math::Matrix;
use crate::math::{Real, Vector};
use crate::shape::Voxels;

impl MassProperties {
    /// Computes the mass properties of a voxel grid.
    ///
    /// Voxels (volumetric pixels) represent a 3D shape as a grid of small cubes. This
    /// function treats each non-empty voxel as a small cuboid and combines their mass
    /// properties. It's useful for volumetric data, destructible terrain, or shapes that
    /// are difficult to represent with traditional geometry.
    ///
    /// # Arguments
    ///
    /// * `density` - The material density
    ///   - In 3D: kg/m³ (mass per unit volume)
    ///   - In 2D: kg/m² (mass per unit area)
    /// * `voxels` - A `Voxels` structure containing the voxel grid
    ///   - Each voxel is a small cube/square of uniform size
    ///   - Voxels can be empty or filled
    ///   - Since v0.25.0, uses sparse storage internally for efficiency
    ///
    /// # Returns
    ///
    /// A `MassProperties` struct containing:
    /// - **mass**: Total mass of all non-empty voxels
    /// - **local_com**: Center of mass (weighted average of voxel centers)
    /// - **inv_principal_inertia**: Combined angular inertia
    ///
    /// # Physics Background
    ///
    /// The algorithm:
    /// 1. Compute mass properties of a single voxel (small cuboid)
    /// 2. For each non-empty voxel, shift its mass properties to its position
    /// 3. Sum all contributions using parallel axis theorem
    /// 4. Empty voxels contribute nothing (zero mass)
    ///
    /// # Example (3D) - Simple Voxel Object
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// // Create a 3×3×3 voxel grid with 1m voxels
    /// let voxel_size = Vector::new(1.0, 1.0, 1.0);
    ///
    /// // Fill some voxels to create an L-shape
    /// let voxels = &[
    ///     IVector::new(0, 0, 0), // Bottom bar
    ///     IVector::new(1, 0, 0),
    ///     IVector::new(2, 0, 0),
    ///     IVector::new(0, 1, 0), // Vertical part
    ///     IVector::new(0, 2, 0),
    /// ];
    /// let voxels = Voxels::new(voxel_size, voxels);
    ///
    /// let density = 1000.0; // Water density
    /// let voxel_props = MassProperties::from_voxels(density, &voxels);
    ///
    /// // 5 voxels × 1m³ each × 1000 kg/m³ = 5000 kg
    /// println!("Voxel object mass: {:.2} kg", voxel_props.mass());
    /// println!("Center of mass: {:?}", voxel_props.local_com);
    /// # }
    /// ```
    ///
    /// # Example (3D) - Destructible Terrain
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// // Create a chunk of destructible terrain
    /// let voxel_size = Vector::new(0.5, 0.5, 0.5); // 50cm voxels
    /// let mut voxels = vec![];
    ///
    /// // Fill a 4×4×4 solid block
    /// for x in 0..4 {
    ///     for y in 0..4 {
    ///         for z in 0..4 {
    ///             voxels.push(IVector::new(x, y, z));
    ///         }
    ///     }
    /// }
    ///
    /// let mut terrain = Voxels::new(voxel_size, &voxels);
    ///
    /// let density = 2400.0; // Concrete
    /// let terrain_props = MassProperties::from_voxels(density, &terrain);
    ///
    /// println!("Terrain chunk mass: {:.2} kg", terrain_props.mass());
    /// # }
    /// ```
    ///
    /// # Example - Sparse Voxel Grid (Efficient)
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::mass_properties::MassProperties;
    /// use parry3d::shape::Voxels;
    /// use parry3d::math::{Vector, IVector};
    ///
    /// // Large sparse grid (only stores filled voxels since v0.25.0)
    /// let voxel_size = Vector::new(0.1, 0.1, 0.1);
    ///
    /// // Scatter some voxels in a large space (efficient with sparse storage)
    /// let voxels = &[
    ///     IVector::new(0, 0, 0),
    ///     IVector::new(100, 50, 75),
    ///     IVector::new(-50, 200, -30),
    /// ];
    /// let voxels = Voxels::new(voxel_size, voxels);
    /// let density = 1000.0;
    /// let props = MassProperties::from_voxels(density, &voxels);
    ///
    /// // Only 3 voxels contribute to mass
    /// println!("Sparse voxel mass: {:.4} kg", props.mass());
    /// # }
    /// ```
    ///
    /// # Use Cases
    ///
    /// - **Destructible terrain**: Voxel-based environments (Minecraft-style)
    /// - **Medical imaging**: CT scans, MRI data volumetric analysis
    /// - **Procedural generation**: Voxel-based world generation
    /// - **Simulation**: Granular materials, fluids represented as voxels
    /// - **Dynamic shapes**: Objects that change shape at runtime
    /// - **Complex geometry**: Shapes difficult to represent with meshes
    ///
    /// # Performance Considerations
    ///
    /// - **Sparse storage** (v0.25.0+): Only filled voxels consume memory
    /// - **Computation time**: O(n) where n = number of filled voxels
    /// - **For large grids**: Prefer coarser voxel sizes when possible
    /// - **Memory usage**: Each voxel stores position and state
    /// - **Alternative**: For static shapes, consider using triangle meshes
    ///
    /// # Voxel Size Trade-offs
    ///
    /// **Smaller voxels**:
    /// - More accurate representation of curved surfaces
    /// - More voxels = longer computation time
    /// - Higher memory usage (more voxels to store)
    ///
    /// **Larger voxels**:
    /// - Faster computation
    /// - Less memory
    /// - Blockier appearance (lower resolution)
    ///
    /// # Accuracy Notes
    ///
    /// - Voxel representation is an approximation of the true shape
    /// - Smooth curves become staircase patterns
    /// - Mass properties accuracy depends on voxel resolution
    /// - For exact results with smooth shapes, use primitive shapes or meshes
    ///
    /// # Empty vs Filled Voxels
    ///
    /// - Only non-empty voxels contribute to mass
    /// - Empty voxels are ignored (zero mass, no inertia)
    /// - The voxel state is checked using `vox.state.is_empty()`
    ///
    /// # See Also
    ///
    /// - `Voxels::new()`: Create a new voxel grid
    /// - `Voxels::set_voxel()`: Add or remove voxels
    /// - `from_trimesh()`: Alternative for precise shapes
    /// - `from_compound()`: Combine multiple shapes efficiently
    pub fn from_voxels(density: Real, voxels: &Voxels) -> Self {
        let mut com = Vector::ZERO;
        let mut num_not_empty = 0;
        #[cfg(feature = "dim2")]
        let mut angular_inertia = 0.0;
        #[cfg(feature = "dim3")]
        let mut angular_inertia = Matrix::ZERO;
        let block_ref_mprops = MassProperties::from_cuboid(density, voxels.voxel_size() / 2.0);

        for vox in voxels.voxels() {
            if !vox.state.is_empty() {
                com += vox.center;
                num_not_empty += 1;
            }
        }

        com /= num_not_empty as Real;

        for vox in voxels.voxels() {
            if !vox.state.is_empty() {
                angular_inertia +=
                    block_ref_mprops.construct_shifted_inertia_matrix(vox.center - com);
            }
        }

        let mass = block_ref_mprops.mass() * num_not_empty as Real;

        #[cfg(feature = "dim2")]
        return Self::new(com, mass, angular_inertia);
        #[cfg(feature = "dim3")]
        return Self::with_inertia_matrix(com, mass, angular_inertia);
    }
}
