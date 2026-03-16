#[cfg(feature = "dim3")]
use crate::math::{Real, Vector};

/// Returns the index of the support point of a list of points.
///
/// The support point is the point that extends furthest in the given direction.
/// This is a fundamental operation used in convex hull algorithms and collision
/// detection (especially GJK/EPA algorithms).
///
/// # Arguments
/// * `direction` - The direction vector to test against
/// * `points` - A slice of points to search
///
/// # Returns
/// * `Some(index)` - Index of the support point (furthest in the given direction)
/// * `None` - If the points slice is empty
///
/// # Example
///
/// ```ignore
/// # // This is a pub(crate) function. Can't really run it.
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::convex_hull_utils::support_point_id;
/// use parry3d::na::{Vector3, Vector3};
///
/// let points = vec![
///     Vector::ZERO,
///     Vector::new(1.0, 0.0, 0.0),
///     Vector::new(0.0, 1.0, 0.0),
///     Vector::new(0.0, 0.0, 1.0),
/// ];
///
/// // Find point furthest in the positive X direction
/// let dir = Vector::new(1.0, 0.0, 0.0);
/// let support_id = support_point_id(&dir, &points);
/// assert_eq!(support_id, Some(1)); // Vector at (1, 0, 0)
///
/// // Find point furthest in the positive Y direction
/// let dir = Vector::new(0.0, 1.0, 0.0);
/// let support_id = support_point_id(&dir, &points);
/// assert_eq!(support_id, Some(2)); // Vector at (0, 1, 0)
/// # }
/// ```
#[cfg(feature = "dim3")]
pub fn support_point_id(direction: Vector, points: &[Vector]) -> Option<usize> {
    let mut argmax = None;
    let mut max = -Real::MAX;

    for (id, pt) in points.iter().enumerate() {
        let dot = direction.dot(*pt);

        if dot > max {
            argmax = Some(id);
            max = dot;
        }
    }

    argmax
}

/// Returns the index of the support point of an indexed list of points.
///
/// This is similar to [`support_point_id`], but only considers points at the indices
/// provided by the iterator. This is useful when you want to find the support point
/// within a subset of points without creating a new array.
///
/// # Arguments
/// * `direction` - The direction vector to test against
/// * `points` - The full array of points
/// * `idx` - Iterator yielding indices of points to consider
///
/// # Returns
/// * `Some(index)` - Index into the original `points` array of the support point
/// * `None` - If the iterator is empty
///
/// # Example
///
/// ```ignore
/// # // This is a pub(crate) function. Can't really run it.
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::convex_hull_utils::indexed_support_point_id;
/// use parry3d::na::{Vector3, Vector3};
///
/// let points = vec![
///     Vector::ZERO,
///     Vector::new(1.0, 0.0, 0.0),
///     Vector::new(2.0, 0.0, 0.0),
///     Vector::new(0.0, 1.0, 0.0),
/// ];
///
/// // Only consider points at indices 0, 1, and 3 (skip index 2)
/// let subset = vec![0, 1, 3];
/// let dir = Vector::new(1.0, 0.0, 0.0);
///
/// let support_id = indexed_support_point_id(&dir, &points, subset.into_iter());
/// // Returns 1 (not 2, since we skipped that index)
/// assert_eq!(support_id, Some(1));
/// # }
/// ```
#[cfg(feature = "dim3")]
pub fn indexed_support_point_id<I>(direction: Vector, points: &[Vector], idx: I) -> Option<usize>
where
    I: Iterator<Item = usize>,
{
    let mut argmax = None;
    let mut max = -Real::MAX;

    for i in idx.into_iter() {
        let dot = direction.dot(points[i]);

        if dot > max {
            argmax = Some(i);
            max = dot;
        }
    }

    argmax
}

/// Returns the position in the iterator where the support point is found.
///
/// This is similar to [`indexed_support_point_id`], but returns the position within
/// the iterator rather than the index in the original points array. In other words,
/// if the iterator yields indices `[5, 7, 2, 9]` and the support point is at index 2,
/// this function returns `Some(2)` (the 3rd position in the iterator), not `Some(2)`.
///
/// # Arguments
/// * `direction` - The direction vector to test against
/// * `points` - The full array of points
/// * `idx` - Iterator yielding indices of points to consider
///
/// # Returns
/// * `Some(n)` - The `n`th element of the iterator corresponds to the support point
/// * `None` - If the iterator is empty
///
/// # Example
///
/// ```ignore
/// # // This is a pub(crate) function. Can’t really run it.
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::convex_hull_utils::indexed_support_point_nth;
/// use parry3d::na::{Vector3, Vector3};
///
/// let points = vec![
///     Vector::ZERO,  // index 0
///     Vector::new(1.0, 0.0, 0.0),  // index 1
///     Vector::new(5.0, 0.0, 0.0),  // index 2
///     Vector::new(0.0, 1.0, 0.0),  // index 3
/// ];
///
/// // Consider points at indices [3, 0, 2, 1]
/// let indices = vec![3, 0, 2, 1];
/// let dir = Vector::new(1.0, 0.0, 0.0);
///
/// let nth = indexed_support_point_nth(&dir, &points, indices.into_iter());
/// // The support point is at original index 2, which is position 2 in our iterator
/// assert_eq!(nth, Some(2));
/// # }
/// ```
#[cfg(feature = "dim3")] // We only use this in 3D right now.
pub fn indexed_support_point_nth<I>(direction: Vector, points: &[Vector], idx: I) -> Option<usize>
where
    I: Iterator<Item = usize>,
{
    let mut argmax = None;
    let mut max = -Real::MAX;

    for (k, i) in idx.into_iter().enumerate() {
        let dot = direction.dot(points[i]);

        if dot > max {
            argmax = Some(k);
            max = dot;
        }
    }

    argmax
}

/// Normalizes a point cloud by centering and scaling it to fit within a unit cube.
///
/// This function computes the axis-aligned bounding box (AABB) of the input points,
/// then translates all points so they're centered at the origin, and scales them
/// so the bounding box diagonal has length 1. This normalization is useful for
/// improving numerical stability in geometric algorithms like convex hull computation.
///
/// The transformation is reversible using the returned center and scale values.
///
/// # Arguments
/// * `coords` - Mutable slice of points to normalize in-place
///
/// # Returns
/// A tuple `(center, scale)` where:
/// * `center` - The original center point of the AABB (before normalization)
/// * `scale` - The original AABB diagonal length (before normalization)
///
/// To reverse the transformation: `original_point = normalized_point * scale + center`
///
/// # Example
///
/// ```ignore
/// # // This is a pub(crate) function. Can’t really run it.
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::transformation::convex_hull_utils::normalize;
/// use parry3d::math::Vector;
///
/// let mut points = vec![
///     Vector::new(10.0, 10.0, 10.0),
///     Vector::new(20.0, 20.0, 20.0),
///     Vector::new(15.0, 15.0, 15.0),
/// ];
///
/// let (center, scale) = normalize(&mut points);
///
/// // Vectors are now centered around origin and scaled
/// // The AABB diagonal is now approximately 1.0
/// println!("Original center: {:?}", center);
/// println!("Original scale: {}", scale);
///
/// // To recover original points:
/// for p in &mut points {
///     *p = *p * scale + center;
/// }
/// # }
/// ```
#[cfg(feature = "dim3")]
pub fn normalize(coords: &mut [Vector]) -> (Vector, Real) {
    let aabb = crate::bounding_volume::details::local_point_cloud_aabb_ref(&*coords);
    let diag = aabb.mins.distance(aabb.maxs);
    let center = aabb.center();

    for c in coords.iter_mut() {
        *c = (*c - center) / diag;
    }

    (center, diag)
}
