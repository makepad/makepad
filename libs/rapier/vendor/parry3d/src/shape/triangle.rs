//! Definition of the triangle shape.

use crate::math::{ComplexField, Pose, Real, Vector};
use crate::shape::SupportMap;
use crate::shape::{PolygonalFeature, Segment};
use crate::utils;

use core::mem;
use num::Zero;

#[cfg(feature = "dim3")]
use crate::shape::FeatureId;

#[cfg(feature = "dim2")]
use crate::shape::PackedFeatureId;

/// A triangle shape defined by three vertices.
///
/// A triangle is one of the most fundamental shapes in computational geometry.
/// It's the simplest 2D polygon and the building block for triangle meshes.
///
/// # Structure
///
/// - **a, b, c**: The three vertices of the triangle
/// - **Edges**: AB (from a to b), BC (from b to c), CA (from c to a)
/// - **Orientation**: Counter-clockwise (CCW) is the standard convention
///
/// # Properties
///
/// - **Convex**: Always convex
/// - **2D/3D**: Can be used in both dimensions
/// - **In 2D**: A filled triangular region with area
/// - **In 3D**: A flat surface embedded in 3D space (zero volume)
///
/// # Orientation Convention
///
/// Triangles are typically defined with counter-clockwise vertex order:
/// - Looking at the triangle from the "front", vertices go a → b → c in CCW order
/// - The normal vector (3D) points toward the observer
/// - Right-hand rule: Curl fingers from a→b→c, thumb points along normal
///
/// # Use Cases
///
/// - **Mesh building block**: Fundamental unit of triangle meshes
/// - **Simple collision shapes**: Fast collision detection
/// - **Terrain representation**: Ground planes and surfaces
/// - **Testing and debugging**: Simple shape for verification
///
/// # Example
///
/// ```rust
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Triangle;
/// use parry3d::math::Vector;
///
/// // Create a right triangle in the XY plane
/// let triangle = Triangle::new(
///     Vector::ZERO,  // a: origin
///     Vector::new(3.0, 0.0, 0.0),  // b: along +X
///     Vector::new(0.0, 4.0, 0.0)   // c: along +Y
/// );
///
/// // Area of 3-4-5 right triangle is 6.0
/// assert_eq!(triangle.area(), 6.0);
///
/// // Check if a point is inside
/// let inside = Vector::new(1.0, 1.0, 0.0);
/// assert!(triangle.contains_point(inside));
/// # }
/// ```
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "bytemuck", derive(bytemuck::Pod, bytemuck::Zeroable))]
#[cfg_attr(feature = "encase", derive(encase::ShaderType))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(PartialEq, Debug, Copy, Clone, Default)]
#[repr(C)]
pub struct Triangle {
    /// The first vertex of the triangle.
    pub a: Vector,
    /// The second vertex of the triangle.
    pub b: Vector,
    /// The third vertex of the triangle.
    pub c: Vector,
}

/// Description of the location of a point on a triangle.
#[derive(Copy, Clone, Debug)]
pub enum TrianglePointLocation {
    /// The point lies on a vertex.
    OnVertex(u32),
    /// The point lies on an edge.
    ///
    /// The 0-st edge is the segment AB.
    /// The 1-st edge is the segment BC.
    /// The 2-nd edge is the segment AC.
    // XXX: it appears the conversion of edge indexing here does not match the
    // convention of edge indexing for the `fn edge` method (from the ConvexPolyhedron impl).
    OnEdge(u32, [Real; 2]),
    /// The point lies on the triangle interior.
    ///
    /// The integer indicates on which side of the face the point is. 0 indicates the point
    /// is on the half-space toward the CW normal of the triangle. 1 indicates the point is on the other
    /// half-space. This is always set to 0 in 2D.
    OnFace(u32, [Real; 3]),
    /// The point lies on the triangle interior (for "solid" point queries).
    OnSolid,
}

impl TrianglePointLocation {
    /// The barycentric coordinates corresponding to this point location.
    ///
    /// Returns `None` if the location is `TrianglePointLocation::OnSolid`.
    pub fn barycentric_coordinates(&self) -> Option<[Real; 3]> {
        let mut bcoords = [0.0; 3];

        match self {
            TrianglePointLocation::OnVertex(i) => bcoords[*i as usize] = 1.0,
            TrianglePointLocation::OnEdge(i, uv) => {
                let idx = match i {
                    0 => (0, 1),
                    1 => (1, 2),
                    2 => (0, 2),
                    _ => unreachable!(),
                };

                bcoords[idx.0] = uv[0];
                bcoords[idx.1] = uv[1];
            }
            TrianglePointLocation::OnFace(_, uvw) => {
                bcoords[0] = uvw[0];
                bcoords[1] = uvw[1];
                bcoords[2] = uvw[2];
            }
            TrianglePointLocation::OnSolid => {
                return None;
            }
        }

        Some(bcoords)
    }

    /// Returns `true` if the point is located on the relative interior of the triangle.
    pub fn is_on_face(&self) -> bool {
        matches!(*self, TrianglePointLocation::OnFace(..))
    }
}

/// Orientation of a triangle.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TriangleOrientation {
    /// Orientation with a clockwise orientation, i.e., with a positive signed area.
    Clockwise,
    /// Orientation with a clockwise orientation, i.e., with a negative signed area.
    CounterClockwise,
    /// Degenerate triangle.
    Degenerate,
}

impl From<[Vector; 3]> for Triangle {
    fn from(arr: [Vector; 3]) -> Self {
        *Self::from_array(&arr)
    }
}

impl Triangle {
    /// Creates a triangle from three vertices.
    ///
    /// # Arguments
    ///
    /// * `a` - The first vertex
    /// * `b` - The second vertex
    /// * `c` - The third vertex
    ///
    /// # Convention
    ///
    /// For proper normal calculation and consistent collision detection, vertices
    /// should be ordered counter-clockwise when viewed from the "front" side.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Triangle;
    /// use parry3d::math::Vector;
    ///
    /// // Create a triangle in the XY plane
    /// let tri = Triangle::new(
    ///     Vector::ZERO,
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0)
    /// );
    ///
    /// assert_eq!(tri.area(), 0.5);
    /// # }
    /// ```
    #[inline]
    pub fn new(a: Vector, b: Vector, c: Vector) -> Triangle {
        Triangle { a, b, c }
    }

    /// Creates the reference to a triangle from the reference to an array of three points.
    pub fn from_array(arr: &[Vector; 3]) -> &Triangle {
        unsafe { mem::transmute(arr) }
    }

    /// Reference to an array containing the three vertices of this triangle.
    #[inline]
    pub fn vertices(&self) -> &[Vector; 3] {
        unsafe { mem::transmute(self) }
    }

    /// The normal of this triangle assuming it is oriented ccw.
    ///
    /// The normal points such that it is collinear to `AB × AC` (where `×` denotes the cross
    /// product).
    #[inline]
    #[cfg(feature = "dim3")]
    pub fn normal(&self) -> Option<Vector> {
        self.scaled_normal().try_normalize()
    }

    /// The three edges of this triangle: [AB, BC, CA].
    #[inline]
    pub fn edges(&self) -> [Segment; 3] {
        [
            Segment::new(self.a, self.b),
            Segment::new(self.b, self.c),
            Segment::new(self.c, self.a),
        ]
    }

    /// Computes a scaled version of this triangle.
    pub fn scaled(self, scale: Vector) -> Self {
        Self::new(self.a * scale, self.b * scale, self.c * scale)
    }

    /// Returns a new triangle with vertices transformed by `m`.
    #[inline]
    pub fn transformed(&self, m: &Pose) -> Self {
        Triangle::new(m * self.a, m * self.b, m * self.c)
    }

    /// The three edges scaled directions of this triangle: [B - A, C - B, A - C].
    #[inline]
    pub fn edges_scaled_directions(&self) -> [Vector; 3] {
        [self.b - self.a, self.c - self.b, self.a - self.c]
    }

    /// Return the edge segment of this cuboid with a normal cone containing
    /// a direction that that maximizes the dot product with `local_dir`.
    pub fn local_support_edge_segment(&self, dir: Vector) -> Segment {
        let dots = [dir.dot(self.a), dir.dot(self.b), dir.dot(self.c)];

        let imin = if dots[0] <= dots[1] && dots[0] <= dots[2] {
            0
        } else if dots[1] <= dots[2] {
            1
        } else {
            2
        };

        match imin {
            0 => Segment::new(self.b, self.c),
            1 => Segment::new(self.c, self.a),
            _ => Segment::new(self.a, self.b),
        }
    }

    /// Return the face of this triangle with a normal that maximizes
    /// the dot product with `dir`.
    #[cfg(feature = "dim3")]
    pub fn support_face(&self, _dir: Vector) -> PolygonalFeature {
        PolygonalFeature::from(*self)
    }

    /// Return the face of this triangle with a normal that maximizes
    /// the dot product with `dir`.
    #[cfg(feature = "dim2")]
    pub fn support_face(&self, dir: Vector) -> PolygonalFeature {
        let mut best = 0;
        let mut best_dot = -Real::MAX;

        for (i, tangent) in self.edges_scaled_directions().iter().enumerate() {
            let normal = Vector::new(tangent.y, -tangent.x);
            if let Some(normal) = normal.try_normalize() {
                let dot = normal.dot(dir);
                if normal.dot(dir) > best_dot {
                    best = i;
                    best_dot = dot;
                }
            }
        }

        let pts = self.vertices();
        let i1 = best;
        let i2 = (best + 1) % 3;

        PolygonalFeature {
            vertices: [pts[i1], pts[i2]],
            vids: PackedFeatureId::vertices([i1 as u32, i2 as u32]),
            fid: PackedFeatureId::face(i1 as u32),
            num_vertices: 2,
        }
    }

    /// A vector normal of this triangle.
    ///
    /// The vector points such that it is collinear to `AB × AC` (where `×` denotes the cross
    /// product).
    ///
    /// Note that on thin triangles the calculated normals can suffer from numerical issues.
    /// For a more robust (but more computationally expensive) normal calculation, see
    /// [`Triangle::robust_scaled_normal`].
    #[inline]
    #[cfg(feature = "dim3")]
    pub fn scaled_normal(&self) -> Vector {
        let ab = self.b - self.a;
        let ac = self.c - self.a;
        ab.cross(ac)
    }

    /// Find a triangle normal more robustly than with [`Triangle::scaled_normal`].
    ///
    /// Thin triangles can cause numerical issues when computing its normal. This method accounts
    /// for these numerical issues more robustly than [`Triangle::scaled_normal`], but is more
    /// computationally expensive.
    #[inline]
    #[cfg(feature = "dim3")]
    pub fn robust_scaled_normal(&self) -> Vector {
        let pts = self.vertices();
        let best_vertex = self.angle_closest_to_90();
        let d1 = pts[(best_vertex + 2) % 3] - pts[(best_vertex + 1) % 3];
        let d2 = pts[best_vertex] - pts[(best_vertex + 1) % 3];

        d1.cross(d2)
    }

    /// Similar to [`Triangle::robust_scaled_normal`], but returns the unit length normal.
    #[inline]
    #[cfg(feature = "dim3")]
    pub fn robust_normal(&self) -> Vector {
        self.robust_scaled_normal().normalize()
    }

    /// Computes the extents of this triangle on the given direction.
    ///
    /// This computes the min and max values of the dot products between each
    /// vertex of this triangle and `dir`.
    #[inline]
    pub fn extents_on_dir(&self, dir: Vector) -> (Real, Real) {
        let a = self.a.dot(dir);
        let b = self.b.dot(dir);
        let c = self.c.dot(dir);

        if a > b {
            if b > c {
                (c, a)
            } else if a > c {
                (b, a)
            } else {
                (b, c)
            }
        } else {
            // b >= a
            if a > c {
                (c, b)
            } else if b > c {
                (a, b)
            } else {
                (a, c)
            }
        }
    }
    //
    // #[cfg(feature = "dim3")]
    // fn support_feature_id_toward(&self, local_dir: Vector, eps: Real) -> FeatureId {
    //     if let Some(normal) = self.normal() {
    //         let (seps, ceps) = ComplexField::sin_cos(eps);
    //
    //         let normal_dot = local_dir.dot(*normal);
    //         if normal_dot >= ceps {
    //             FeatureId::Face(0)
    //         } else if normal_dot <= -ceps {
    //             FeatureId::Face(1)
    //         } else {
    //             let edges = self.edges();
    //             let mut dots = [0.0; 3];
    //
    //             let dir1 = edges[0].direction();
    //             if let Some(dir1) = dir1 {
    //                 dots[0] = dir1.dot(local_dir);
    //
    //                 if dots[0].abs() < seps {
    //                     return FeatureId::Edge(0);
    //                 }
    //             }
    //
    //             let dir2 = edges[1].direction();
    //             if let Some(dir2) = dir2 {
    //                 dots[1] = dir2.dot(local_dir);
    //
    //                 if dots[1].abs() < seps {
    //                     return FeatureId::Edge(1);
    //                 }
    //             }
    //
    //             let dir3 = edges[2].direction();
    //             if let Some(dir3) = dir3 {
    //                 dots[2] = dir3.dot(local_dir);
    //
    //                 if dots[2].abs() < seps {
    //                     return FeatureId::Edge(2);
    //                 }
    //             }
    //
    //             if dots[0] > 0.0 && dots[1] < 0.0 {
    //                 FeatureId::Vertex(1)
    //             } else if dots[1] > 0.0 && dots[2] < 0.0 {
    //                 FeatureId::Vertex(2)
    //             } else {
    //                 FeatureId::Vertex(0)
    //             }
    //         }
    //     } else {
    //         FeatureId::Vertex(0)
    //     }
    // }

    /// The area of this triangle.
    #[inline]
    pub fn area(&self) -> Real {
        // Kahan's formula.
        let a = self.b.distance(self.a);
        let b = self.c.distance(self.b);
        let c = self.a.distance(self.c);

        let (c, b, a) = utils::sort3(&a, &b, &c);
        let a = *a;
        let b = *b;
        let c = *c;

        let sqr = (a + (b + c)) * (c - (a - b)) * (c + (a - b)) * (a + (b - c));

        // We take the max(0.0) because it can be slightly negative
        // because of numerical errors due to almost-degenerate triangles.
        <Real as ComplexField>::sqrt(sqr.max(0.0)) * 0.25
    }

    /// Computes the unit angular inertia of this triangle.
    #[cfg(feature = "dim2")]
    pub fn unit_angular_inertia(&self) -> Real {
        let factor = 1.0 / 6.0;

        // Algorithm adapted from Box2D
        let e1 = self.b - self.a;
        let e2 = self.c - self.a;

        let intx2 = e1.x * e1.x + e2.x * e1.x + e2.x * e2.x;
        let inty2 = e1.y * e1.y + e2.y * e1.y + e2.y * e2.y;
        factor * (intx2 + inty2)
    }

    /// The geometric center of this triangle.
    #[inline]
    pub fn center(&self) -> Vector {
        utils::center(&[self.a, self.b, self.c])
    }

    /// The perimeter of this triangle.
    #[inline]
    pub fn perimeter(&self) -> Real {
        self.b.distance(self.a) + self.c.distance(self.b) + self.a.distance(self.c)
    }

    /// The circumcircle of this triangle.
    pub fn circumcircle(&self) -> (Vector, Real) {
        let a = self.a - self.c;
        let b = self.b - self.c;

        let na = a.length_squared();
        let nb = b.length_squared();

        let dab = a.dot(b);

        let denom = 2.0 * (na * nb - dab * dab);

        if denom.is_zero() {
            // The triangle is degenerate (the three points are collinear).
            // So we find the longest segment and take its center.
            let c = self.a - self.b;
            let nc = c.length_squared();

            if nc >= na && nc >= nb {
                // Longest segment: [&self.a, &self.b]
                (
                    self.a.midpoint(self.b),
                    <Real as ComplexField>::sqrt(nc) / 2.0,
                )
            } else if na >= nb && na >= nc {
                // Longest segment: [&self.a, pc]
                (
                    self.a.midpoint(self.c),
                    <Real as ComplexField>::sqrt(na) / 2.0,
                )
            } else {
                // Longest segment: [&self.b, &self.c]
                (
                    self.b.midpoint(self.c),
                    <Real as ComplexField>::sqrt(nb) / 2.0,
                )
            }
        } else {
            let k = b * na - a * nb;

            let center = self.c + (a * k.dot(b) - b * k.dot(a)) / denom;
            let radius = center.distance(self.a);

            (center, radius)
        }
    }

    /// Tests if this triangle is affinely dependent, i.e., its points are almost aligned.
    #[cfg(feature = "dim3")]
    pub fn is_affinely_dependent(&self) -> bool {
        const EPS: Real = crate::math::DEFAULT_EPSILON * 100.0;

        let p1p2 = self.b - self.a;
        let p1p3 = self.c - self.a;
        relative_eq!(p1p2.cross(p1p3).length_squared(), 0.0, epsilon = EPS * EPS)

        // relative_eq!(
        //     self.area(),
        //     0.0,
        //     epsilon = EPS * self.perimeter()
        // )
    }

    /// Is this triangle degenerate or almost degenerate?
    #[cfg(feature = "dim3")]
    pub fn is_affinely_dependent_eps(&self, eps: Real) -> bool {
        let p1p2 = self.b - self.a;
        let p1p3 = self.c - self.a;
        relative_eq!(
            p1p2.cross(p1p3).length(),
            0.0,
            epsilon = eps * p1p2.length().max(p1p3.length())
        )

        // relative_eq!(
        //     self.area(),
        //     0.0,
        //     epsilon = EPS * self.perimeter()
        // )
    }

    /// Tests if a point is inside of this triangle.
    #[cfg(feature = "dim2")]
    pub fn contains_point(&self, p: Vector) -> bool {
        let ab = self.b - self.a;
        let bc = self.c - self.b;
        let ca = self.a - self.c;
        let sgn1 = ab.perp_dot(p - self.a);
        let sgn2 = bc.perp_dot(p - self.b);
        let sgn3 = ca.perp_dot(p - self.c);
        sgn1.signum() * sgn2.signum() >= 0.0
            && sgn1.signum() * sgn3.signum() >= 0.0
            && sgn2.signum() * sgn3.signum() >= 0.0
    }

    /// Tests if a point is inside of this triangle.
    #[cfg(feature = "dim3")]
    pub fn contains_point(&self, p: Vector) -> bool {
        const EPS: Real = crate::math::DEFAULT_EPSILON;

        let vb = self.b - self.a;
        let vc = self.c - self.a;
        let vp = p - self.a;

        let n = vc.cross(vb);
        let n_norm = n.length_squared();
        if n_norm < EPS || vp.dot(n).abs() > EPS * n_norm {
            // the triangle is degenerate or the
            // point does not lie on the same plane as the triangle.
            return false;
        }

        // We are seeking B, C such that vp = vb * B + vc * C .
        // If B and C are both in [0, 1] and B + C <= 1 then p is in the triangle.
        //
        // We can project this equation along a vector nb coplanar to the triangle
        // and perpendicular to vb:
        // vp.dot(nb) = vb.dot(nb) * B + vc.dot(nb) * C
        //     => C = vp.dot(nb) / vc.dot(nb)
        // and similarly for B.
        //
        // In order to avoid divisions and sqrts we scale both B and C - so
        // b = vb.dot(nc) * B and c = vc.dot(nb) * C - this results in harder-to-follow math but
        // hopefully fast code.

        let nb = vb.cross(n);
        let nc = vc.cross(n);

        let signed_blim = vb.dot(nc);
        let b = vp.dot(nc) * signed_blim.signum();
        let blim = signed_blim.abs();

        let signed_clim = vc.dot(nb);
        let c = vp.dot(nb) * signed_clim.signum();
        let clim = signed_clim.abs();

        c >= 0.0 && c <= clim && b >= 0.0 && b <= blim && c * blim + b * clim <= blim * clim
    }

    /// The normal of the given feature of this shape.
    #[cfg(feature = "dim3")]
    pub fn feature_normal(&self, _: FeatureId) -> Option<Vector> {
        self.normal()
    }

    /// The orientation of the triangle, based on its signed area.
    ///
    /// Returns `TriangleOrientation::Degenerate` if the triangle’s area is
    /// smaller than `epsilon`.
    #[cfg(feature = "dim2")]
    pub fn orientation(&self, epsilon: Real) -> TriangleOrientation {
        let area2 = (self.b - self.a).perp_dot(self.c - self.a);
        // println!("area2: {}", area2);
        if area2 > epsilon {
            TriangleOrientation::CounterClockwise
        } else if area2 < -epsilon {
            TriangleOrientation::Clockwise
        } else {
            TriangleOrientation::Degenerate
        }
    }

    /// The orientation of the 2D triangle, based on its signed area.
    ///
    /// Returns `TriangleOrientation::Degenerate` if the triangle's area is
    /// smaller than `epsilon`.
    pub fn orientation2d(
        a: crate::math::Vector2,
        b: crate::math::Vector2,
        c: crate::math::Vector2,
        epsilon: Real,
    ) -> TriangleOrientation {
        let area2 = (b - a).perp_dot(c - a);
        // println!("area2: {}", area2);
        if area2 > epsilon {
            TriangleOrientation::CounterClockwise
        } else if area2 < -epsilon {
            TriangleOrientation::Clockwise
        } else {
            TriangleOrientation::Degenerate
        }
    }

    /// Find the index of a vertex in this triangle, such that the two
    /// edges incident in that vertex form the angle closest to 90
    /// degrees in the triangle.
    pub fn angle_closest_to_90(&self) -> usize {
        let points = self.vertices();
        let mut best_cos = 2.0;
        let mut selected_i = 0;

        for i in 0..3 {
            let d1 = (points[i] - points[(i + 1) % 3]).normalize();
            let d2 = (points[(i + 2) % 3] - points[(i + 1) % 3]).normalize();

            let cos_abs = d1.dot(d2).abs();

            if cos_abs < best_cos {
                best_cos = cos_abs;
                selected_i = i;
            }
        }

        selected_i
    }

    /// Reverse the orientation of this triangle by swapping b and c.
    pub fn reverse(&mut self) {
        mem::swap(&mut self.b, &mut self.c);
    }
}

impl SupportMap for Triangle {
    #[inline]
    fn local_support_point(&self, dir: Vector) -> Vector {
        let d1 = self.a.dot(dir);
        let d2 = self.b.dot(dir);
        let d3 = self.c.dot(dir);

        if d1 > d2 {
            if d1 > d3 {
                self.a
            } else {
                self.c
            }
        } else if d2 > d3 {
            self.b
        } else {
            self.c
        }
    }
}

/*
#[cfg(feature = "dim3")]
impl ConvexPolyhedron for Triangle {
    fn vertex(&self, id: FeatureId) -> Vector {
        match id.unwrap_vertex() {
            0 => self.a,
            1 => self.b,
            2 => self.c,
            _ => panic!("Triangle vertex index out of bounds."),
        }
    }
    fn edge(&self, id: FeatureId) -> (Vector, Vector, FeatureId, FeatureId) {
        match id.unwrap_edge() {
            0 => (self.a, self.b, FeatureId::Vertex(0), FeatureId::Vertex(1)),
            1 => (self.b, self.c, FeatureId::Vertex(1), FeatureId::Vertex(2)),
            2 => (self.c, self.a, FeatureId::Vertex(2), FeatureId::Vertex(0)),
            _ => panic!("Triangle edge index out of bounds."),
        }
    }

    fn face(&self, id: FeatureId, face: &mut ConvexPolygonalFeature) {
        face.clear();

        if let Some(normal) = self.normal() {
            face.set_feature_id(id);

            match id.unwrap_face() {
                0 => {
                    face.push(self.a, FeatureId::Vertex(0));
                    face.push(self.b, FeatureId::Vertex(1));
                    face.push(self.c, FeatureId::Vertex(2));
                    face.push_edge_feature_id(FeatureId::Edge(0));
                    face.push_edge_feature_id(FeatureId::Edge(1));
                    face.push_edge_feature_id(FeatureId::Edge(2));
                    face.set_normal(normal);
                }
                1 => {
                    face.push(self.a, FeatureId::Vertex(0));
                    face.push(self.c, FeatureId::Vertex(2));
                    face.push(self.b, FeatureId::Vertex(1));
                    face.push_edge_feature_id(FeatureId::Edge(2));
                    face.push_edge_feature_id(FeatureId::Edge(1));
                    face.push_edge_feature_id(FeatureId::Edge(0));
                    face.set_normal(-normal);
                }
                _ => unreachable!(),
            }

            face.recompute_edge_normals();
        } else {
            face.push(self.a, FeatureId::Vertex(0));
            face.set_feature_id(FeatureId::Vertex(0));
        }
    }

    fn support_face_toward(
        &self,
        m: &Pose,
        dir: Vector,
        face: &mut ConvexPolygonalFeature,
    ) {
        let normal = self.scaled_normal();

        if normal.dot(*dir) >= 0.0 {
            ConvexPolyhedron::face(self, FeatureId::Face(0), face);
        } else {
            ConvexPolyhedron::face(self, FeatureId::Face(1), face);
        }
        face.transform_by(m)
    }

    fn support_feature_toward(
        &self,
        transform: &Pose,
        dir: Vector,
        eps: Real,
        out: &mut ConvexPolygonalFeature,
    ) {
        out.clear();
        let tri = self.transformed(transform);
        let feature = tri.support_feature_id_toward(dir, eps);

        match feature {
            FeatureId::Vertex(_) => {
                let v = tri.vertex(feature);
                out.push(v, feature);
                out.set_feature_id(feature);
            }
            FeatureId::Edge(_) => {
                let (a, b, fa, fb) = tri.edge(feature);
                out.push(a, fa);
                out.push(b, fb);
                out.push_edge_feature_id(feature);
                out.set_feature_id(feature);
            }
            FeatureId::Face(_) => tri.face(feature, out),
            _ => unreachable!(),
        }
    }

    fn support_feature_id_toward(&self, local_dir: Vector) -> FeatureId {
        self.support_feature_id_toward(local_dir, (f64::consts::PI / 180.0) as Real)
    }
}
*/

#[cfg(feature = "dim2")]
#[cfg(test)]
mod test {
    use crate::math::Vector;
    use crate::shape::Triangle;

    #[test]
    fn test_triangle_area() {
        let pa = Vector::new(5.0, 0.0);
        let pb = Vector::ZERO;
        let pc = Vector::new(0.0, 4.0);

        assert!(relative_eq!(Triangle::new(pa, pb, pc).area(), 10.0));
    }

    #[test]
    fn test_triangle_contains_point() {
        let tri = Triangle::new(Vector::new(5.0, 0.0), Vector::ZERO, Vector::new(0.0, 4.0));

        assert!(tri.contains_point(Vector::new(1.0, 1.0)));
        assert!(!tri.contains_point(Vector::new(-1.0, 1.0)));
    }

    #[test]
    fn test_obtuse_triangle_contains_point() {
        let tri = Triangle::new(
            Vector::new(-10.0, 10.0),
            Vector::ZERO,
            Vector::new(20.0, 0.0),
        );

        assert!(tri.contains_point(Vector::new(-3.0, 5.0)));
        assert!(tri.contains_point(Vector::new(5.0, 1.0)));
        assert!(!tri.contains_point(Vector::new(0.0, -1.0)));
    }
}

#[cfg(feature = "dim3")]
#[cfg(test)]
mod test {
    use crate::math::{Real, Vector};
    use crate::shape::Triangle;

    #[test]
    fn test_triangle_area() {
        let pa = Vector::new(0.0, 5.0, 0.0);
        let pb = Vector::ZERO;
        let pc = Vector::new(0.0, 0.0, 4.0);

        assert!(relative_eq!(Triangle::new(pa, pb, pc).area(), 10.0));
    }

    #[test]
    fn test_triangle_contains_point() {
        let tri = Triangle::new(
            Vector::new(0.0, 5.0, 0.0),
            Vector::ZERO,
            Vector::new(0.0, 0.0, 4.0),
        );

        assert!(tri.contains_point(Vector::new(0.0, 1.0, 1.0)));
        assert!(!tri.contains_point(Vector::new(0.0, -1.0, 1.0)));
    }

    #[test]
    fn test_obtuse_triangle_contains_point() {
        let tri = Triangle::new(
            Vector::new(-10.0, 10.0, 0.0),
            Vector::ZERO,
            Vector::new(20.0, 0.0, 0.0),
        );

        assert!(tri.contains_point(Vector::new(-3.0, 5.0, 0.0)));
        assert!(tri.contains_point(Vector::new(5.0, 1.0, 0.0)));
        assert!(!tri.contains_point(Vector::new(0.0, -1.0, 0.0)));
    }

    #[test]
    fn test_3dtriangle_contains_point() {
        let o = Vector::ZERO;
        let pa = Vector::new(1.2, 1.4, 5.6);
        let pb = Vector::new(1.5, 6.7, 1.9);
        let pc = Vector::new(5.0, 2.1, 1.3);

        let tri = Triangle::new(pa, pb, pc);

        let va = pa - o;
        let vb = pb - o;
        let vc = pc - o;

        let n = (pa - pb).cross(pb - pc);

        // This is a simple algorithm for generating points that are inside the
        // triangle: o + (va * alpha + vb * beta + vc * gamma) is always inside the
        // triangle if:
        // * each of alpha, beta, gamma is in (0, 1)
        // * alpha + beta + gamma = 1
        let contained_p = o + (va * 0.2 + vb * 0.3 + vc * 0.5);
        let not_contained_coplanar_p = o + (va * -0.5 + vb * 0.8 + vc * 0.7);
        let not_coplanar_p = o + (va * 0.2 + vb * 0.3 + vc * 0.5) + n * 0.1;
        let not_coplanar_p2 = o + (va * -0.5 + vb * 0.8 + vc * 0.7) + n * 0.1;
        assert!(tri.contains_point(contained_p));
        assert!(!tri.contains_point(not_contained_coplanar_p));
        assert!(!tri.contains_point(not_coplanar_p));
        assert!(!tri.contains_point(not_coplanar_p2));

        // Test that points that are clearly within the triangle as seen as such, by testing
        // a number of points along a line intersecting the triangle.
        for i in -50i16..150 {
            let a = 0.15;
            let b = 0.01 * Real::from(i); // b ranges from -0.5 to 1.5
            let c = 1.0 - a - b;
            let p = o + (va * a + vb * b + vc * c);

            match i {
                ii if ii < 0 || ii > 85 => assert!(
                    !tri.contains_point(p),
                    "Should not contain: i = {}, b = {}",
                    i,
                    b
                ),
                ii if ii > 0 && ii < 85 => assert!(
                    tri.contains_point(p),
                    "Should contain: i = {}, b = {}",
                    i,
                    b
                ),
                _ => (), // Vectors at the edge may be seen as inside or outside
            }
        }
    }
}
