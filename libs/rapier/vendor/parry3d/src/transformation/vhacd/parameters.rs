use crate::math::Real;
use crate::transformation::voxelization::FillMode;

/// Parameters controlling the VHACD convex decomposition algorithm.
///
/// These parameters control the trade-off between decomposition quality, performance,
/// and the number of resulting convex parts. Understanding these parameters helps you
/// achieve the desired balance for your specific use case.
///
/// # Quick Parameter Guide
///
/// | Parameter | Lower Values | Higher Values | Recommended For |
/// |-----------|-------------|---------------|-----------------|
/// | `concavity` | More parts, better fit | Fewer parts, faster | Games: 0.01-0.05, Simulation: 0.001-0.01 |
/// | `resolution` | Faster, less detail | Slower, more detail | Preview: 32-64, Final: 64-256 |
/// | `max_convex_hulls` | Simpler result | More accurate | Simple: 4-8, Complex: 16-32 |
///
/// # Examples
///
/// ## Default Parameters (Balanced)
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::vhacd::VHACDParameters;
///
/// let params = VHACDParameters::default();
/// // Resolution: 64 (3D) or 256 (2D)
/// // Concavity: 0.01 (3D) or 0.1 (2D)
/// // Max convex hulls: 1024
/// // Good starting point for most cases
/// # }
/// ```
///
/// ## High Quality (Slower, More Accurate)
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::vhacd::VHACDParameters;
/// use parry3d::transformation::voxelization::FillMode;
///
/// let params = VHACDParameters {
///     resolution: 256,           // High detail voxelization
///     concavity: 0.001,          // Very tight fit to original
///     max_convex_hulls: 64,      // Allow many parts
///     plane_downsampling: 2,     // More precise plane search
///     convex_hull_downsampling: 2, // More precise hulls
///     alpha: 0.05,
///     beta: 0.05,
///     convex_hull_approximation: false, // Exact hulls
///     fill_mode: FillMode::FloodFill {
///         detect_cavities: false,
///     },
/// };
/// // Best for: Critical collision accuracy, offline processing
/// # }
/// ```
///
/// ## Fast Preview (Quick Iteration)
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::vhacd::VHACDParameters;
/// use parry3d::transformation::voxelization::FillMode;
///
/// let params = VHACDParameters {
///     resolution: 32,            // Low resolution for speed
///     concavity: 0.05,           // Allow some approximation
///     max_convex_hulls: 16,      // Limit part count
///     plane_downsampling: 8,     // Coarse plane search
///     convex_hull_downsampling: 8, // Coarse hulls
///     alpha: 0.05,
///     beta: 0.05,
///     convex_hull_approximation: true, // Fast approximation
///     fill_mode: FillMode::FloodFill {
///         detect_cavities: false,
///     },
/// };
/// // Best for: Rapid prototyping, testing during development
/// # }
/// ```
///
/// ## Game-Ready (Performance & Quality)
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::vhacd::VHACDParameters;
/// use parry3d::transformation::voxelization::FillMode;
///
/// let params = VHACDParameters {
///     resolution: 128,           // Good balance
///     concavity: 0.01,           // Reasonably tight
///     max_convex_hulls: 32,      // Practical limit
///     plane_downsampling: 4,     // Default precision
///     convex_hull_downsampling: 4, // Default precision
///     alpha: 0.05,
///     beta: 0.05,
///     convex_hull_approximation: true,
///     fill_mode: FillMode::FloodFill {
///         detect_cavities: false,
///     },
/// };
/// // Best for: Game colliders, physics simulations
/// # }
/// ```
///
/// # See Also
///
/// - Original implementation: <https://github.com/kmammou/v-hacd>
/// - Unity documentation: <https://github.com/Unity-Technologies/VHACD#parameters>
/// - Module documentation: [`crate::transformation::vhacd`]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VHACDParameters {
    /// Maximum allowed concavity (deviation from convexity) for each part.
    ///
    /// This is the **most important parameter** controlling the quality vs. part count trade-off.
    /// It measures how much the volume of each convex part can deviate from the actual geometry.
    ///
    /// # Behavior
    ///
    /// - **Lower values** (0.001 - 0.01): More convex parts, tighter fit to original shape
    /// - **Higher values** (0.05 - 0.1): Fewer convex parts, more approximation
    ///
    /// # Typical Values
    ///
    /// - **Simulation/Robotics**: 0.001 - 0.005 (high accuracy)
    /// - **Games**: 0.01 - 0.03 (balanced)
    /// - **Prototyping**: 0.05 - 0.1 (fast preview)
    ///
    /// # Default
    ///
    /// - 2D: `0.1` (more tolerant due to simpler geometry)
    /// - 3D: `0.01` (tighter tolerance for complex meshes)
    ///
    /// # Range
    ///
    /// Valid range: `[0.0, 1.0]`
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::vhacd::VHACDParameters;
    ///
    /// // High precision (more parts)
    /// let high_quality = VHACDParameters {
    ///     concavity: 0.001,
    ///     ..Default::default()
    /// };
    ///
    /// // Low precision (fewer parts, faster)
    /// let low_quality = VHACDParameters {
    ///     concavity: 0.05,
    ///     ..Default::default()
    /// };
    /// # }
    /// ```
    pub concavity: Real,

    /// Bias toward splitting along symmetry planes (e.g., cutting a humanoid down the middle).
    ///
    /// This parameter influences the algorithm to prefer cutting planes that align with
    /// the shape's symmetry. Higher values make the algorithm more likely to choose
    /// symmetric splits, which can produce more aesthetically pleasing decompositions.
    ///
    /// # Behavior
    ///
    /// - **0.0**: No bias, purely based on concavity minimization
    /// - **0.05** (default): Slight preference for symmetric splits
    /// - **0.2+**: Strong preference for symmetric splits
    ///
    /// # When to Adjust
    ///
    /// - Increase for symmetric objects (characters, vehicles, buildings)
    /// - Decrease for asymmetric or organic shapes
    ///
    /// # Default
    ///
    /// `0.05`
    ///
    /// # Range
    ///
    /// Valid range: `[0.0, 1.0]`
    pub alpha: Real,

    /// Bias toward splitting along revolution axes (e.g., cutting a cylinder lengthwise).
    ///
    /// This parameter influences the algorithm to prefer cutting planes that align with
    /// axes of rotational symmetry. Useful for objects with cylindrical or rotational
    /// features.
    ///
    /// # Behavior
    ///
    /// - **0.0**: No bias, purely based on concavity minimization
    /// - **0.05** (default): Slight preference for revolution axis splits
    /// - **0.2+**: Strong preference for revolution axis splits
    ///
    /// # When to Adjust
    ///
    /// - Increase for objects with cylindrical features (wheels, bottles, tunnels)
    /// - Decrease for box-like or irregular shapes
    ///
    /// # Default
    ///
    /// `0.05`
    ///
    /// # Range
    ///
    /// Valid range: `[0.0, 1.0]`
    pub beta: Real,

    /// Resolution of the voxel grid used for decomposition.
    ///
    /// The input mesh is first converted to a voxel grid. This parameter determines
    /// how fine that grid is. Higher resolution captures more detail but is slower
    /// and uses more memory.
    ///
    /// # Behavior
    ///
    /// - **32-64**: Fast, suitable for simple shapes or preview
    /// - **64-128**: Balanced, good for most game objects
    /// - **128-256**: High quality, captures fine details
    /// - **256+**: Very high quality, slow, for critical objects
    ///
    /// # Memory Usage
    ///
    /// Memory scales as O(resolution³) in 3D or O(resolution²) in 2D.
    /// Resolution 64 ≈ 260KB, Resolution 128 ≈ 2MB, Resolution 256 ≈ 16MB
    ///
    /// # Default
    ///
    /// - 2D: `256` (2D voxelization is much cheaper)
    /// - 3D: `64` (balances quality and performance)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::vhacd::VHACDParameters;
    ///
    /// // Quick preview
    /// let preview = VHACDParameters {
    ///     resolution: 32,
    ///     ..Default::default()
    /// };
    ///
    /// // Production quality
    /// let production = VHACDParameters {
    ///     resolution: 128,
    ///     ..Default::default()
    /// };
    /// # }
    /// ```
    pub resolution: u32,

    /// Granularity of the search for optimal clipping planes.
    ///
    /// During decomposition, the algorithm samples potential cutting planes along
    /// each axis. This parameter controls how many planes to skip between samples.
    /// Lower values mean more planes are tested (slower but more accurate).
    ///
    /// # Behavior
    ///
    /// - **1**: Test every possible plane (slowest, highest quality)
    /// - **4** (default): Test every 4th plane (good balance)
    /// - **8+**: Test fewer planes (faster, lower quality)
    ///
    /// # Performance Impact
    ///
    /// Reducing this value can significantly increase decomposition time, especially
    /// with high `resolution` values. Often doubled for the initial coarse pass.
    ///
    /// # Default
    ///
    /// `4`
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::vhacd::VHACDParameters;
    ///
    /// // Fast decomposition (coarse plane search)
    /// let fast = VHACDParameters {
    ///     plane_downsampling: 8,
    ///     ..Default::default()
    /// };
    ///
    /// // Precise decomposition (fine plane search)
    /// let precise = VHACDParameters {
    ///     plane_downsampling: 1,
    ///     ..Default::default()
    /// };
    /// # }
    /// ```
    pub plane_downsampling: u32,

    /// Precision of convex hull generation during plane selection.
    ///
    /// When evaluating potential cutting planes, the algorithm computes convex hulls
    /// of the resulting parts. This parameter controls how much to downsample the
    /// voxels before computing these hulls. Lower values = more precise but slower.
    ///
    /// # Behavior
    ///
    /// - **1**: Use all voxels (slowest, highest quality)
    /// - **4** (default): Use every 4th voxel (good balance)
    /// - **8+**: Use fewer voxels (faster, less precise)
    ///
    /// # Trade-off
    ///
    /// This primarily affects which cutting plane is chosen, not the final result.
    /// Higher downsampling can still produce good results much faster.
    ///
    /// # Default
    ///
    /// `4`
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::vhacd::VHACDParameters;
    ///
    /// // Faster hull computation
    /// let fast = VHACDParameters {
    ///     convex_hull_downsampling: 8,
    ///     ..Default::default()
    /// };
    /// # }
    /// ```
    pub convex_hull_downsampling: u32,

    /// How to fill the voxel grid from the input mesh or polyline.
    ///
    /// This controls the voxelization process, including how to handle the interior
    /// of the shape and whether to detect special features like cavities.
    ///
    /// # Options
    ///
    /// - `FloodFill { detect_cavities: false, detect_self_intersections: false }` (default):
    ///   Fast flood-fill from outside, works for most closed meshes
    /// - `FloodFill { detect_cavities: true, ... }`:
    ///   Also detect and preserve internal cavities (slower)
    /// - `SurfaceOnly`:
    ///   Only voxelize the surface, not the interior (for hollow shapes)
    ///
    /// # Default
    ///
    /// ```text
    /// FillMode::FloodFill {
    ///     detect_cavities: false,
    ///     detect_self_intersections: false,  // 2D only
    /// }
    /// ```
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::vhacd::VHACDParameters;
    /// use parry3d::transformation::voxelization::FillMode;
    ///
    /// // For meshes with internal cavities (e.g., a bottle)
    /// let with_cavities = VHACDParameters {
    ///     fill_mode: FillMode::FloodFill {
    ///         detect_cavities: true,
    ///     },
    ///     ..Default::default()
    /// };
    /// # }
    /// ```
    ///
    /// # See Also
    ///
    /// [`FillMode`] for detailed documentation on voxelization modes.
    pub fill_mode: FillMode,

    /// Whether to use approximate convex hulls during decomposition.
    ///
    /// When `true`, the algorithm uses a faster but less precise method to compute
    /// convex hulls during the decomposition process. This significantly speeds up
    /// decomposition with minimal impact on the final quality.
    ///
    /// # Behavior
    ///
    /// - **`true`** (default): Clip the parent's convex hull and add sample points
    ///   (fast, good quality)
    /// - **`false`**: Compute exact convex hulls from all voxels
    ///   (slower, slightly better quality)
    ///
    /// # Performance Impact
    ///
    /// Setting this to `false` can increase decomposition time by 2-5x, with often
    /// negligible quality improvement. Recommended to keep `true` for most cases.
    ///
    /// # Default
    ///
    /// `true`
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::vhacd::VHACDParameters;
    ///
    /// // Absolute highest quality (slow)
    /// let exact = VHACDParameters {
    ///     convex_hull_approximation: false,
    ///     ..Default::default()
    /// };
    /// # }
    /// ```
    pub convex_hull_approximation: bool,

    /// Maximum number of convex parts to generate.
    ///
    /// This acts as an upper limit on the number of convex hulls the algorithm will
    /// produce. The actual number may be less if the shape can be decomposed with
    /// fewer parts while meeting the `concavity` threshold.
    ///
    /// # Behavior
    ///
    /// The algorithm uses a binary tree decomposition with depth calculated to potentially
    /// produce up to `max_convex_hulls` parts. If parts are already approximately convex
    /// (concavity below threshold), they won't be split further.
    ///
    /// # Typical Values
    ///
    /// - **4-8**: Simple objects, performance-critical
    /// - **16-32**: Standard game objects, good balance
    /// - **32-64**: Complex objects, high quality
    /// - **64+**: Very complex objects, offline processing
    ///
    /// # Performance Note
    ///
    /// More parts = more collision checks during physics simulation. Keep this
    /// reasonable for dynamic objects (8-32). Static objects can afford more.
    ///
    /// # Default
    ///
    /// `1024` (effectively unlimited for most practical cases)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::vhacd::VHACDParameters;
    ///
    /// // Limit to reasonable number for game character
    /// let game_character = VHACDParameters {
    ///     max_convex_hulls: 16,
    ///     concavity: 0.02,
    ///     ..Default::default()
    /// };
    ///
    /// // Allow many parts for static environment
    /// let environment = VHACDParameters {
    ///     max_convex_hulls: 128,
    ///     concavity: 0.005,
    ///     ..Default::default()
    /// };
    /// # }
    /// ```
    pub max_convex_hulls: u32,
}

impl Default for VHACDParameters {
    fn default() -> Self {
        Self {
            #[cfg(feature = "dim3")]
            resolution: 64,
            #[cfg(feature = "dim3")]
            concavity: 0.01,
            #[cfg(feature = "dim2")]
            resolution: 256,
            #[cfg(feature = "dim2")]
            concavity: 0.1,
            plane_downsampling: 4,
            convex_hull_downsampling: 4,
            alpha: 0.05,
            beta: 0.05,
            convex_hull_approximation: true,
            max_convex_hulls: 1024,
            fill_mode: FillMode::FloodFill {
                detect_cavities: false,
                #[cfg(feature = "dim2")]
                detect_self_intersections: false,
            },
        }
    }
}
