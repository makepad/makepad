//! Definition of the segment shape.

use crate::math::{Pose, Real, Vector};
use crate::shape::{FeatureId, SupportMap};
use core::mem;

/// A line segment shape.
///
/// A segment is the simplest 1D shape, defined by two endpoints. It represents
/// a straight line between two points with no thickness or volume.
///
/// # Structure
///
/// - **a**: The first endpoint
/// - **b**: The second endpoint
/// - **Direction**: Vectors from `a` toward `b`
///
/// # Properties
///
/// - **1-dimensional**: Has length but no width or volume
/// - **Convex**: Always convex
/// - **No volume**: Mass properties are zero
/// - **Simple**: Very fast collision detection
///
/// # Use Cases
///
/// Segments are commonly used for:
/// - **Thin objects**: Ropes, wires, laser beams
/// - **Skeletal animation**: Bone connections
/// - **Path representation**: Straight-line paths
/// - **Geometry building block**: Part of polylines and meshes
/// - **Testing**: Simple shape for debugging
///
/// # Note
///
/// For shapes with thickness, consider using [`Capsule`](super::Capsule) instead,
/// which is a segment with a radius (rounded cylinder).
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Segment;
/// use parry3d::math::Vector;
///
/// // Create a horizontal segment of length 5
/// let a = Vector::ZERO;
/// let b = Vector::new(5.0, 0.0, 0.0);
/// let segment = Segment::new(a, b);
///
/// assert_eq!(segment.length(), 5.0);
/// assert_eq!(segment.a, a);
/// assert_eq!(segment.b, b);
/// # }
/// ```
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "bytemuck", derive(bytemuck::Pod, bytemuck::Zeroable))]
#[cfg_attr(feature = "encase", derive(encase::ShaderType))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(C)]
pub struct Segment {
    /// The first endpoint of the segment.
    pub a: Vector,
    /// The second endpoint of the segment.
    pub b: Vector,
}

/// Describes where a point is located on a segment.
///
/// This enum is used by point projection queries to indicate whether the
/// projected point is at one of the endpoints or somewhere along the segment.
///
/// # Variants
///
/// - **OnVertex(id)**: Vector projects to an endpoint (0 = `a`, 1 = `b`)
/// - **OnEdge(bary)**: Vector projects to the interior with barycentric coordinates
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::SegmentPointLocation;
///
/// // Vector at first vertex
/// let loc = SegmentPointLocation::OnVertex(0);
/// assert_eq!(loc.barycentric_coordinates(), [1.0, 0.0]);
///
/// // Vector at second vertex
/// let loc = SegmentPointLocation::OnVertex(1);
/// assert_eq!(loc.barycentric_coordinates(), [0.0, 1.0]);
///
/// // Vector halfway along the segment
/// let loc = SegmentPointLocation::OnEdge([0.5, 0.5]);
/// assert_eq!(loc.barycentric_coordinates(), [0.5, 0.5]);
/// # }
/// ```
#[derive(PartialEq, Debug, Clone, Copy)]
pub enum SegmentPointLocation {
    /// The point lies on a vertex (endpoint).
    ///
    /// - `0` = Vector is at `segment.a`
    /// - `1` = Vector is at `segment.b`
    OnVertex(u32),

    /// The point lies on the segment interior.
    ///
    /// Contains barycentric coordinates `[u, v]` where:
    /// - `u + v = 1.0`
    /// - Vector = `a * u + b * v`
    /// - `0.0 < u, v < 1.0` (strictly between endpoints)
    OnEdge([Real; 2]),
}

impl SegmentPointLocation {
    /// Returns the barycentric coordinates corresponding to this location.
    ///
    /// Barycentric coordinates `[u, v]` satisfy:
    /// - `u + v = 1.0`
    /// - Vector = `a * u + b * v`
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::{Segment, SegmentPointLocation};
    /// use parry3d::math::Vector;
    ///
    /// let segment = Segment::new(
    ///     Vector::ZERO,
    ///     Vector::new(10.0, 0.0, 0.0)
    /// );
    ///
    /// // Vector at endpoint a
    /// let loc_a = SegmentPointLocation::OnVertex(0);
    /// assert_eq!(loc_a.barycentric_coordinates(), [1.0, 0.0]);
    ///
    /// // Vector at endpoint b
    /// let loc_b = SegmentPointLocation::OnVertex(1);
    /// assert_eq!(loc_b.barycentric_coordinates(), [0.0, 1.0]);
    ///
    /// // Vector at 30% from a to b
    /// let loc_mid = SegmentPointLocation::OnEdge([0.7, 0.3]);
    /// let coords = loc_mid.barycentric_coordinates();
    /// assert_eq!(coords[0], 0.7);
    /// assert_eq!(coords[1], 0.3);
    /// # }
    /// ```
    pub fn barycentric_coordinates(&self) -> [Real; 2] {
        let mut bcoords = [0.0; 2];

        match self {
            SegmentPointLocation::OnVertex(i) => bcoords[*i as usize] = 1.0,
            SegmentPointLocation::OnEdge(uv) => {
                bcoords[0] = uv[0];
                bcoords[1] = uv[1];
            }
        }

        bcoords
    }
}

impl Segment {
    /// Creates a new segment from two endpoints.
    ///
    /// # Arguments
    ///
    /// * `a` - The first endpoint
    /// * `b` - The second endpoint
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Segment;
    /// use parry3d::math::Vector;
    ///
    /// let segment = Segment::new(
    ///     Vector::ZERO,
    ///     Vector::new(5.0, 0.0, 0.0)
    /// );
    /// assert_eq!(segment.length(), 5.0);
    /// # }
    /// ```
    #[inline]
    pub fn new(a: Vector, b: Vector) -> Segment {
        Segment { a, b }
    }

    /// Creates a segment reference from an array of two points.
    ///
    /// This is a zero-cost conversion using memory transmutation.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Segment;
    /// use parry3d::math::Vector;
    ///
    /// let points = [Vector::ZERO, Vector::new(1.0, 0.0, 0.0)];
    /// let segment = Segment::from_array(&points);
    /// assert_eq!(segment.a, points[0]);
    /// assert_eq!(segment.b, points[1]);
    /// # }
    /// ```
    pub fn from_array(arr: &[Vector; 2]) -> &Segment {
        unsafe { mem::transmute(arr) }
    }

    /// Computes a scaled version of this segment.
    ///
    /// Each endpoint is scaled component-wise by the scale vector.
    ///
    /// # Arguments
    ///
    /// * `scale` - The scaling factors for each axis
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Segment;
    /// use parry3d::math::Vector;
    ///
    /// let segment = Segment::new(
    ///     Vector::new(1.0, 2.0, 3.0),
    ///     Vector::new(4.0, 5.0, 6.0)
    /// );
    ///
    /// let scaled = segment.scaled(Vector::new(2.0, 2.0, 2.0));
    /// assert_eq!(scaled.a, Vector::new(2.0, 4.0, 6.0));
    /// assert_eq!(scaled.b, Vector::new(8.0, 10.0, 12.0));
    /// # }
    /// ```
    pub fn scaled(self, scale: Vector) -> Self {
        Self::new(self.a * scale, self.b * scale)
    }

    /// Returns the direction vector of this segment scaled by its length.
    ///
    /// This is equivalent to `b - a` and points from `a` toward `b`.
    /// The magnitude equals the segment length.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Segment;
    /// use parry3d::math::Vector;
    ///
    /// let segment = Segment::new(
    ///     Vector::ZERO,
    ///     Vector::new(3.0, 4.0, 0.0)
    /// );
    ///
    /// let dir = segment.scaled_direction();
    /// assert_eq!(dir, Vector::new(3.0, 4.0, 0.0));
    /// assert_eq!(dir.length(), 5.0); // Length of the segment
    /// # }
    /// ```
    pub fn scaled_direction(&self) -> Vector {
        self.b - self.a
    }

    /// Returns the length of this segment.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Segment;
    /// use parry3d::math::Vector;
    ///
    /// // 3-4-5 right triangle
    /// let segment = Segment::new(
    ///     Vector::ZERO,
    ///     Vector::new(3.0, 4.0, 0.0)
    /// );
    /// assert_eq!(segment.length(), 5.0);
    /// # }
    /// ```
    pub fn length(&self) -> Real {
        self.scaled_direction().length()
    }

    /// Swaps the two endpoints of this segment.
    ///
    /// After swapping, `a` becomes `b` and `b` becomes `a`.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Segment;
    /// use parry3d::math::Vector;
    ///
    /// let mut segment = Segment::new(
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(5.0, 0.0, 0.0)
    /// );
    ///
    /// segment.swap();
    /// assert_eq!(segment.a, Vector::new(5.0, 0.0, 0.0));
    /// assert_eq!(segment.b, Vector::new(1.0, 0.0, 0.0));
    /// # }
    /// ```
    pub fn swap(&mut self) {
        mem::swap(&mut self.a, &mut self.b)
    }

    /// Returns the unit direction vector of this segment.
    ///
    /// Vectors from `a` toward `b` with length 1.0.
    ///
    /// # Returns
    ///
    /// * `Some(direction)` - The normalized direction if the segment has non-zero length
    /// * `None` - If both endpoints are equal (degenerate segment)
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Segment;
    /// use parry3d::math::Vector;
    ///
    /// let segment = Segment::new(
    ///     Vector::ZERO,
    ///     Vector::new(3.0, 4.0, 0.0)
    /// );
    ///
    /// if let Some(dir) = segment.direction() {
    ///     // Direction is normalized
    ///     assert!((dir.length() - 1.0).abs() < 1e-6);
    ///     // Vectors from a to b
    ///     assert_eq!(dir, Vector::new(0.6, 0.8, 0.0));
    /// }
    ///
    /// // Degenerate segment (zero length)
    /// let degenerate = Segment::new(Vector::ZERO, Vector::ZERO);
    /// assert!(degenerate.direction().is_none());
    /// # }
    /// ```
    pub fn direction(&self) -> Option<Vector> {
        self.scaled_direction().try_normalize()
    }

    /// In 2D, the not-normalized counterclockwise normal of this segment.
    #[cfg(feature = "dim2")]
    pub fn scaled_normal(&self) -> Vector {
        let dir = self.scaled_direction();
        Vector::new(dir.y, -dir.x)
    }

    /// The not-normalized counterclockwise normal of this segment, assuming it lies on the plane
    /// with the normal collinear to the given axis (0 = X, 1 = Y, 2 = Z).
    #[cfg(feature = "dim3")]
    pub fn scaled_planar_normal(&self, plane_axis: u8) -> Vector {
        let dir = self.scaled_direction();
        match plane_axis {
            0 => Vector::new(0.0, dir.z, -dir.y),
            1 => Vector::new(-dir.z, 0.0, dir.x),
            2 => Vector::new(dir.y, -dir.x, 0.0),
            _ => panic!("Invalid axis given: must be 0 (X axis), 1 (Y axis) or 2 (Z axis)"),
        }
    }

    /// In 2D, the normalized counterclockwise normal of this segment.
    #[cfg(feature = "dim2")]
    pub fn normal(&self) -> Option<Vector> {
        let (dir, length) = self.scaled_normal().normalize_and_length();
        (length > crate::math::DEFAULT_EPSILON).then_some(dir)
    }

    /// Returns `None`. Exists only for API similarity with the 2D parry.
    #[cfg(feature = "dim3")]
    pub fn normal(&self) -> Option<Vector> {
        None
    }

    /// The normalized counterclockwise normal of this segment, assuming it lies on the plane
    /// with the normal collinear to the given axis (0 = X, 1 = Y, 2 = Z).
    #[cfg(feature = "dim3")]
    pub fn planar_normal(&self, plane_axis: u8) -> Option<Vector> {
        self.scaled_planar_normal(plane_axis).try_normalize()
    }

    /// Applies the isometry `m` to the vertices of this segment and returns the resulting segment.
    pub fn transformed(&self, m: &Pose) -> Self {
        Segment::new(m * self.a, m * self.b)
    }

    /// Computes the point at the given location.
    pub fn point_at(&self, location: &SegmentPointLocation) -> Vector {
        match *location {
            SegmentPointLocation::OnVertex(0) => self.a,
            SegmentPointLocation::OnVertex(1) => self.b,
            SegmentPointLocation::OnEdge(bcoords) => self.a * bcoords[0] + self.b * bcoords[1],
            _ => panic!(),
        }
    }

    /// The normal of the given feature of this shape.
    pub fn feature_normal(&self, feature: FeatureId) -> Option<Vector> {
        if let Some(direction) = self.direction() {
            match feature {
                FeatureId::Vertex(id) => {
                    if id == 0 {
                        Some(direction)
                    } else {
                        Some(-direction)
                    }
                }
                #[cfg(feature = "dim3")]
                FeatureId::Edge(_) => {
                    let imin = direction.abs().min_position();
                    let mut normal = Vector::ZERO;
                    normal[imin] = 1.0;
                    normal -= direction * direction[imin];
                    Some(normal.normalize())
                }
                FeatureId::Face(id) => {
                    let mut dir = Vector::ZERO;
                    if id == 0 {
                        dir[0] = direction[1];
                        dir[1] = -direction[0];
                    } else {
                        dir[0] = -direction[1];
                        dir[1] = direction[0];
                    }
                    Some(dir)
                }
                _ => None,
            }
        } else {
            Some(Vector::Y)
        }
    }
}

impl SupportMap for Segment {
    #[inline]
    fn local_support_point(&self, dir: Vector) -> Vector {
        if self.a.dot(dir) > self.b.dot(dir) {
            self.a
        } else {
            self.b
        }
    }
}

impl From<[Vector; 2]> for Segment {
    fn from(arr: [Vector; 2]) -> Self {
        *Self::from_array(&arr)
    }
}

/*
impl ConvexPolyhedron for Segment {
    fn vertex(&self, id: FeatureId) -> Vector {
        if id.unwrap_vertex() == 0 {
            self.a
        } else {
            self.b
        }
    }

    #[cfg(feature = "dim3")]
    fn edge(&self, _: FeatureId) -> (Vector, Vector, FeatureId, FeatureId) {
        (self.a, self.b, FeatureId::Vertex(0), FeatureId::Vertex(1))
    }

    #[cfg(feature = "dim3")]
    fn face(&self, _: FeatureId, _: &mut ConvexPolygonalFeature) {
        panic!("A segment does not have any face in dimensions higher than 2.")
    }

    #[cfg(feature = "dim2")]
    fn face(&self, id: FeatureId, face: &mut ConvexPolygonalFeature) {
        face.clear();

        if let Some(normal) = utils::ccw_face_normal([&self.a, &self.b]) {
            face.set_feature_id(id);

            match id.unwrap_face() {
                0 => {
                    face.push(self.a, FeatureId::Vertex(0));
                    face.push(self.b, FeatureId::Vertex(1));
                    face.set_normal(normal);
                }
                1 => {
                    face.push(self.b, FeatureId::Vertex(1));
                    face.push(self.a, FeatureId::Vertex(0));
                    face.set_normal(-normal);
                }
                _ => unreachable!(),
            }
        } else {
            face.push(self.a, FeatureId::Vertex(0));
            face.set_feature_id(FeatureId::Vertex(0));
        }
    }

    #[cfg(feature = "dim2")]
    fn support_face_toward(
        &self,
        m: &Pose,
        dir: Vector,
        face: &mut ConvexPolygonalFeature,
    ) {
        let seg_dir = self.scaled_direction();

        if dir.perp(&seg_dir) >= 0.0 {
            self.face(FeatureId::Face(0), face);
        } else {
            self.face(FeatureId::Face(1), face);
        }
        face.transform_by(m)
    }

    #[cfg(feature = "dim3")]
    fn support_face_toward(
        &self,
        m: &Pose,
        _: Vector,
        face: &mut ConvexPolygonalFeature,
    ) {
        face.clear();
        face.push(self.a, FeatureId::Vertex(0));
        face.push(self.b, FeatureId::Vertex(1));
        face.push_edge_feature_id(FeatureId::Edge(0));
        face.set_feature_id(FeatureId::Edge(0));
        face.transform_by(m)
    }

    fn support_feature_toward(
        &self,
        transform: &Pose,
        dir: Vector,
        eps: Real,
        face: &mut ConvexPolygonalFeature,
    ) {
        face.clear();
        let seg = self.transformed(transform);
        let ceps = <Real as ComplexField>::sin(eps);

        if let Some(seg_dir) = seg.direction() {
            let cang = dir.dot(seg_dir);

            if cang > ceps {
                face.set_feature_id(FeatureId::Vertex(1));
                face.push(seg.b, FeatureId::Vertex(1));
            } else if cang < -ceps {
                face.set_feature_id(FeatureId::Vertex(0));
                face.push(seg.a, FeatureId::Vertex(0));
            } else {
                #[cfg(feature = "dim3")]
                {
                    face.push(seg.a, FeatureId::Vertex(0));
                    face.push(seg.b, FeatureId::Vertex(1));
                    face.push_edge_feature_id(FeatureId::Edge(0));
                    face.set_feature_id(FeatureId::Edge(0));
                }
                #[cfg(feature = "dim2")]
                {
                    if dir.perp(&seg_dir) >= 0.0 {
                        seg.face(FeatureId::Face(0), face);
                    } else {
                        seg.face(FeatureId::Face(1), face);
                    }
                }
            }
        }
    }

    fn support_feature_id_toward(&self, local_dir: Vector) -> FeatureId {
        if let Some(seg_dir) = self.direction() {
            let eps: Real = (f64::consts::PI / 180.0) as Real;
            let seps = <Real as ComplexField>::sin(eps);
            let dot = seg_dir.dot(local_dir.as_ref());

            if dot <= seps {
                #[cfg(feature = "dim2")]
                {
                    if local_dir.perp(seg_dir.as_ref()) >= 0.0 {
                        FeatureId::Face(0)
                    } else {
                        FeatureId::Face(1)
                    }
                }
                #[cfg(feature = "dim3")]
                {
                    FeatureId::Edge(0)
                }
            } else if dot >= 0.0 {
                FeatureId::Vertex(1)
            } else {
                FeatureId::Vertex(0)
            }
        } else {
            FeatureId::Vertex(0)
        }
    }
}
*/

#[cfg(test)]
mod test {
    use crate::query::{Ray, RayCast};

    pub use super::*;
    #[test]
    fn segment_intersect_zero_length_issue_31() {
        // never intersect each other
        let ray = Ray::new(Vector::ZERO, Vector::X);
        let segment = Segment {
            a: Vector::new(
                10.0,
                10.0,
                #[cfg(feature = "dim3")]
                10.0,
            ),
            b: Vector::new(
                10.0,
                10.0,
                #[cfg(feature = "dim3")]
                10.0,
            ),
        };

        let hit = segment.intersects_ray(&Pose::identity(), &ray, Real::MAX);
        assert_eq!(hit, false);
    }
    #[test]
    fn segment_very_close_points_hit() {
        let epsilon = 1.1920929e-7;
        // intersect each other
        let ray = Ray::new(
            Vector::new(
                epsilon * 0.5,
                0.3,
                #[cfg(feature = "dim3")]
                0.0,
            ),
            -Vector::Y,
        );
        let segment = Segment {
            a: Vector::ZERO,
            b: Vector::new(
                // Theoretically, epsilon would suffice but imprecisions force us to add some more offset.
                epsilon * 1.01,
                0.0,
                #[cfg(feature = "dim3")]
                0.0,
            ),
        };

        let hit = segment.intersects_ray(&Pose::identity(), &ray, Real::MAX);
        assert_eq!(hit, true);
    }
    #[test]
    fn segment_very_close_points_no_hit() {
        let epsilon = 1.1920929e-7;
        // never intersect each other
        let ray = Ray::new(
            Vector::new(
                // Theoretically, epsilon would suffice  but imprecisions force us to add some more offset.
                epsilon * 11.0,
                0.1,
                #[cfg(feature = "dim3")]
                0.0,
            ),
            -Vector::Y,
        );
        let segment = Segment {
            a: Vector::ZERO,
            b: Vector::new(
                epsilon * 0.9,
                0.0,
                #[cfg(feature = "dim3")]
                0.0,
            ),
        };

        let hit = segment.intersects_ray(&Pose::identity(), &ray, Real::MAX);
        assert_eq!(hit, false);
    }
}
