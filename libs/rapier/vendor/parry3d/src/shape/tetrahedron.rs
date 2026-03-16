//! Definition of the tetrahedron shape.

use crate::math::{MatExt, Matrix, Real, Vector};
use crate::shape::{Segment, Triangle};
use crate::utils;
use core::mem;

#[cfg(feature = "dim3")]
use crate::math::CrossMatrix; // Mat3 for glam

#[cfg(all(feature = "dim2", not(feature = "std")))]
use crate::math::ComplexField; // for .abs()

/// A tetrahedron with 4 vertices.
///
/// # What is a Tetrahedron?
///
/// A tetrahedron is the simplest 3D polyhedron, consisting of exactly **4 vertices**,
/// **6 edges**, and **4 triangular faces**. It's the 3D equivalent of a triangle in 2D.
/// Think of it as a pyramid with a triangular base.
///
/// # Structure
///
/// A tetrahedron has:
/// - **4 vertices** labeled `a`, `b`, `c`, and `d`
/// - **4 triangular faces**: ABC, ABD, ACD, and BCD
/// - **6 edges**: AB, AC, AD, BC, BD, and CD
///
/// # Common Use Cases
///
/// Tetrahedra are fundamental building blocks in many applications:
/// - **Mesh generation**: Tetrahedral meshes are used for finite element analysis (FEA)
/// - **Collision detection**: Simple convex shape for physics simulations
/// - **Volume computation**: Computing volumes of complex 3D shapes
/// - **Spatial partitioning**: Subdividing 3D space for games and simulations
/// - **Computer graphics**: Rendering and ray tracing acceleration structures
///
/// # Examples
///
/// ## Creating a Simple Tetrahedron
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Tetrahedron;
/// use parry3d::math::Vector;
///
/// // Create a tetrahedron with vertices at unit positions
/// let tetra = Tetrahedron::new(
///     Vector::new(0.0, 0.0, 0.0),  // vertex a
///     Vector::new(1.0, 0.0, 0.0),  // vertex b
///     Vector::new(0.0, 1.0, 0.0),  // vertex c
///     Vector::new(0.0, 0.0, 1.0),  // vertex d
/// );
///
/// println!("First vertex: {:?}", tetra.a);
/// # }
/// ```
///
/// ## Computing Volume
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Tetrahedron;
/// use parry3d::math::Vector;
///
/// // Create a regular tetrahedron
/// let tetra = Tetrahedron::new(
///     Vector::new(0.0, 0.0, 0.0),
///     Vector::new(1.0, 0.0, 0.0),
///     Vector::new(0.5, 0.866, 0.0),
///     Vector::new(0.5, 0.433, 0.816),
/// );
///
/// let volume = tetra.volume();
/// assert!(volume > 0.0);
/// println!("Volume: {}", volume);
/// # }
/// ```
///
/// ## Accessing Faces and Edges
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::Tetrahedron;
/// use parry3d::math::Vector;
///
/// let tetra = Tetrahedron::new(
///     Vector::new(0.0, 0.0, 0.0),
///     Vector::new(1.0, 0.0, 0.0),
///     Vector::new(0.0, 1.0, 0.0),
///     Vector::new(0.0, 0.0, 1.0),
/// );
///
/// // Get the first face (triangle ABC)
/// let face = tetra.face(0);
/// println!("First face: {:?}", face);
///
/// // Get the first edge (segment AB)
/// let edge = tetra.edge(0);
/// println!("First edge: {:?}", edge);
/// # }
/// ```
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "bytemuck", derive(bytemuck::Pod, bytemuck::Zeroable))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct Tetrahedron {
    /// The tetrahedron's first vertex.
    pub a: Vector,
    /// The tetrahedron's second vertex.
    pub b: Vector,
    /// The tetrahedron's third vertex.
    pub c: Vector,
    /// The tetrahedron's fourth vertex.
    pub d: Vector,
}

/// Logical description of the location of a point on a tetrahedron.
///
/// This enum describes where a point is located relative to a tetrahedron's features
/// (vertices, edges, faces, or interior). It's commonly used in collision detection
/// and geometric queries to understand spatial relationships.
///
/// # Variants
///
/// - `OnVertex`: Vector is at one of the four vertices (0=a, 1=b, 2=c, 3=d)
/// - `OnEdge`: Vector lies on one of the six edges with barycentric coordinates
/// - `OnFace`: Vector lies on one of the four triangular faces with barycentric coordinates
/// - `OnSolid`: Vector is inside the tetrahedron volume
///
/// # Examples
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::shape::{Tetrahedron, TetrahedronPointLocation};
/// use parry3d::math::Vector;
///
/// let tetra = Tetrahedron::new(
///     Vector::new(0.0, 0.0, 0.0),
///     Vector::new(1.0, 0.0, 0.0),
///     Vector::new(0.0, 1.0, 0.0),
///     Vector::new(0.0, 0.0, 1.0),
/// );
///
/// // Check location of vertex
/// let location = TetrahedronPointLocation::OnVertex(0);
/// if let Some(bcoords) = location.barycentric_coordinates() {
///     println!("Barycentric coordinates: {:?}", bcoords);
///     // For vertex 0, this will be [1.0, 0.0, 0.0, 0.0]
/// }
/// # }
/// ```
#[derive(Copy, Clone, Debug)]
pub enum TetrahedronPointLocation {
    /// The point lies on a vertex.
    ///
    /// The vertex index maps to: 0=a, 1=b, 2=c, 3=d
    OnVertex(u32),
    /// The point lies on an edge.
    ///
    /// Contains the edge index and barycentric coordinates `[u, v]` where:
    /// - `u` is the weight of the first vertex
    /// - `v` is the weight of the second vertex
    /// - `u + v = 1.0`
    ///
    /// Edge indices:
    /// - 0: segment AB
    /// - 1: segment AC
    /// - 2: segment AD
    /// - 3: segment BC
    /// - 4: segment BD
    /// - 5: segment CD
    OnEdge(u32, [Real; 2]),
    /// The point lies on a triangular face interior.
    ///
    /// Contains the face index and barycentric coordinates `[u, v, w]` where:
    /// - `u`, `v`, `w` are the weights of the three vertices
    /// - `u + v + w = 1.0`
    ///
    /// Face indices:
    /// - 0: triangle ABC
    /// - 1: triangle ABD
    /// - 2: triangle ACD
    /// - 3: triangle BCD
    OnFace(u32, [Real; 3]),
    /// The point lies inside of the tetrahedron.
    OnSolid,
}

impl TetrahedronPointLocation {
    /// The barycentric coordinates corresponding to this point location.
    ///
    /// Barycentric coordinates represent a point as a weighted combination of the
    /// tetrahedron's four vertices. The returned array `[wa, wb, wc, wd]` contains
    /// weights such that: `point = wa*a + wb*b + wc*c + wd*d` where `wa + wb + wc + wd = 1.0`.
    ///
    /// # Returns
    ///
    /// - `Some([wa, wb, wc, wd])`: The barycentric coordinates for points on features
    /// - `None`: If the location is `OnSolid` (point is in the interior)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TetrahedronPointLocation;
    ///
    /// // A point on vertex 0 (vertex a)
    /// let location = TetrahedronPointLocation::OnVertex(0);
    /// let bcoords = location.barycentric_coordinates().unwrap();
    /// assert_eq!(bcoords, [1.0, 0.0, 0.0, 0.0]);
    ///
    /// // A point on edge 0 (segment AB) at midpoint
    /// let location = TetrahedronPointLocation::OnEdge(0, [0.5, 0.5]);
    /// let bcoords = location.barycentric_coordinates().unwrap();
    /// assert_eq!(bcoords, [0.5, 0.5, 0.0, 0.0]);
    ///
    /// // A point inside the tetrahedron
    /// let location = TetrahedronPointLocation::OnSolid;
    /// assert!(location.barycentric_coordinates().is_none());
    /// # }
    /// ```
    pub fn barycentric_coordinates(&self) -> Option<[Real; 4]> {
        let mut bcoords = [0.0; 4];

        match self {
            TetrahedronPointLocation::OnVertex(i) => bcoords[*i as usize] = 1.0,
            TetrahedronPointLocation::OnEdge(i, uv) => {
                let idx = Tetrahedron::edge_ids(*i);
                bcoords[idx.0 as usize] = uv[0];
                bcoords[idx.1 as usize] = uv[1];
            }
            TetrahedronPointLocation::OnFace(i, uvw) => {
                let idx = Tetrahedron::face_ids(*i);
                bcoords[idx.0 as usize] = uvw[0];
                bcoords[idx.1 as usize] = uvw[1];
                bcoords[idx.2 as usize] = uvw[2];
            }
            TetrahedronPointLocation::OnSolid => {
                return None;
            }
        }

        Some(bcoords)
    }

    /// Returns `true` if both `self` and `other` correspond to points on the same feature of a tetrahedron.
    ///
    /// Two point locations are considered to be on the same feature if they're both:
    /// - On the same vertex (same vertex index)
    /// - On the same edge (same edge index)
    /// - On the same face (same face index)
    /// - Both inside the tetrahedron (`OnSolid`)
    ///
    /// This is useful for determining if two points share a common geometric feature,
    /// which is important in collision detection and contact point generation.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::TetrahedronPointLocation;
    ///
    /// let loc1 = TetrahedronPointLocation::OnVertex(0);
    /// let loc2 = TetrahedronPointLocation::OnVertex(0);
    /// let loc3 = TetrahedronPointLocation::OnVertex(1);
    ///
    /// assert!(loc1.same_feature_as(&loc2));  // Same vertex
    /// assert!(!loc1.same_feature_as(&loc3)); // Different vertices
    ///
    /// let edge1 = TetrahedronPointLocation::OnEdge(0, [0.5, 0.5]);
    /// let edge2 = TetrahedronPointLocation::OnEdge(0, [0.3, 0.7]);
    /// let edge3 = TetrahedronPointLocation::OnEdge(1, [0.5, 0.5]);
    ///
    /// assert!(edge1.same_feature_as(&edge2));  // Same edge, different coords
    /// assert!(!edge1.same_feature_as(&edge3)); // Different edges
    /// # }
    /// ```
    pub fn same_feature_as(&self, other: &TetrahedronPointLocation) -> bool {
        match (*self, *other) {
            (TetrahedronPointLocation::OnVertex(i), TetrahedronPointLocation::OnVertex(j)) => {
                i == j
            }
            (TetrahedronPointLocation::OnEdge(i, _), TetrahedronPointLocation::OnEdge(j, _)) => {
                i == j
            }
            (TetrahedronPointLocation::OnFace(i, _), TetrahedronPointLocation::OnFace(j, _)) => {
                i == j
            }
            (TetrahedronPointLocation::OnSolid, TetrahedronPointLocation::OnSolid) => true,
            _ => false,
        }
    }
}

impl Tetrahedron {
    /// Creates a tetrahedron from four points.
    ///
    /// The vertices can be specified in any order, but the order affects the
    /// orientation and signed volume. For a tetrahedron with positive volume,
    /// vertex `d` should be on the positive side of the plane defined by the
    /// counter-clockwise triangle `(a, b, c)`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Tetrahedron;
    /// use parry3d::math::Vector;
    ///
    /// // Create a simple tetrahedron
    /// let tetra = Tetrahedron::new(
    ///     Vector::new(0.0, 0.0, 0.0),
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0),
    /// );
    ///
    /// assert!(tetra.volume() > 0.0);
    /// # }
    /// ```
    #[inline]
    pub fn new(a: Vector, b: Vector, c: Vector, d: Vector) -> Tetrahedron {
        Tetrahedron { a, b, c, d }
    }

    /// Creates the reference to a tetrahedron from the reference to an array of four points.
    ///
    /// This is a zero-cost conversion that reinterprets the array as a tetrahedron.
    /// The array elements correspond to vertices `[a, b, c, d]`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Tetrahedron;
    /// use parry3d::math::Vector;
    ///
    /// let points = [
    ///     Vector::new(0.0, 0.0, 0.0),
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0),
    /// ];
    ///
    /// let tetra = Tetrahedron::from_array(&points);
    /// assert_eq!(tetra.a, points[0]);
    /// assert_eq!(tetra.b, points[1]);
    /// # }
    /// ```
    pub fn from_array(arr: &[Vector; 4]) -> &Tetrahedron {
        unsafe { mem::transmute(arr) }
    }

    /// Returns the i-th face of this tetrahedron.
    ///
    /// A tetrahedron has 4 triangular faces indexed from 0 to 3:
    /// - Face 0: triangle ABC
    /// - Face 1: triangle ABD
    /// - Face 2: triangle ACD
    /// - Face 3: triangle BCD
    ///
    /// # Panics
    ///
    /// Panics if `i >= 4`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Tetrahedron;
    /// use parry3d::math::Vector;
    ///
    /// let tetra = Tetrahedron::new(
    ///     Vector::new(0.0, 0.0, 0.0),
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0),
    /// );
    ///
    /// // Get the first face (triangle ABC)
    /// let face = tetra.face(0);
    /// assert_eq!(face.a, tetra.a);
    /// assert_eq!(face.b, tetra.b);
    /// assert_eq!(face.c, tetra.c);
    ///
    /// // Get the last face (triangle BCD)
    /// let face = tetra.face(3);
    /// assert_eq!(face.a, tetra.b);
    /// assert_eq!(face.b, tetra.c);
    /// assert_eq!(face.c, tetra.d);
    /// # }
    /// ```
    pub fn face(&self, i: usize) -> Triangle {
        match i {
            0 => Triangle::new(self.a, self.b, self.c),
            1 => Triangle::new(self.a, self.b, self.d),
            2 => Triangle::new(self.a, self.c, self.d),
            3 => Triangle::new(self.b, self.c, self.d),
            _ => panic!("Tetrahedron face index out of bounds (must be < 4."),
        }
    }

    /// Returns the indices of the vertices of the i-th face of this tetrahedron.
    ///
    /// Returns a tuple `(v1, v2, v3)` where each value is a vertex index (0=a, 1=b, 2=c, 3=d).
    ///
    /// Face to vertex index mapping:
    /// - Face 0: (0, 1, 2) = triangle ABC
    /// - Face 1: (0, 1, 3) = triangle ABD
    /// - Face 2: (0, 2, 3) = triangle ACD
    /// - Face 3: (1, 2, 3) = triangle BCD
    ///
    /// # Panics
    ///
    /// Panics if `i >= 4`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Tetrahedron;
    ///
    /// // Get indices for face 0 (triangle ABC)
    /// let (v1, v2, v3) = Tetrahedron::face_ids(0);
    /// assert_eq!((v1, v2, v3), (0, 1, 2));  // vertices a, b, c
    ///
    /// // Get indices for face 3 (triangle BCD)
    /// let (v1, v2, v3) = Tetrahedron::face_ids(3);
    /// assert_eq!((v1, v2, v3), (1, 2, 3));  // vertices b, c, d
    /// # }
    /// ```
    pub fn face_ids(i: u32) -> (u32, u32, u32) {
        match i {
            0 => (0, 1, 2),
            1 => (0, 1, 3),
            2 => (0, 2, 3),
            3 => (1, 2, 3),
            _ => panic!("Tetrahedron face index out of bounds (must be < 4."),
        }
    }

    /// Returns the i-th edge of this tetrahedron.
    ///
    /// A tetrahedron has 6 edges indexed from 0 to 5:
    /// - Edge 0: segment AB
    /// - Edge 1: segment AC
    /// - Edge 2: segment AD
    /// - Edge 3: segment BC
    /// - Edge 4: segment BD
    /// - Edge 5: segment CD
    ///
    /// # Panics
    ///
    /// Panics if `i >= 6`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Tetrahedron;
    /// use parry3d::math::Vector;
    ///
    /// let tetra = Tetrahedron::new(
    ///     Vector::new(0.0, 0.0, 0.0),
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0),
    /// );
    ///
    /// // Get edge 0 (segment AB)
    /// let edge = tetra.edge(0);
    /// assert_eq!(edge.a, tetra.a);
    /// assert_eq!(edge.b, tetra.b);
    ///
    /// // Get edge 5 (segment CD)
    /// let edge = tetra.edge(5);
    /// assert_eq!(edge.a, tetra.c);
    /// assert_eq!(edge.b, tetra.d);
    /// # }
    /// ```
    pub fn edge(&self, i: u32) -> Segment {
        match i {
            0 => Segment::new(self.a, self.b),
            1 => Segment::new(self.a, self.c),
            2 => Segment::new(self.a, self.d),
            3 => Segment::new(self.b, self.c),
            4 => Segment::new(self.b, self.d),
            5 => Segment::new(self.c, self.d),
            _ => panic!("Tetrahedron edge index out of bounds (must be < 6)."),
        }
    }

    /// Returns the indices of the vertices of the i-th edge of this tetrahedron.
    ///
    /// Returns a tuple `(v1, v2)` where each value is a vertex index (0=a, 1=b, 2=c, 3=d).
    ///
    /// Edge to vertex index mapping:
    /// - Edge 0: (0, 1) = segment AB
    /// - Edge 1: (0, 2) = segment AC
    /// - Edge 2: (0, 3) = segment AD
    /// - Edge 3: (1, 2) = segment BC
    /// - Edge 4: (1, 3) = segment BD
    /// - Edge 5: (2, 3) = segment CD
    ///
    /// # Panics
    ///
    /// Panics if `i >= 6`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Tetrahedron;
    ///
    /// // Get indices for edge 0 (segment AB)
    /// let (v1, v2) = Tetrahedron::edge_ids(0);
    /// assert_eq!((v1, v2), (0, 1));  // vertices a, b
    ///
    /// // Get indices for edge 5 (segment CD)
    /// let (v1, v2) = Tetrahedron::edge_ids(5);
    /// assert_eq!((v1, v2), (2, 3));  // vertices c, d
    /// # }
    /// ```
    pub fn edge_ids(i: u32) -> (u32, u32) {
        match i {
            0 => (0, 1),
            1 => (0, 2),
            2 => (0, 3),
            3 => (1, 2),
            4 => (1, 3),
            5 => (2, 3),
            _ => panic!("Tetrahedron edge index out of bounds (must be < 6)."),
        }
    }

    /// Computes the barycentric coordinates of the given point in the coordinate system of this tetrahedron.
    ///
    /// Barycentric coordinates express a point as a weighted combination of the tetrahedron's
    /// vertices. For point `p`, the returned array `[wa, wb, wc, wd]` satisfies:
    /// `p = wa*a + wb*b + wc*c + wd*d` where `wa + wb + wc + wd = 1.0`.
    ///
    /// These coordinates are useful for:
    /// - Interpolating values defined at vertices
    /// - Determining if a point is inside the tetrahedron (all weights are non-negative)
    /// - Computing distances and projections
    ///
    /// # Returns
    ///
    /// - `Some([wa, wb, wc, wd])`: The barycentric coordinates
    /// - `None`: If the tetrahedron is degenerate (zero volume, coplanar vertices)
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Tetrahedron;
    /// use parry3d::math::Vector;
    ///
    /// let tetra = Tetrahedron::new(
    ///     Vector::new(0.0, 0.0, 0.0),
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0),
    /// );
    ///
    /// // Vector at vertex a
    /// let bcoords = tetra.barycentric_coordinates(tetra.a).unwrap();
    /// assert!((bcoords[0] - 1.0).abs() < 1e-6);
    /// assert!(bcoords[1].abs() < 1e-6);
    ///
    /// // Vector at center
    /// let center = tetra.center();
    /// let bcoords = tetra.barycentric_coordinates(center).unwrap();
    /// // All coordinates should be approximately 0.25
    /// for coord in &bcoords {
    ///     assert!((coord - 0.25).abs() < 1e-6);
    /// }
    /// # }
    /// ```
    pub fn barycentric_coordinates(&self, p: Vector) -> Option<[Real; 4]> {
        let ab = self.b - self.a;
        let ac = self.c - self.a;
        let ad = self.d - self.a;
        let m = Matrix::from_cols_array(&[ab.x, ab.y, ab.z, ac.x, ac.y, ac.z, ad.x, ad.y, ad.z]);

        m.try_inverse().map(|im| {
            let bcoords = im * (p - self.a);
            [
                1.0 - bcoords.x - bcoords.y - bcoords.z,
                bcoords.x,
                bcoords.y,
                bcoords.z,
            ]
        })
    }

    /// Computes the volume of this tetrahedron.
    ///
    /// The volume is always non-negative, regardless of vertex ordering.
    /// For the signed volume (which can be negative), use [`signed_volume`](Self::signed_volume).
    ///
    /// # Formula
    ///
    /// Volume = |det(b-a, c-a, d-a)| / 6
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Tetrahedron;
    /// use parry3d::math::Vector;
    ///
    /// let tetra = Tetrahedron::new(
    ///     Vector::new(0.0, 0.0, 0.0),
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0),
    /// );
    ///
    /// let volume = tetra.volume();
    /// assert!((volume - 1.0/6.0).abs() < 1e-6);
    /// # }
    /// ```
    #[inline]
    pub fn volume(&self) -> Real {
        self.signed_volume().abs()
    }

    /// Computes the signed volume of this tetrahedron.
    ///
    /// The sign of the volume depends on the vertex ordering:
    /// - **Positive**: Vertex `d` is on the positive side of the plane defined by
    ///   the counter-clockwise triangle `(a, b, c)`
    /// - **Negative**: Vertex `d` is on the negative side (opposite orientation)
    /// - **Zero**: The tetrahedron is degenerate (all vertices are coplanar)
    ///
    /// # Formula
    ///
    /// Signed Volume = det(b-a, c-a, d-a) / 6
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Tetrahedron;
    /// use parry3d::math::Vector;
    ///
    /// let tetra = Tetrahedron::new(
    ///     Vector::new(0.0, 0.0, 0.0),
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0),
    /// );
    ///
    /// let signed_vol = tetra.signed_volume();
    /// assert!(signed_vol > 0.0);  // Positive orientation
    ///
    /// // Swap two vertices to flip orientation
    /// let tetra_flipped = Tetrahedron::new(
    ///     tetra.a, tetra.c, tetra.b, tetra.d
    /// );
    /// let signed_vol_flipped = tetra_flipped.signed_volume();
    /// assert!(signed_vol_flipped < 0.0);  // Negative orientation
    /// assert!((signed_vol + signed_vol_flipped).abs() < 1e-6);  // Same magnitude
    /// # }
    /// ```
    #[inline]
    pub fn signed_volume(&self) -> Real {
        let p1p2 = self.b - self.a;
        let p1p3 = self.c - self.a;
        let p1p4 = self.d - self.a;

        let mat = CrossMatrix::from_cols(p1p2, p1p3, p1p4);

        mat.determinant() / 6.0
    }

    /// Computes the center of this tetrahedron.
    ///
    /// The center (also called centroid or barycenter) is the average of all four vertices.
    /// It's the point where all barycentric coordinates are equal to 0.25.
    ///
    /// # Formula
    ///
    /// Center = (a + b + c + d) / 4
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::shape::Tetrahedron;
    /// use parry3d::math::Vector;
    ///
    /// let tetra = Tetrahedron::new(
    ///     Vector::new(0.0, 0.0, 0.0),
    ///     Vector::new(1.0, 0.0, 0.0),
    ///     Vector::new(0.0, 1.0, 0.0),
    ///     Vector::new(0.0, 0.0, 1.0),
    /// );
    ///
    /// let center = tetra.center();
    /// assert!((center.x - 0.25).abs() < 1e-6);
    /// assert!((center.y - 0.25).abs() < 1e-6);
    /// assert!((center.z - 0.25).abs() < 1e-6);
    ///
    /// // The center has equal barycentric coordinates
    /// let bcoords = tetra.barycentric_coordinates(center).unwrap();
    /// for coord in &bcoords {
    ///     assert!((coord - 0.25).abs() < 1e-6);
    /// }
    /// # }
    /// ```
    #[inline]
    pub fn center(&self) -> Vector {
        utils::center(&[self.a, self.b, self.c, self.d])
    }
}
