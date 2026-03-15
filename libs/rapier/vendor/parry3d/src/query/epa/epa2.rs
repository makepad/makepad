//! Two-dimensional penetration depth queries using the Expanding Polytope Algorithm.
//!
//! This module provides the 2D-specific implementation of EPA, which works with
//! polygons (edges) rather than polyhedra (faces) as in the 3D version.

use alloc::{collections::BinaryHeap, vec::Vec};
use core::cmp::Ordering;
use num::Bounded;

use crate::math::{Pose, Real, Vector};
use crate::query::gjk::{self, ConstantOrigin, CsoPoint, VoronoiSimplex};
use crate::shape::SupportMap;
use crate::utils;

#[derive(Copy, Clone, PartialEq)]
struct FaceId {
    id: usize,
    neg_dist: Real,
}

impl FaceId {
    fn new(id: usize, neg_dist: Real) -> Option<Self> {
        if neg_dist > gjk::eps_tol() {
            None
        } else {
            Some(FaceId { id, neg_dist })
        }
    }
}

impl Eq for FaceId {}

impl PartialOrd for FaceId {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FaceId {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        if self.neg_dist < other.neg_dist {
            Ordering::Less
        } else if self.neg_dist > other.neg_dist {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    }
}

#[derive(Clone, Debug)]
struct Face {
    pts: [usize; 2],
    normal: Vector,
    proj: Vector,
    bcoords: [Real; 2],
    deleted: bool,
}

impl Face {
    pub fn new(vertices: &[CsoPoint], pts: [usize; 2]) -> (Self, bool) {
        if let Some((proj, bcoords)) =
            project_origin(vertices[pts[0]].point, vertices[pts[1]].point)
        {
            (Self::new_with_proj(vertices, proj, bcoords, pts), true)
        } else {
            (
                Self::new_with_proj(vertices, Vector::ZERO, [0.0; 2], pts),
                false,
            )
        }
    }

    pub fn new_with_proj(
        vertices: &[CsoPoint],
        proj: Vector,
        bcoords: [Real; 2],
        pts: [usize; 2],
    ) -> Self {
        let normal;
        let deleted;

        if let Some(n) = utils::ccw_face_normal([vertices[pts[0]].point, vertices[pts[1]].point]) {
            normal = n;
            deleted = false;
        } else {
            normal = Vector::ZERO;
            deleted = true;
        }

        Face {
            pts,
            normal,
            proj,
            bcoords,
            deleted,
        }
    }

    pub fn closest_points(&self, vertices: &[CsoPoint]) -> (Vector, Vector) {
        (
            vertices[self.pts[0]].orig1 * self.bcoords[0]
                + vertices[self.pts[1]].orig1 * self.bcoords[1],
            vertices[self.pts[0]].orig2 * self.bcoords[0]
                + vertices[self.pts[1]].orig2 * self.bcoords[1],
        )
    }
}

/// The Expanding Polytope Algorithm in 2D.
///
/// This structure computes the penetration depth between two shapes when they are overlapping.
/// It's used after GJK (Gilbert-Johnson-Keerthi) determines that two shapes are penetrating.
///
/// # What does EPA do?
///
/// EPA finds:
/// - The **penetration depth**: How far the shapes are overlapping
/// - The **contact normal**: The direction to separate the shapes
/// - The **contact points**: Where the shapes are touching on each surface
///
/// # How it works in 2D
///
/// In 2D, EPA maintains a polygon in the Minkowski difference space (CSO - Configuration Space
/// Obstacle) that encloses the origin. It iteratively:
///
/// 1. Finds the edge closest to the origin
/// 2. Expands the polygon by adding a new support point in the direction of that edge's normal
/// 3. Updates the polygon structure with the new point
/// 4. Repeats until the polygon cannot expand further (convergence)
///
/// The final closest edge provides the penetration depth (distance to origin) and contact normal.
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim2", feature = "f32"))] {
/// use parry2d::query::epa::EPA;
/// use parry2d::query::gjk::VoronoiSimplex;
/// use parry2d::shape::Ball;
/// use parry2d::math::Pose;
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
/// let pos12 = Pose::translation(1.5, 0.0); // Overlapping circles
///
/// // After GJK determines penetration and fills a simplex:
/// let mut epa = EPA::new();
/// let simplex = VoronoiSimplex::new(); // Would be filled by GJK
///
/// // EPA computes the contact details
/// // if let Some((pt1, pt2, normal)) = epa.closest_points(&pos12, &ball1, &ball2, &simplex) {
/// //     println!("Penetration depth: {}", (pt2 - pt1).length());
/// //     println!("Contact normal: {}", normal);
/// // }
/// # }
/// ```
///
/// # Reusability
///
/// The `EPA` structure can be reused across multiple queries to avoid allocations.
/// Internal buffers are cleared and reused in each call to [`closest_points`](EPA::closest_points).
///
/// # Convergence and Failure Cases
///
/// EPA may return `None` in these situations:
/// - The shapes are not actually penetrating (GJK should be used instead)
/// - Degenerate or nearly-degenerate geometry causes numerical instability
/// - The initial simplex from GJK is invalid
/// - The algorithm fails to converge after 100 iterations
///
/// When `None` is returned, the shapes may be touching at a single point or edge, or there
/// may be numerical precision issues with the input geometry.
#[derive(Default)]
pub struct EPA {
    vertices: Vec<CsoPoint>,
    faces: Vec<Face>,
    heap: BinaryHeap<FaceId>,
}

impl EPA {
    /// Creates a new instance of the 2D Expanding Polytope Algorithm.
    ///
    /// This allocates internal data structures (vertices, faces, and a priority heap).
    /// The same `EPA` instance can be reused for multiple queries to avoid repeated allocations.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::query::epa::EPA;
    ///
    /// let mut epa = EPA::new();
    /// // Use epa for multiple queries...
    /// # }
    /// ```
    pub fn new() -> Self {
        EPA::default()
    }

    fn reset(&mut self) {
        self.vertices.clear();
        self.faces.clear();
        self.heap.clear();
    }

    /// Projects the origin onto the boundary of the given shape.
    ///
    /// This is a specialized version of [`closest_points`](EPA::closest_points) for projecting
    /// a point (the origin) onto a single shape's surface.
    ///
    /// # Parameters
    ///
    /// - `m`: The position and orientation of the shape in world space
    /// - `g`: The shape to project onto (must implement [`SupportMap`])
    /// - `simplex`: A Voronoi simplex from GJK that encloses the origin (indicating the origin
    ///   is inside the shape)
    ///
    /// # Returns
    ///
    /// - `Some(point)`: The closest point on the shape's boundary to the origin, in the local
    ///   space of `g`
    /// - `None`: If the origin is not inside the shape, or if EPA failed to converge
    ///
    /// # Prerequisites
    ///
    /// The origin **must be inside** the shape. If it's outside, use GJK instead.
    /// Typically, you would:
    /// 1. Run GJK to detect if a point is inside a shape
    /// 2. If inside, use this method to find the closest boundary point
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::query::epa::EPA;
    /// use parry2d::query::gjk::VoronoiSimplex;
    /// use parry2d::shape::Ball;
    /// use parry2d::math::Pose;
    ///
    /// let ball = Ball::new(2.0);
    /// let pos = Pose::identity();
    ///
    /// // Assume GJK determined the origin is inside and filled simplex
    /// let simplex = VoronoiSimplex::new();
    /// let mut epa = EPA::new();
    ///
    /// // Find the closest point on the ball's surface to the origin
    /// // if let Some(surface_point) = epa.project_origin(&pos, &ball, &simplex) {
    /// //     println!("Closest surface point: {:?}", surface_point);
    /// // }
    /// # }
    /// ```
    pub fn project_origin<G: ?Sized + SupportMap>(
        &mut self,
        m: &Pose,
        g: &G,
        simplex: &VoronoiSimplex,
    ) -> Option<Vector> {
        self.closest_points(&m.inverse(), g, &ConstantOrigin, simplex)
            .map(|(p, _, _)| p)
    }

    /// Computes the closest points between two penetrating shapes and their contact normal.
    ///
    /// This is the main EPA method that computes detailed contact information for overlapping shapes.
    /// It should be called after GJK determines that two shapes are penetrating.
    ///
    /// # Parameters
    ///
    /// - `pos12`: The relative position/orientation from `g2`'s frame to `g1`'s frame
    ///   (typically computed as `pos1.inverse() * pos2`)
    /// - `g1`: The first shape (must implement [`SupportMap`])
    /// - `g2`: The second shape (must implement [`SupportMap`])
    /// - `simplex`: A Voronoi simplex from GJK that encloses the origin, indicating penetration
    ///
    /// # Returns
    ///
    /// Returns `Some((point1, point2, normal))` where:
    /// - `point1`: Contact point on shape `g1` in `g1`'s local frame
    /// - `point2`: Contact point on shape `g2` in `g2`'s local frame
    /// - `normal`: Contact normal pointing from `g2` toward `g1`, normalized
    ///
    /// The **penetration depth** can be computed as `(point1 - point2).length()` after transforming
    /// both points to the same coordinate frame.
    ///
    /// Returns `None` if:
    /// - The shapes are not actually penetrating
    /// - EPA fails to converge (degenerate geometry, numerical issues)
    /// - The simplex is invalid or empty
    /// - The algorithm reaches the maximum iteration limit (100 iterations)
    ///
    /// # Prerequisites
    ///
    /// The shapes **must be penetrating**. The typical workflow is:
    /// 1. Run GJK to check if shapes intersect
    /// 2. If GJK detects penetration and returns a simplex enclosing the origin
    /// 3. Use EPA with that simplex to compute detailed contact information
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim2", feature = "f32"))] {
    /// use parry2d::query::epa::EPA;
    /// use parry2d::query::gjk::{GJKResult, VoronoiSimplex};
    /// use parry2d::shape::Ball;
    /// use parry2d::math::Pose;
    ///
    /// let ball1 = Ball::new(1.0);
    /// let ball2 = Ball::new(1.0);
    ///
    /// let pos1 = Pose::identity();
    /// let pos2 = Pose::translation(1.5, 0.0); // Overlapping
    /// let pos12 = pos1.inverse() * pos2;
    ///
    /// // After GJK detects penetration:
    /// let mut simplex = VoronoiSimplex::new();
    /// // ... simplex would be filled by GJK ...
    ///
    /// let mut epa = EPA::new();
    /// // if let Some((pt1, pt2, normal)) = epa.closest_points(&pos12, &ball1, &ball2, &simplex) {
    /// //     println!("Contact on shape 1: {:?}", pt1);
    /// //     println!("Contact on shape 2: {:?}", pt2);
    /// //     println!("Contact normal: {}", normal);
    /// //     println!("Penetration depth: ~0.5");
    /// // }
    /// # }
    /// ```
    ///
    /// # Technical Details
    ///
    /// The algorithm works in the **Minkowski difference space** (also called the Configuration
    /// Space Obstacle or CSO), where the difference between support points of the two shapes
    /// forms a new shape. When shapes penetrate, this CSO contains the origin.
    ///
    /// EPA iteratively expands a polygon (in 2D) that surrounds the origin, finding the edge
    /// closest to the origin at each iteration. This closest edge defines the penetration
    /// depth and contact normal.
    pub fn closest_points<G1, G2>(
        &mut self,
        pos12: &Pose,
        g1: &G1,
        g2: &G2,
        simplex: &VoronoiSimplex,
    ) -> Option<(Vector, Vector, Vector)>
    where
        G1: ?Sized + SupportMap,
        G2: ?Sized + SupportMap,
    {
        let _eps: Real = crate::math::DEFAULT_EPSILON;
        let _eps_tol = _eps * 100.0;

        self.reset();

        /*
         * Initialization.
         */
        for i in 0..simplex.dimension() + 1 {
            self.vertices.push(*simplex.point(i));
        }

        if simplex.dimension() == 0 {
            const MAX_ITERS: usize = 100; // If there is no convergence, just use whatever direction was extracted so fare

            // The contact is vertex-vertex.
            // We need to determine a valid normal that lies
            // on both vertices' normal cone.
            let mut n = Vector::Y;

            // First, find a vector on the first vertex tangent cone.
            let orig1 = self.vertices[0].orig1;
            for _ in 0..MAX_ITERS {
                let supp1 = g1.local_support_point(n);
                if let Some(tangent) = (supp1 - orig1).try_normalize() {
                    if n.dot(tangent) < _eps_tol {
                        break;
                    }

                    n = Vector::new(-tangent.y, tangent.x);
                } else {
                    break;
                }
            }

            // Second, ensure the direction lies on the second vertex's tangent cone.
            let orig2 = self.vertices[0].orig2;
            for _ in 0..MAX_ITERS {
                let supp2 = g2.support_point(pos12, -n);
                if let Some(tangent) = (supp2 - orig2).try_normalize() {
                    if (-n).dot(tangent) < _eps_tol {
                        break;
                    }

                    n = Vector::new(-tangent.y, tangent.x);
                } else {
                    break;
                }
            }

            return Some((Vector::ZERO, Vector::ZERO, n));
        } else if simplex.dimension() == 2 {
            let dp1 = self.vertices[1] - self.vertices[0];
            let dp2 = self.vertices[2] - self.vertices[0];

            if dp1.perp_dot(dp2) < 0.0 {
                self.vertices.swap(1, 2)
            }

            let pts1 = [0, 1];
            let pts2 = [1, 2];
            let pts3 = [2, 0];

            let (face1, proj_inside1) = Face::new(&self.vertices, pts1);
            let (face2, proj_inside2) = Face::new(&self.vertices, pts2);
            let (face3, proj_inside3) = Face::new(&self.vertices, pts3);

            self.faces.push(face1);
            self.faces.push(face2);
            self.faces.push(face3);

            if proj_inside1 {
                let dist1 = self.faces[0].normal.dot(self.vertices[0].point);
                self.heap.push(FaceId::new(0, -dist1)?);
            }

            if proj_inside2 {
                let dist2 = self.faces[1].normal.dot(self.vertices[1].point);
                self.heap.push(FaceId::new(1, -dist2)?);
            }

            if proj_inside3 {
                let dist3 = self.faces[2].normal.dot(self.vertices[2].point);
                self.heap.push(FaceId::new(2, -dist3)?);
            }

            if !(proj_inside1 || proj_inside2 || proj_inside3) {
                // Related issues:
                // https://github.com/dimforge/parry/issues/253
                // https://github.com/dimforge/parry/issues/246
                log::debug!("Hit unexpected state in EPA: failed to project the origin on the initial simplex.");
                return None;
            }
        } else {
            let pts1 = [0, 1];
            let pts2 = [1, 0];

            self.faces.push(Face::new_with_proj(
                &self.vertices,
                Vector::ZERO,
                [1.0, 0.0],
                pts1,
            ));
            self.faces.push(Face::new_with_proj(
                &self.vertices,
                Vector::ZERO,
                [1.0, 0.0],
                pts2,
            ));

            let dist1 = self.faces[0].normal.dot(self.vertices[0].point);
            let dist2 = self.faces[1].normal.dot(self.vertices[1].point);

            self.heap.push(FaceId::new(0, dist1)?);
            self.heap.push(FaceId::new(1, dist2)?);
        }

        let mut niter = 0;
        let mut max_dist = Real::max_value();
        let mut best_face_id = *self.heap.peek().unwrap();
        let mut old_dist = 0.0;

        /*
         * Run the expansion.
         */
        while let Some(face_id) = self.heap.pop() {
            // Create new faces.
            let face = self.faces[face_id.id].clone();

            if face.deleted {
                continue;
            }

            let cso_point = CsoPoint::from_shapes(pos12, g1, g2, face.normal);
            let support_point_id = self.vertices.len();
            self.vertices.push(cso_point);

            let candidate_max_dist = cso_point.point.dot(face.normal);

            if candidate_max_dist < max_dist {
                best_face_id = face_id;
                max_dist = candidate_max_dist;
            }

            let curr_dist = -face_id.neg_dist;

            if max_dist - curr_dist < _eps_tol ||
                // Accept the intersection as the algorithm is stuck and no new points will be found
                // This happens because of numerical stability issue
                ((curr_dist - old_dist).abs() < _eps && candidate_max_dist < max_dist)
            {
                let best_face = &self.faces[best_face_id.id];
                let cpts = best_face.closest_points(&self.vertices);
                return Some((cpts.0, cpts.1, best_face.normal));
            }

            old_dist = curr_dist;

            let pts1 = [face.pts[0], support_point_id];
            let pts2 = [support_point_id, face.pts[1]];

            let new_faces = [
                Face::new(&self.vertices, pts1),
                Face::new(&self.vertices, pts2),
            ];

            for f in new_faces.iter() {
                if f.1 {
                    let dist = f.0.normal.dot(f.0.proj);
                    if dist < curr_dist {
                        // TODO: if we reach this point, there were issues due to
                        // numerical errors.
                        let cpts = f.0.closest_points(&self.vertices);
                        return Some((cpts.0, cpts.1, f.0.normal));
                    }

                    if !f.0.deleted {
                        self.heap.push(FaceId::new(self.faces.len(), -dist)?);
                    }
                }

                self.faces.push(f.0.clone());
            }

            niter += 1;
            if niter > 100 {
                // if we reached this point, our algorithm didn't converge to what precision we wanted.
                // still return an intersection point, as it's probably close enough.
                break;
            }
        }

        let best_face = &self.faces[best_face_id.id];
        let cpts = best_face.closest_points(&self.vertices);
        Some((cpts.0, cpts.1, best_face.normal))
    }
}

fn project_origin(a: Vector, b: Vector) -> Option<(Vector, [Real; 2])> {
    let ab = b - a;
    let ap = -a;
    let ab_ap = ab.dot(ap);
    let sqnab = ab.length_squared();

    if sqnab == 0.0 {
        return None;
    }

    let position_on_segment;

    let _eps: Real = gjk::eps_tol();

    if ab_ap < -_eps || ab_ap > sqnab + _eps {
        // Voronoï region of vertex 'a' or 'b'.
        None
    } else {
        // Voronoï region of the segment interior.
        position_on_segment = ab_ap / sqnab;

        let res = a + ab * position_on_segment;

        Some((res, [1.0 - position_on_segment, position_on_segment]))
    }
}
