/// Errors that can occur during convex hull computation.
///
/// When computing the convex hull of a set of points using [`convex_hull`] or [`try_convex_hull`],
/// various problems can arise due to invalid input data or numerical issues. This enum describes
/// all possible error conditions.
///
/// # Overview
///
/// Convex hull computation uses incremental algorithms that build the hull by adding points one at a time.
/// The algorithm can fail if the input is degenerate (too few points, collinear/coplanar points) or
/// contains invalid data (NaN values, duplicates).
///
/// # Common Causes and Solutions
///
/// ## Input Validation Issues
///
/// - **Too few points**: Need at least 4 non-coplanar points for 3D, or 3 non-collinear points for 2D
/// - **Invalid coordinates**: Check for NaN or infinite values in your point data
/// - **Duplicate points**: Remove duplicate points before computing the hull
/// - **Degenerate geometry**: Ensure points are not all collinear (2D) or coplanar (3D)
///
/// ## How to Handle Errors
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::{try_convex_hull, ConvexHullError};
/// use parry3d::math::Vector;
///
/// let points = vec![
///     Vector::ZERO,
///     Vector::new(1.0, 0.0, 0.0),
///     Vector::new(0.0, 1.0, 0.0),
///     Vector::new(0.0, 0.0, 1.0),
/// ];
///
/// match try_convex_hull(&points) {
///     Ok((vertices, indices)) => {
///         println!("Successfully computed hull with {} faces", indices.len());
///     }
///     Err(ConvexHullError::IncompleteInput) => {
///         println!("Not enough points provided (need at least 4 in 3D)");
///     }
///     Err(ConvexHullError::MissingSupportPoint) => {
///         println!("Vectors are invalid (NaN) or nearly coplanar");
///         // Try removing duplicate points or checking for degeneracies
///     }
///     Err(ConvexHullError::DuplicatePoints(i, j)) => {
///         println!("Vectors {} and {} are duplicates", i, j);
///         // Remove duplicates and try again
///     }
///     Err(err) => {
///         println!("Unexpected error: {}", err);
///     }
/// }
/// # }
/// ```
///
/// [`convex_hull`]: crate::transformation::convex_hull
/// [`try_convex_hull`]: crate::transformation::try_convex_hull
#[derive(Debug, PartialEq)]
pub enum ConvexHullError {
    /// An internal error occurred during convex hull computation.
    ///
    /// This indicates a bug in the convex hull algorithm itself. If you encounter this error,
    /// please report it as a bug with a minimal reproducible example.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim3", feature = "f32", feature = "alloc"))] {
    /// # use parry3d::transformation::{try_convex_hull, ConvexHullError};
    /// # use parry3d::math::Vector;
    /// # let points = vec![Vector::ZERO];
    /// match try_convex_hull(&points) {
    ///     Err(ConvexHullError::InternalError(msg)) => {
    ///         eprintln!("Bug in convex hull algorithm: {}", msg);
    ///         // This should not happen - please report this!
    ///     }
    ///     _ => {}
    /// }
    /// # }
    /// ```
    InternalError(&'static str),

    /// The algorithm could not find a valid support point.
    ///
    /// This error occurs when:
    /// 1. The input contains points with NaN or infinite coordinates
    /// 2. All points are nearly collinear (in 2D) or coplanar (in 3D)
    /// 3. The numerical precision is insufficient to distinguish between points
    ///
    /// # Common Causes
    ///
    /// - **NaN values**: Check your input data for NaN coordinates
    /// - **Nearly flat geometry**: Vectors lie almost on a line (2D) or plane (3D)
    /// - **Numerical precision**: Vectors are too close together relative to floating-point precision
    ///
    //    /// TODO: check why this doc-test is failing.
    //    /// # How to Fix
    //    ///
    //    /// ```
    //    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    //    /// use parry3d::transformation::try_convex_hull;
    //    /// use parry3d::math::Vector;
    //    ///
    //    /// let points = vec![
    //    ///     Vector::ZERO,
    //    ///     Vector::new(1.0, 0.0, 0.0),
    //    ///     Vector::new(2.0, 0.0, 0.0),  // Collinear!
    //    /// ];
    //    ///
    //    /// // This will fail because points are collinear
    //    /// assert!(try_convex_hull(&points).is_err());
    //    ///
    //    /// // Add a point out of the line
    //    /// let mut fixed_points = points.clone();
    //    /// fixed_points.push(Vector::new(0.0, 1.0, 0.0));
    //    /// fixed_points.push(Vector::new(0.0, 0.0, 1.0));
    //    ///
    //    /// // Now it should work
    //    /// assert!(try_convex_hull(&fixed_points).is_ok());
    //    /// # }
    //    /// ```
    MissingSupportPoint,

    /// Not enough points were provided to compute a convex hull.
    ///
    /// A convex hull requires:
    /// - **3D (dim3)**: At least 4 non-coplanar points to form a tetrahedron
    /// - **2D (dim2)**: At least 3 non-collinear points to form a triangle
    ///
    /// Providing fewer points than this minimum results in this error.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::{try_convex_hull, ConvexHullError};
    /// use parry3d::math::Vector;
    ///
    /// // Only 2 points - not enough for 3D hull
    /// let points = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    /// ];
    ///
    /// match try_convex_hull(&points) {
    ///     Err(ConvexHullError::IncompleteInput) => {
    ///         println!("Need at least 4 points for 3D convex hull");
    ///     }
    ///     _ => {}
    /// }
    /// # }
    /// ```
    IncompleteInput,

    /// Internal error: reached an unreachable code path.
    ///
    /// This should never happen and indicates a serious bug in the implementation.
    /// If you encounter this, please report it with your input data.
    Unreachable,

    /// A triangle in the hull was not properly constructed.
    ///
    /// This is an internal consistency error that indicates the algorithm failed to
    /// maintain the correct half-edge topology during hull construction.
    ///
    /// This is likely caused by numerical precision issues or edge cases in the input geometry.
    UnfinishedTriangle,

    /// Detected a T-junction in the hull topology.
    ///
    /// A T-junction occurs when an edge has more than two adjacent faces, which violates
    /// the manifold property required for a valid convex hull. This is an internal error
    /// that shouldn't occur with valid convex hull computation.
    ///
    /// The error reports:
    /// - `0`: The triangle index where the T-junction was detected
    /// - `1`, `2`: The vertex indices forming the problematic edge
    ///
    /// # Example
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "dim3", feature = "f32", feature = "alloc"))] {
    /// # use parry3d::transformation::{try_convex_hull, ConvexHullError};
    /// # use parry3d::math::Vector;
    /// # let points = vec![Vector::ZERO];
    /// match try_convex_hull(&points) {
    ///     Err(ConvexHullError::TJunction(tri_id, v1, v2)) => {
    ///         eprintln!("T-junction at triangle {} on edge ({}, {})", tri_id, v1, v2);
    ///     }
    ///     _ => {}
    /// }
    /// # }
    /// ```
    TJunction(usize, u32, u32),

    /// The input contains duplicate points at the same location.
    ///
    /// This error is raised during validation when two points have identical coordinates.
    /// Duplicate points can cause issues with the hull topology and should be removed
    /// before computing the hull.
    ///
    /// The error reports the indices of the two duplicate points.
    ///
    /// # How to Fix
    ///
    /// Remove duplicate points from your input data:
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::transformation::try_convex_hull;
    /// use parry3d::math::Vector;
    ///
    /// let points = vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0),
    ///     Vector::ZERO,  // Duplicate!
    /// ];
    ///
    /// // Remove duplicates (note: this is a simple example, not production code)
    /// fn remove_duplicates(points: Vec<Vector>) -> Vec<Vector> {
    ///     let mut seen: Vec<Vector> = Vec::new();
    ///     let mut result = Vec::new();
    ///     for pt in points {
    ///         if !seen.iter().any(|p| (*p - pt).length() < 1e-6) {
    ///             seen.push(pt);
    ///             result.push(pt);
    ///         }
    ///     }
    ///     result
    /// }
    ///
    /// let unique_points = remove_duplicates(points);
    /// assert!(try_convex_hull(&unique_points).is_ok());
    /// # }
    /// ```
    DuplicatePoints(usize, usize),
}

impl fmt::Display for ConvexHullError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InternalError(message) => write!(f, "Internal error: {message}"),
            Self::MissingSupportPoint => {
                f.write_str("Input points are either invalid (NaN) or are almost coplanar.")
            }
            Self::IncompleteInput => {
                f.write_str("Less than 3 points were given to the convex-hull algorithm.")
            }
            Self::Unreachable => f.write_str("Internal error: unreachable code path"),
            Self::UnfinishedTriangle => f.write_str("Detected unfinished triangle"),
            Self::TJunction(triangle, v1, v2) => {
                write!(f, "Detected t-junction for triangle {triangle}, edge: ({v1}, {v2})")
            }
            Self::DuplicatePoints(i, j) => write!(f, "Detected duplicate points {i} and {j}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ConvexHullError {}
use core::fmt;
