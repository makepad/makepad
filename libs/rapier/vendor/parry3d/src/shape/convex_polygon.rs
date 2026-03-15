use crate::math::{ComplexField, Real, RealField, Vector};
use crate::shape::{FeatureId, PackedFeatureId, PolygonalFeature, PolygonalFeatureMap, SupportMap};
use crate::utils;
use alloc::vec::Vec;

/// A 2D convex polygon.
///
/// A convex polygon is a closed 2D shape where all interior angles are less than 180 degrees,
/// and any line segment drawn between two points inside the polygon stays entirely within the polygon.
/// Common examples include triangles, rectangles, and regular polygons like hexagons.
///
/// # What is a convex polygon?
///
/// In 2D space, a polygon is **convex** if:
/// - Every interior angle is less than or equal to 180 degrees
/// - The line segment between any two points inside the polygon lies entirely inside the polygon
/// - All vertices "bulge outward" - there are no indentations or concave areas
///
/// Examples of **convex** polygons: triangle, square, regular pentagon, regular hexagon
/// Examples of **non-convex** (concave) polygons: star shapes, L-shapes, crescents
///
/// # Use cases
///
/// Convex polygons are widely used in:
/// - **Game development**: Character hitboxes, platform boundaries, simple building shapes
/// - **Physics simulations**: Rigid body collision detection (more efficient than arbitrary polygons)
/// - **Robotics**: Simplified environment representations, obstacle boundaries
/// - **Computer graphics**: Fast rendering primitives, clipping regions
/// - **Computational geometry**: As building blocks for more complex operations
///
/// # Representation
///
/// This structure stores:
/// - **Vectors**: The vertices of the polygon in counter-clockwise order
/// - **Normals**: Unit vectors perpendicular to each edge, pointing outward
///
/// The normals are pre-computed for efficient collision detection algorithms.
///
/// # Example: Creating a simple triangle
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::shape::ConvexPolygon;
/// use parry2d::math::Vector;
///
/// // Create a triangle from three vertices (counter-clockwise order)
/// let vertices = vec![
///     Vector::ZERO,    // bottom-left
///     Vector::new(2.0, 0.0),    // bottom-right
///     Vector::new(1.0, 2.0),    // top
/// ];
///
/// let triangle = ConvexPolygon::from_convex_polyline(vertices)
///     .expect("Failed to create triangle");
///
/// // The polygon has 3 vertices
/// assert_eq!(triangle.points().len(), 3);
/// // and 3 edge normals (one per edge)
/// assert_eq!(triangle.normals().len(), 3);
/// # }
/// ```
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(Clone, Debug)]
pub struct ConvexPolygon {
    points: Vec<Vector>,
    normals: Vec<Vector>,
}

impl ConvexPolygon {
    /// Creates a new 2D convex polygon from an arbitrary set of points by computing their convex hull.
    ///
    /// This is the most flexible constructor - it automatically computes the **convex hull** of the
    /// given points, which is the smallest convex polygon that contains all the input points.
    /// Think of it as wrapping a rubber band around the points.
    ///
    /// Use this when:
    /// - You have an arbitrary collection of points and want the convex boundary
    /// - You're not sure if your points form a convex shape
    /// - You want to simplify a point cloud to its convex outline
    ///
    /// # Returns
    ///
    /// - `Some(ConvexPolygon)` if successful
    /// - `None` if the convex hull computation failed (e.g., all points are collinear or coincident)
    ///
    /// # Example: Creating a convex polygon from arbitrary points
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::ConvexPolygon;
    /// use parry2d::math::Vector;
    ///
    /// // Some arbitrary points (including one inside the convex hull)
    /// let points = vec![
    ///     Vector::ZERO,
    ///     Vector::new(4.0, 0.0),
    ///     Vector::new(2.0, 3.0),
    ///     Vector::new(2.0, 1.0),  // This point is inside the triangle
    /// ];
    ///
    /// let polygon = ConvexPolygon::from_convex_hull(&points)
    ///     .expect("Failed to create convex hull");
    ///
    /// // The convex hull only has 3 vertices (the interior point was excluded)
    /// assert_eq!(polygon.points().len(), 3);
    /// # }
    /// ```
    ///
    /// # Example: Simplifying a point cloud
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::ConvexPolygon;
    /// use parry2d::math::Vector;
    ///
    /// // A cloud of points that roughly forms a circle
    /// let mut points = Vec::new();
    /// for i in 0..20 {
    ///     let angle = (i as f32) * std::f32::consts::TAU / 20.0;
    ///     points.push(Vector::new(angle.cos(), angle.sin()));
    /// }
    /// // Add some interior points
    /// points.push(Vector::ZERO);
    /// points.push(Vector::new(0.5, 0.5));
    ///
    /// let polygon = ConvexPolygon::from_convex_hull(&points)
    ///     .expect("Failed to create convex hull");
    ///
    /// // The convex hull has 20 vertices (the boundary points)
    /// assert_eq!(polygon.points().len(), 20);
    /// # }
    /// ```
    pub fn from_convex_hull(points: &[Vector]) -> Option<Self> {
        let vertices = crate::transformation::convex_hull(points);
        Self::from_convex_polyline(vertices)
    }

    /// Creates a new 2D convex polygon from vertices that already form a convex shape.
    ///
    /// This constructor is more efficient than [`from_convex_hull`] because it **assumes** the input
    /// points already form a convex polygon in counter-clockwise order, and it doesn't compute the
    /// convex hull. The convexity is **not verified** - if you pass non-convex points, the resulting
    /// shape may behave incorrectly in collision detection.
    ///
    /// **Important**: Vectors must be ordered **counter-clockwise** (CCW). If you're unsure about the
    /// ordering or convexity, use [`from_convex_hull`] instead.
    ///
    /// This method automatically removes collinear vertices (points that lie on the line between
    /// their neighbors) to simplify the polygon. If you want to preserve all vertices exactly as given,
    /// use [`from_convex_polyline_unmodified`].
    ///
    /// # When to use this
    ///
    /// Use this constructor when:
    /// - You already know your points form a convex polygon
    /// - The points are ordered counter-clockwise around the shape
    /// - You want better performance by skipping convex hull computation
    ///
    /// # Returns
    ///
    /// - `Some(ConvexPolygon)` if successful
    /// - `None` if all points are nearly collinear (form an almost flat line) or there are fewer than 3 vertices after removing collinear points
    ///
    /// # Example: Creating a square
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::ConvexPolygon;
    /// use parry2d::math::Vector;
    ///
    /// // A square with vertices in counter-clockwise order
    /// let square = ConvexPolygon::from_convex_polyline(vec![
    ///     Vector::ZERO,  // bottom-left
    ///     Vector::new(1.0, 0.0),  // bottom-right
    ///     Vector::new(1.0, 1.0),  // top-right
    ///     Vector::new(0.0, 1.0),  // top-left
    /// ]).expect("Failed to create square");
    ///
    /// assert_eq!(square.points().len(), 4);
    /// # }
    /// ```
    ///
    /// # Example: Collinear points are automatically removed
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::ConvexPolygon;
    /// use parry2d::math::Vector;
    ///
    /// // A quadrilateral with one vertex on an edge (making it collinear)
    /// let polygon = ConvexPolygon::from_convex_polyline(vec![
    ///     Vector::ZERO,
    ///     Vector::new(2.0, 0.0),
    ///     Vector::new(2.0, 1.0),   // This point is on the line from (2,0) to (2,2)
    ///     Vector::new(2.0, 2.0),
    ///     Vector::new(0.0, 2.0),
    /// ]).expect("Failed to create polygon");
    ///
    /// // The collinear point at (2.0, 1.0) was removed, leaving a rectangle
    /// assert_eq!(polygon.points().len(), 4);
    /// # }
    /// ```
    ///
    /// [`from_convex_hull`]: ConvexPolygon::from_convex_hull
    /// [`from_convex_polyline_unmodified`]: ConvexPolygon::from_convex_polyline_unmodified
    pub fn from_convex_polyline(mut points: Vec<Vector>) -> Option<Self> {
        if points.is_empty() {
            return None;
        }
        let eps = <Real as ComplexField>::sqrt(crate::math::DEFAULT_EPSILON);
        let mut normals = Vec::with_capacity(points.len());
        // First, compute all normals.
        for i1 in 0..points.len() {
            let i2 = (i1 + 1) % points.len();
            normals.push(utils::ccw_face_normal([points[i1], points[i2]])?);
        }

        let mut nremoved = 0;
        // See if the first vertex must be removed.
        if normals[0].dot(normals[normals.len() - 1]) > 1.0 - eps {
            nremoved = 1;
        }

        // Second, find vertices that can be removed because
        // of collinearity of adjacent faces.
        for i2 in 1..points.len() {
            let i1 = i2 - 1;
            if normals[i1].dot(normals[i2]) > 1.0 - eps {
                // Remove
                nremoved += 1;
            } else {
                points[i2 - nremoved] = points[i2];
                normals[i2 - nremoved] = normals[i2];
            }
        }

        let new_length = points.len() - nremoved;
        points.truncate(new_length);
        normals.truncate(new_length);

        if points.len() > 2 {
            Some(ConvexPolygon { points, normals })
        } else {
            None
        }
    }

    /// Creates a new 2D convex polygon from a set of points assumed to
    /// describe a counter-clockwise convex polyline.
    ///
    /// This is the same as [`ConvexPolygon::from_convex_polyline`] but without removing any point
    /// from the input even if some are coplanar.
    ///
    /// Returns `None` if `points` doesn’t contain at least three points.
    pub fn from_convex_polyline_unmodified(points: Vec<Vector>) -> Option<Self> {
        if points.len() <= 2 {
            return None;
        }
        let mut normals = Vec::with_capacity(points.len());
        // First, compute all normals.
        for i1 in 0..points.len() {
            let i2 = (i1 + 1) % points.len();
            normals.push(utils::ccw_face_normal([points[i1], points[i2]])?);
        }

        Some(ConvexPolygon { points, normals })
    }

    /// Returns the vertices of this convex polygon.
    ///
    /// The vertices are stored in counter-clockwise order around the polygon.
    /// This is a slice reference to the internal vertex storage.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::ConvexPolygon;
    /// use parry2d::math::Vector;
    ///
    /// let triangle = ConvexPolygon::from_convex_polyline(vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0),
    ///     Vector::new(0.5, 1.0),
    /// ]).unwrap();
    ///
    /// let vertices = triangle.points();
    /// assert_eq!(vertices.len(), 3);
    /// assert_eq!(vertices[0], Vector::ZERO);
    /// # }
    /// ```
    #[inline]
    pub fn points(&self) -> &[Vector] {
        &self.points
    }

    /// Returns the outward-pointing normals of the edges of this convex polygon.
    ///
    /// Each normal is a unit vector perpendicular to an edge, pointing outward from the polygon.
    /// The normals are stored in the same order as the edges, so `normals()[i]` is the normal
    /// for the edge from `points()[i]` to `points()[(i+1) % len]`.
    ///
    /// These pre-computed normals are used internally for efficient collision detection.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::ConvexPolygon;
    /// use parry2d::math::Vector;
    ///
    /// // Create a square aligned with the axes
    /// let square = ConvexPolygon::from_convex_polyline(vec![
    ///     Vector::ZERO,  // bottom-left
    ///     Vector::new(1.0, 0.0),  // bottom-right
    ///     Vector::new(1.0, 1.0),  // top-right
    ///     Vector::new(0.0, 1.0),  // top-left
    /// ]).unwrap();
    ///
    /// let normals = square.normals();
    /// assert_eq!(normals.len(), 4);
    ///
    /// // The first normal points downward (perpendicular to bottom edge)
    /// let bottom_normal = normals[0];
    /// assert!((bottom_normal.y - (-1.0)).abs() < 1e-5);
    /// assert!(bottom_normal.x.abs() < 1e-5);
    /// # }
    /// ```
    #[inline]
    pub fn normals(&self) -> &[Vector] {
        &self.normals
    }

    /// Computes a scaled version of this convex polygon.
    ///
    /// This method scales the polygon by multiplying each vertex coordinate by the corresponding
    /// component of the `scale` vector. This allows for non-uniform scaling (different scale factors
    /// for x and y axes).
    ///
    /// The normals are also updated to reflect the scaling transformation.
    ///
    /// # Returns
    ///
    /// - `Some(ConvexPolygon)` with the scaled shape
    /// - `None` if the scaling results in degenerate normals (e.g., if the scale factor along
    ///   one axis is zero or nearly zero)
    ///
    /// # Example: Uniform scaling
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::ConvexPolygon;
    /// use parry2d::math::Vector;
    ///
    /// let triangle = ConvexPolygon::from_convex_polyline(vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0),
    ///     Vector::new(0.5, 1.0),
    /// ]).unwrap();
    ///
    /// // Scale uniformly by 2x
    /// let scaled = triangle.scaled(Vector::new(2.0, 2.0))
    ///     .expect("Failed to scale");
    ///
    /// // All coordinates are doubled
    /// assert_eq!(scaled.points()[1], Vector::new(2.0, 0.0));
    /// assert_eq!(scaled.points()[2], Vector::new(1.0, 2.0));
    /// # }
    /// ```
    ///
    /// # Example: Non-uniform scaling
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::ConvexPolygon;
    /// use parry2d::math::Vector;
    ///
    /// let square = ConvexPolygon::from_convex_polyline(vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0),
    ///     Vector::new(1.0, 1.0),
    ///     Vector::new(0.0, 1.0),
    /// ]).unwrap();
    ///
    /// // Scale to make it wider (2x) and taller (3x)
    /// let rectangle = square.scaled(Vector::new(2.0, 3.0))
    ///     .expect("Failed to scale");
    ///
    /// assert_eq!(rectangle.points()[2], Vector::new(2.0, 3.0));
    /// # }
    /// ```
    pub fn scaled(mut self, scale: Vector) -> Option<Self> {
        self.points.iter_mut().for_each(|pt| *pt *= scale);

        for n in &mut self.normals {
            let scaled = *n * scale;
            *n = scaled.try_normalize()?;
        }

        Some(self)
    }

    /// Returns a mitered offset (expanded or contracted) version of the polygon.
    ///
    /// This method creates a new polygon by moving each edge outward (or inward for negative values)
    /// by the specified `amount`. The vertices are adjusted to maintain sharp corners (mitered joints).
    /// This is also known as "polygon dilation" or "Minkowski sum with a circle".
    ///
    /// # Use cases
    ///
    /// - Creating "safe zones" or margins around objects
    /// - Implementing "thick" polygon collision detection
    /// - Creating rounded rectangular shapes (when combined with rounding)
    /// - Growing or shrinking shapes for morphological operations
    ///
    /// # Arguments
    ///
    /// * `amount` - The distance to move each edge. Positive values expand the polygon outward,
    ///   making it larger. Must be a non-negative finite number.
    ///
    /// # Panics
    ///
    /// Panics if `amount` is not a non-negative finite number (NaN, infinity, or negative).
    ///
    /// # Example: Expanding a triangle
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::ConvexPolygon;
    /// use parry2d::math::Vector;
    ///
    /// let triangle = ConvexPolygon::from_convex_polyline(vec![
    ///     Vector::ZERO,
    ///     Vector::new(2.0, 0.0),
    ///     Vector::new(1.0, 2.0),
    /// ]).unwrap();
    ///
    /// // Expand the triangle by 0.5 units
    /// let expanded = triangle.offsetted(0.5);
    ///
    /// // The expanded triangle has the same number of vertices
    /// assert_eq!(expanded.points().len(), 3);
    /// // But the vertices have moved outward
    /// // (exact positions depend on the miter calculation)
    /// # }
    /// ```
    ///
    /// # Example: Creating a margin around a square
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::shape::ConvexPolygon;
    /// use parry2d::math::Vector;
    ///
    /// let square = ConvexPolygon::from_convex_polyline(vec![
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0),
    ///     Vector::new(1.0, 1.0),
    ///     Vector::new(0.0, 1.0),
    /// ]).unwrap();
    ///
    /// // Create a 0.2 unit margin around the square
    /// let margin = square.offsetted(0.2);
    ///
    /// // The shape is still a square (4 vertices)
    /// assert_eq!(margin.points().len(), 4);
    /// # }
    /// ```
    pub fn offsetted(&self, amount: Real) -> Self {
        if !amount.is_finite() || amount < 0. {
            panic!(
                "Offset amount must be a non-negative finite number, got {}.",
                amount
            );
        }

        let mut points = Vec::with_capacity(self.points.len());
        let normals = self.normals.clone();

        for i2 in 0..self.points.len() {
            let i1 = if i2 == 0 {
                self.points.len() - 1
            } else {
                i2 - 1
            };
            let normal_a = normals[i1];
            let direction = normal_a + normals[i2];
            points.push(self.points[i2] + (amount / direction.dot(normal_a)) * direction);
        }

        ConvexPolygon { points, normals }
    }

    /// Get the ID of the feature with a normal that maximizes the dot product with `local_dir`.
    pub fn support_feature_id_toward(&self, local_dir: Vector) -> FeatureId {
        let eps: Real = Real::pi() / 180.0;
        let ceps = <Real as ComplexField>::cos(eps);

        // Check faces.
        for i in 0..self.normals.len() {
            let normal = &self.normals[i];

            if normal.dot(local_dir) >= ceps {
                return FeatureId::Face(i as u32);
            }
        }

        // Support vertex.
        FeatureId::Vertex(utils::point_cloud_support_point_id(local_dir, &self.points) as u32)
    }

    /// The normal of the given feature.
    pub fn feature_normal(&self, feature: FeatureId) -> Option<Vector> {
        match feature {
            FeatureId::Face(id) => Some(self.normals[id as usize]),
            FeatureId::Vertex(id2) => {
                let id1 = if id2 == 0 {
                    self.normals.len() - 1
                } else {
                    id2 as usize - 1
                };
                let sum = self.normals[id1] + self.normals[id2 as usize];
                sum.try_normalize()
            }
            _ => None,
        }
    }
}

impl SupportMap for ConvexPolygon {
    #[inline]
    fn local_support_point(&self, dir: Vector) -> Vector {
        utils::point_cloud_support_point(dir, self.points())
    }
}

impl PolygonalFeatureMap for ConvexPolygon {
    fn local_support_feature(&self, dir: Vector, out_feature: &mut PolygonalFeature) {
        let cuboid = crate::shape::Cuboid::new(self.points[2]);
        cuboid.local_support_feature(dir, out_feature);
        let mut best_face = 0;
        let mut max_dot = self.normals[0].dot(dir);

        for i in 1..self.normals.len() {
            let dot = self.normals[i].dot(dir);

            if dot > max_dot {
                max_dot = dot;
                best_face = i;
            }
        }

        let i1 = best_face;
        let i2 = (best_face + 1) % self.points.len();
        *out_feature = PolygonalFeature {
            vertices: [self.points[i1], self.points[i2]],
            vids: PackedFeatureId::vertices([i1 as u32 * 2, i2 as u32 * 2]),
            fid: PackedFeatureId::face(i1 as u32 * 2 + 1),
            num_vertices: 2,
        };
    }
}

/*
impl ConvexPolyhedron for ConvexPolygon {
    fn vertex(&self, id: FeatureId) -> Vector {
        self.points[id.unwrap_vertex() as usize]
    }

    fn face(&self, id: FeatureId, out: &mut ConvexPolygonalFeature) {
        out.clear();

        let ia = id.unwrap_face() as usize;
        let ib = (ia + 1) % self.points.len();
        out.push(self.points[ia], FeatureId::Vertex(ia as u32));
        out.push(self.points[ib], FeatureId::Vertex(ib as u32));

        out.set_normal(self.normals[ia as usize]);
        out.set_feature_id(FeatureId::Face(ia as u32));
    }



    fn support_face_toward(
        &self,
        m: &Pose,
        dir: Vector,
        out: &mut ConvexPolygonalFeature,
    ) {
        let ls_dir = m.inverse_transform_vector(dir);
        let mut best_face = 0;
        let mut max_dot = self.normals[0].dot(ls_dir);

        for i in 1..self.points.len() {
            let dot = self.normals[i].dot(ls_dir);

            if dot > max_dot {
                max_dot = dot;
                best_face = i;
            }
        }

        self.face(FeatureId::Face(best_face as u32), out);
        out.transform_by(m);
    }

    fn support_feature_toward(
        &self,
        transform: &Pose,
        dir: Vector,
        _angle: Real,
        out: &mut ConvexPolygonalFeature,
    ) {
        out.clear();
        // TODO: actually find the support feature.
        self.support_face_toward(transform, dir, out)
    }

    fn support_feature_id_toward(&self, local_dir: Vector) -> FeatureId {
        let eps: Real = (f64::consts::PI / 180.0) as Real;
        let ceps = <Real as ComplexField>::cos(eps);

        // Check faces.
        for i in 0..self.normals.len() {
            let normal = &self.normals[i];

            if normal.dot(local_dir.as_ref()) >= ceps {
                return FeatureId::Face(i as u32);
            }
        }

        // Support vertex.
        FeatureId::Vertex(
            utils::point_cloud_support_point_id(local_dir.as_ref(), &self.points) as u32,
        )
    }
}
*/

#[cfg(feature = "dim2")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dilation() {
        let polygon = ConvexPolygon::from_convex_polyline(vec![
            Vector::new(1., 0.),
            Vector::new(-1., 0.),
            Vector::new(0., -1.),
        ])
        .unwrap();

        let offsetted = polygon.offsetted(0.5);
        let expected = vec![
            Vector::new(2.207, 0.5),
            Vector::new(-2.207, 0.5),
            Vector::new(0., -1.707),
        ];

        assert_eq!(offsetted.points().len(), 3);
        assert!(offsetted
            .points()
            .iter()
            .zip(expected.iter())
            .all(|(a, b)| (a - b).length() < 0.001));
    }
}
