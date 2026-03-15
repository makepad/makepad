use crate::math::{Pose, Vector};

use core::mem;

/// Result of a closest points query between two shapes.
///
/// This enum represents the outcome of computing closest points between two shapes,
/// taking into account a user-defined maximum search distance. It's useful for
/// proximity-based gameplay mechanics, AI perception systems, and efficient spatial
/// queries where you only care about nearby objects.
///
/// # Variants
///
/// The result depends on the relationship between the shapes and the `max_dist` parameter:
///
/// - **`Intersecting`**: Shapes are overlapping (penetrating or touching)
/// - **`WithinMargin`**: Shapes are separated but within the search distance
/// - **`Disjoint`**: Shapes are too far apart (beyond the search distance)
///
/// # Use Cases
///
/// - **AI perception**: Find nearest enemy within detection range
/// - **Trigger zones**: Detect objects approaching a threshold distance
/// - **LOD systems**: Compute detailed interactions only for nearby objects
/// - **Physics optimization**: Skip expensive computations for distant pairs
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::{closest_points, ClosestPoints};
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
///
/// let pos1 = Pose::translation(0.0, 0.0, 0.0);
/// let pos2 = Pose::translation(5.0, 0.0, 0.0);
///
/// // Search for closest points within 10.0 units
/// let result = closest_points(&pos1, &ball1, &pos2, &ball2, 10.0).unwrap();
///
/// match result {
///     ClosestPoints::Intersecting => {
///         println!("Shapes are overlapping!");
///     }
///     ClosestPoints::WithinMargin(pt1, pt2) => {
///         println!("Closest point on shape1: {:?}", pt1);
///         println!("Closest point on shape2: {:?}", pt2);
///         let distance = (pt2 - pt1).length();
///         println!("Distance between shapes: {}", distance);
///     }
///     ClosestPoints::Disjoint => {
///         println!("Shapes are more than 10.0 units apart");
///     }
/// }
/// # }
/// ```
///
/// # See Also
///
/// - [`closest_points`](crate::query::closest_points::closest_points()) - Main function to compute this result
/// - [`distance`](crate::query::distance::distance()) - For just the distance value
/// - [`contact`](crate::query::contact::contact()) - For detailed contact information when intersecting
#[derive(Debug, PartialEq, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
pub enum ClosestPoints {
    /// The two shapes are intersecting (overlapping or touching).
    ///
    /// When shapes intersect, their closest points are not well-defined, as there
    /// are infinitely many contact points. Use [`contact`](crate::query::contact::contact())
    /// instead to get penetration depth and contact normals.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::{closest_points, ClosestPoints};
    /// use parry3d::shape::Ball;
    /// use parry3d::math::Pose;
    ///
    /// let ball1 = Ball::new(2.0);
    /// let ball2 = Ball::new(2.0);
    ///
    /// // Overlapping balls (centers 3.0 apart, combined radii 4.0)
    /// let pos1 = Pose::translation(0.0, 0.0, 0.0);
    /// let pos2 = Pose::translation(3.0, 0.0, 0.0);
    ///
    /// let result = closest_points(&pos1, &ball1, &pos2, &ball2, 10.0).unwrap();
    /// assert_eq!(result, ClosestPoints::Intersecting);
    /// # }
    /// ```
    Intersecting,

    /// The shapes are separated but within the specified maximum distance.
    ///
    /// Contains the two closest points in world-space coordinates:
    /// - First point: Closest point on the surface of the first shape
    /// - Second point: Closest point on the surface of the second shape
    ///
    /// The distance between the shapes equals the distance between these two points.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::{closest_points, ClosestPoints};
    /// use parry3d::shape::Ball;
    /// use parry3d::math::Pose;
    ///
    /// let ball1 = Ball::new(1.0);
    /// let ball2 = Ball::new(1.0);
    ///
    /// // Balls separated by 3.0 units (centers 5.0 apart, combined radii 2.0)
    /// let pos1 = Pose::translation(0.0, 0.0, 0.0);
    /// let pos2 = Pose::translation(5.0, 0.0, 0.0);
    ///
    /// let result = closest_points(&pos1, &ball1, &pos2, &ball2, 10.0).unwrap();
    ///
    /// if let ClosestPoints::WithinMargin(pt1, pt2) = result {
    ///     // Vectors are on the surface, facing each other
    ///     assert!((pt1.x - 1.0).abs() < 1e-5); // ball1 surface at x=1.0
    ///     assert!((pt2.x - 4.0).abs() < 1e-5); // ball2 surface at x=4.0
    ///
    ///     // Distance between points
    ///     let dist = (pt2 - pt1).length();
    ///     assert!((dist - 3.0).abs() < 1e-5);
    /// } else {
    ///     panic!("Expected WithinMargin");
    /// }
    /// # }
    /// ```
    WithinMargin(Vector, Vector),

    /// The shapes are separated by more than the specified maximum distance.
    ///
    /// The actual distance between shapes is unknown (could be much larger than `max_dist`),
    /// and no closest points are computed. This saves computation when you only care
    /// about nearby objects.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::{closest_points, ClosestPoints};
    /// use parry3d::shape::Ball;
    /// use parry3d::math::Pose;
    ///
    /// let ball1 = Ball::new(1.0);
    /// let ball2 = Ball::new(1.0);
    ///
    /// // Balls separated by 8.0 units
    /// let pos1 = Pose::translation(0.0, 0.0, 0.0);
    /// let pos2 = Pose::translation(10.0, 0.0, 0.0);
    ///
    /// // Only search within 5.0 units
    /// let result = closest_points(&pos1, &ball1, &pos2, &ball2, 5.0).unwrap();
    /// assert_eq!(result, ClosestPoints::Disjoint);
    ///
    /// // With larger search distance, we get the points
    /// let result2 = closest_points(&pos1, &ball1, &pos2, &ball2, 10.0).unwrap();
    /// assert!(matches!(result2, ClosestPoints::WithinMargin(_, _)));
    /// # }
    /// ```
    Disjoint,
}

impl ClosestPoints {
    /// Swaps the two closest points in-place.
    ///
    /// This is useful when you need to reverse the perspective of the query result.
    /// If the result is `WithinMargin`, the two points are swapped. For `Intersecting`
    /// and `Disjoint`, this operation has no effect.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::{closest_points, ClosestPoints};
    /// use parry3d::shape::Ball;
    /// use parry3d::math::Pose;
    ///
    /// let ball1 = Ball::new(1.0);
    /// let ball2 = Ball::new(2.0);
    ///
    /// let pos1 = Pose::translation(0.0, 0.0, 0.0);
    /// let pos2 = Pose::translation(5.0, 0.0, 0.0);
    ///
    /// let mut result = closest_points(&pos1, &ball1, &pos2, &ball2, 10.0).unwrap();
    ///
    /// if let ClosestPoints::WithinMargin(pt1_before, pt2_before) = result {
    ///     result.flip();
    ///     if let ClosestPoints::WithinMargin(pt1_after, pt2_after) = result {
    ///         assert_eq!(pt1_before, pt2_after);
    ///         assert_eq!(pt2_before, pt1_after);
    ///     }
    /// }
    /// # }
    /// ```
    pub fn flip(&mut self) {
        if let ClosestPoints::WithinMargin(ref mut p1, ref mut p2) = *self {
            mem::swap(p1, p2)
        }
    }

    /// Returns a copy with the two closest points swapped.
    ///
    /// This is the non-mutating version of [`flip`](Self::flip). It returns a new
    /// `ClosestPoints` with swapped points if the result is `WithinMargin`, or returns
    /// `self` unchanged for `Intersecting` and `Disjoint`.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::{closest_points, ClosestPoints};
    /// use parry3d::shape::Ball;
    /// use parry3d::math::Pose;
    ///
    /// let ball1 = Ball::new(1.0);
    /// let ball2 = Ball::new(2.0);
    ///
    /// let pos1 = Pose::translation(0.0, 0.0, 0.0);
    /// let pos2 = Pose::translation(5.0, 0.0, 0.0);
    ///
    /// let result = closest_points(&pos1, &ball1, &pos2, &ball2, 10.0).unwrap();
    /// let flipped = result.flipped();
    ///
    /// // Original is unchanged
    /// if let ClosestPoints::WithinMargin(pt1_orig, pt2_orig) = result {
    ///     if let ClosestPoints::WithinMargin(pt1_flip, pt2_flip) = flipped {
    ///         assert_eq!(pt1_orig, pt2_flip);
    ///         assert_eq!(pt2_orig, pt1_flip);
    ///     }
    /// }
    /// # }
    /// ```
    #[must_use]
    pub fn flipped(&self) -> Self {
        if let ClosestPoints::WithinMargin(p1, p2) = *self {
            ClosestPoints::WithinMargin(p2, p1)
        } else {
            *self
        }
    }

    /// Transforms the closest points from local space to world space.
    ///
    /// Applies the isometry transformations to convert closest points from their
    /// respective local coordinate frames to world-space coordinates. This is used
    /// internally by the query system.
    ///
    /// - Vector 1 is transformed by `pos1`
    /// - Vector 2 is transformed by `pos2`
    /// - `Intersecting` and `Disjoint` variants are returned unchanged
    ///
    /// # Arguments
    ///
    /// * `pos1` - Transformation for the first shape
    /// * `pos2` - Transformation for the second shape
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::ClosestPoints;
    /// use parry3d::math::{Pose, Vector};
    ///
    /// // Vectors in local space
    /// let local_result = ClosestPoints::WithinMargin(
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(-1.0, 0.0, 0.0),
    /// );
    ///
    /// // Transform to world space
    /// let pos1 = Pose::translation(10.0, 0.0, 0.0);
    /// let pos2 = Pose::translation(20.0, 0.0, 0.0);
    ///
    /// let world_result = local_result.transform_by(&pos1, &pos2);
    ///
    /// if let ClosestPoints::WithinMargin(pt1, pt2) = world_result {
    ///     assert!((pt1.x - 11.0).abs() < 1e-5); // 10.0 + 1.0
    ///     assert!((pt2.x - 19.0).abs() < 1e-5); // 20.0 + (-1.0)
    /// }
    /// # }
    /// ```
    #[must_use]
    pub fn transform_by(self, pos1: &Pose, pos2: &Pose) -> Self {
        if let ClosestPoints::WithinMargin(p1, p2) = self {
            ClosestPoints::WithinMargin(pos1 * p1, pos2 * p2)
        } else {
            self
        }
    }
}
