//! Three-dimensional penetration depth queries using the Expanding Polytope Algorithm.
//!
//! This module provides the 3D-specific implementation of EPA, which works with
//! polyhedra (triangular faces) rather than polygons (edges) as in the 2D version.

use crate::math::{Pose, Real, Vector};
use crate::query::gjk::{self, ConstantOrigin, CsoPoint, VoronoiSimplex};
use crate::query::PointQueryWithLocation;
use crate::shape::{SupportMap, Triangle, TrianglePointLocation};
use crate::utils;
use alloc::collections::BinaryHeap;
use alloc::format;
use alloc::vec::Vec;
use core::cmp::Ordering;
use num::Bounded;

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
    pts: [usize; 3],
    adj: [usize; 3],
    normal: Vector,
    bcoords: [Real; 3],
    deleted: bool,
}

impl Face {
    pub fn new_with_proj(
        vertices: &[CsoPoint],
        bcoords: [Real; 3],
        pts: [usize; 3],
        adj: [usize; 3],
    ) -> Self {
        let normal;

        if let Some(n) = utils::ccw_face_normal([
            vertices[pts[0]].point,
            vertices[pts[1]].point,
            vertices[pts[2]].point,
        ]) {
            normal = n;
        } else {
            // This is a bit of a hack for degenerate faces.
            // TODO: It will work OK with our current code, though
            // we should do this in another way to avoid any risk
            // of misusing the face normal in the future.
            normal = Vector::ZERO;
        }

        Face {
            pts,
            bcoords,
            adj,
            normal,
            deleted: false,
        }
    }

    pub fn new(vertices: &[CsoPoint], pts: [usize; 3], adj: [usize; 3]) -> (Self, bool) {
        let tri = Triangle::new(
            vertices[pts[0]].point,
            vertices[pts[1]].point,
            vertices[pts[2]].point,
        );
        let (proj, loc) = tri.project_local_point_and_get_location(Vector::ZERO, true);

        match loc {
            TrianglePointLocation::OnVertex(_) | TrianglePointLocation::OnEdge(_, _) => {
                let eps_tol = crate::math::DEFAULT_EPSILON * 100.0; // Same as in closest_points
                (
                    // barycentric_coordinates is guaranteed to work in OnVertex and OnEdge locations
                    Self::new_with_proj(vertices, loc.barycentric_coordinates().unwrap(), pts, adj),
                    proj.is_inside_eps(Vector::ZERO, eps_tol),
                )
            }
            TrianglePointLocation::OnFace(_, bcoords) => {
                (Self::new_with_proj(vertices, bcoords, pts, adj), true)
            }
            _ => (Self::new_with_proj(vertices, [0.0; 3], pts, adj), false),
        }
    }

    pub fn closest_points(&self, vertices: &[CsoPoint]) -> (Vector, Vector) {
        (
            vertices[self.pts[0]].orig1 * self.bcoords[0]
                + vertices[self.pts[1]].orig1 * self.bcoords[1]
                + vertices[self.pts[2]].orig1 * self.bcoords[2],
            vertices[self.pts[0]].orig2 * self.bcoords[0]
                + vertices[self.pts[1]].orig2 * self.bcoords[1]
                + vertices[self.pts[2]].orig2 * self.bcoords[2],
        )
    }

    pub fn contains_point(&self, id: usize) -> bool {
        self.pts[0] == id || self.pts[1] == id || self.pts[2] == id
    }

    pub fn next_ccw_pt_id(&self, id: usize) -> usize {
        if self.pts[0] == id {
            1
        } else if self.pts[1] == id {
            2
        } else {
            if self.pts[2] != id {
                log::debug!(
                    "Hit unexpected state in EPA: found index {}, expected: {}.",
                    self.pts[2],
                    id
                );
            }

            0
        }
    }

    pub fn can_be_seen_by(&self, vertices: &[CsoPoint], point: usize, opp_pt_id: usize) -> bool {
        let p0 = &vertices[self.pts[opp_pt_id]].point;
        let p1 = &vertices[self.pts[(opp_pt_id + 1) % 3]].point;
        let p2 = &vertices[self.pts[(opp_pt_id + 2) % 3]].point;
        let pt = &vertices[point].point;

        // NOTE: it is important that we return true for the case where
        // the dot product is zero. This is because degenerate faces will
        // have a zero normal, causing the dot product to be zero.
        // So return true for these case will let us skip the triangle
        // during silhouette computation.
        (*pt - *p0).dot(self.normal) >= -gjk::eps_tol()
            || Triangle::new(*p1, *p2, *pt).is_affinely_dependent()
    }
}

struct SilhouetteEdge {
    face_id: usize,
    opp_pt_id: usize,
}

impl SilhouetteEdge {
    pub fn new(face_id: usize, opp_pt_id: usize) -> Self {
        SilhouetteEdge { face_id, opp_pt_id }
    }
}

/// The Expanding Polytope Algorithm in 3D.
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
/// # How it works in 3D
///
/// In 3D, EPA maintains a convex polyhedron (made of triangular faces) in the Minkowski
/// difference space (CSO - Configuration Space Obstacle) that encloses the origin. It iteratively:
///
/// 1. Finds the triangular face closest to the origin
/// 2. Expands the polyhedron by adding a new support point in the direction of that face's normal
/// 3. Removes faces that can be "seen" from the new point (they're now inside)
/// 4. Creates new faces connecting the boundary edges (silhouette) to the new point
/// 5. Repeats until the polyhedron cannot expand further (convergence)
///
/// The final closest face provides the penetration depth (distance to origin) and contact normal.
///
/// # Example
///
/// ```
/// # #[cfg(all(feature = "dim3", feature = "f32"))] {
/// use parry3d::query::epa::EPA;
/// use parry3d::query::gjk::VoronoiSimplex;
/// use parry3d::shape::Ball;
/// use parry3d::math::Pose;
///
/// let ball1 = Ball::new(1.0);
/// let ball2 = Ball::new(1.0);
/// let pos12 = Pose::translation(1.5, 0.0, 0.0); // Overlapping spheres
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
/// - Silhouette extraction fails (topology issues)
///
/// When `None` is returned, the shapes may be touching at a single point, edge, or face,
/// or there may be numerical precision issues with the input geometry.
///
/// # Complexity
///
/// The 3D EPA implementation is more complex than 2D because:
/// - It maintains a 3D mesh topology with face adjacency information
/// - It needs to compute silhouettes (visible edges from a point)
/// - It handles more degenerate cases (coplanar faces, edge cases)
#[derive(Default)]
pub struct EPA {
    vertices: Vec<CsoPoint>,
    faces: Vec<Face>,
    silhouette: Vec<SilhouetteEdge>,
    heap: BinaryHeap<FaceId>,
}

impl EPA {
    /// Creates a new instance of the 3D Expanding Polytope Algorithm.
    ///
    /// This allocates internal data structures (vertices, faces, silhouette buffer, and a priority heap).
    /// The same `EPA` instance can be reused for multiple queries to avoid repeated allocations.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::epa::EPA;
    ///
    /// let mut epa = EPA::new();
    /// // Use epa for multiple queries...
    /// # }
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    fn reset(&mut self) {
        self.vertices.clear();
        self.faces.clear();
        self.heap.clear();
        self.silhouette.clear();
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
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::epa::EPA;
    /// use parry3d::query::gjk::VoronoiSimplex;
    /// use parry3d::shape::Ball;
    /// use parry3d::math::Pose;
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
    /// - Silhouette extraction fails (indicates topology corruption)
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
    /// # #[cfg(all(feature = "dim3", feature = "f32"))] {
    /// use parry3d::query::epa::EPA;
    /// use parry3d::query::gjk::{GJKResult, VoronoiSimplex};
    /// use parry3d::shape::Ball;
    /// use parry3d::math::Pose;
    ///
    /// let ball1 = Ball::new(1.0);
    /// let ball2 = Ball::new(1.0);
    ///
    /// let pos1 = Pose::identity();
    /// let pos2 = Pose::translation(1.5, 0.0, 0.0); // Overlapping
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
    /// EPA iteratively expands a convex polyhedron (in 3D) that surrounds the origin. At each
    /// iteration:
    /// 1. It finds the triangular face closest to the origin
    /// 2. Computes a support point in that face's normal direction
    /// 3. Determines which existing faces are visible from the new point (the **silhouette**)
    /// 4. Removes visible faces and creates new ones connecting the silhouette boundary to the new point
    ///
    /// This process maintains a valid convex hull that progressively tightens around the
    /// origin until convergence, at which point the closest face defines the penetration
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
        let _eps = crate::math::DEFAULT_EPSILON;
        let _eps_tol = _eps * 100.0;

        self.reset();

        /*
         * Initialization.
         */
        for i in 0..simplex.dimension() + 1 {
            self.vertices.push(*simplex.point(i));
        }

        if simplex.dimension() == 0 {
            let mut n: Vector = Vector::ZERO;
            n[1] = 1.0;
            return Some((Vector::ZERO, Vector::ZERO, n));
        } else if simplex.dimension() == 3 {
            let dp1 = self.vertices[1] - self.vertices[0];
            let dp2 = self.vertices[2] - self.vertices[0];
            let dp3 = self.vertices[3] - self.vertices[0];

            if dp1.cross(dp2).dot(dp3) > 0.0 {
                self.vertices.swap(1, 2)
            }

            let pts1 = [0, 1, 2];
            let pts2 = [1, 3, 2];
            let pts3 = [0, 2, 3];
            let pts4 = [0, 3, 1];

            let adj1 = [3, 1, 2];
            let adj2 = [3, 2, 0];
            let adj3 = [0, 1, 3];
            let adj4 = [2, 1, 0];

            let (face1, proj_inside1) = Face::new(&self.vertices, pts1, adj1);
            let (face2, proj_inside2) = Face::new(&self.vertices, pts2, adj2);
            let (face3, proj_inside3) = Face::new(&self.vertices, pts3, adj3);
            let (face4, proj_inside4) = Face::new(&self.vertices, pts4, adj4);

            self.faces.push(face1);
            self.faces.push(face2);
            self.faces.push(face3);
            self.faces.push(face4);

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

            if proj_inside4 {
                let dist4 = self.faces[3].normal.dot(self.vertices[3].point);
                self.heap.push(FaceId::new(3, -dist4)?);
            }

            if !(proj_inside1 || proj_inside2 || proj_inside3 || proj_inside4) {
                // Related issues:
                // https://github.com/dimforge/parry/issues/253
                // https://github.com/dimforge/parry/issues/246
                log::debug!("Hit unexpected state in EPA: failed to project the origin on the initial simplex.");
                return None;
            }
        } else {
            if simplex.dimension() == 1 {
                let dpt = self.vertices[1] - self.vertices[0];

                crate::math::orthonormal_subspace_basis(&[dpt], |dir| {
                    // dir is already normalized by orthonormal_subspace_basis
                    self.vertices
                        .push(CsoPoint::from_shapes(pos12, g1, g2, dir));
                    false
                });
            }

            let pts1 = [0, 1, 2];
            let pts2 = [0, 2, 1];

            let adj1 = [1, 1, 1];
            let adj2 = [0, 0, 0];

            let (face1, _) = Face::new(&self.vertices, pts1, adj1);
            let (face2, _) = Face::new(&self.vertices, pts2, adj2);
            self.faces.push(face1);
            self.faces.push(face2);

            self.heap.push(FaceId::new(0, 0.0)?);
            self.heap.push(FaceId::new(1, 0.0)?);
        }

        let mut niter = 0;
        let mut max_dist = Real::max_value();
        let mut best_face_id = *self.heap.peek()?;
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
                let points = best_face.closest_points(&self.vertices);
                return Some((points.0, points.1, best_face.normal));
            }

            old_dist = curr_dist;

            self.faces[face_id.id].deleted = true;

            let adj_opp_pt_id1 = self.faces[face.adj[0]].next_ccw_pt_id(face.pts[0]);
            let adj_opp_pt_id2 = self.faces[face.adj[1]].next_ccw_pt_id(face.pts[1]);
            let adj_opp_pt_id3 = self.faces[face.adj[2]].next_ccw_pt_id(face.pts[2]);

            self.compute_silhouette(support_point_id, face.adj[0], adj_opp_pt_id1);
            self.compute_silhouette(support_point_id, face.adj[1], adj_opp_pt_id2);
            self.compute_silhouette(support_point_id, face.adj[2], adj_opp_pt_id3);

            let first_new_face_id = self.faces.len();

            if self.silhouette.is_empty() {
                // TODO: Something went very wrong because we failed to extract a silhouette…
                return None;
            }

            for edge in &self.silhouette {
                if !self.faces[edge.face_id].deleted {
                    let new_face_id = self.faces.len();

                    let face_adj = &mut self.faces[edge.face_id];
                    let pt_id1 = face_adj.pts[(edge.opp_pt_id + 2) % 3];
                    let pt_id2 = face_adj.pts[(edge.opp_pt_id + 1) % 3];

                    let pts = [pt_id1, pt_id2, support_point_id];
                    let adj = [edge.face_id, new_face_id + 1, new_face_id - 1];
                    let new_face = Face::new(&self.vertices, pts, adj);

                    face_adj.adj[(edge.opp_pt_id + 1) % 3] = new_face_id;

                    self.faces.push(new_face.0);

                    if new_face.1 {
                        let pt = self.vertices[self.faces[new_face_id].pts[0]].point;
                        let dist = self.faces[new_face_id].normal.dot(pt);
                        if dist < curr_dist {
                            // TODO: if we reach this point, there were issues due to
                            // numerical errors.
                            let points = face.closest_points(&self.vertices);
                            return Some((points.0, points.1, face.normal));
                        }

                        self.heap.push(FaceId::new(new_face_id, -dist)?);
                    }
                }
            }

            if first_new_face_id == self.faces.len() {
                // Something went very wrong because all the edges
                // from the silhouette belonged to deleted faces.
                return None;
            }

            self.faces[first_new_face_id].adj[2] = self.faces.len() - 1;
            self.faces.last_mut().unwrap().adj[1] = first_new_face_id;

            self.silhouette.clear();
            // self.check_topology(); // NOTE: for debugging only.

            niter += 1;
            if niter > 100 {
                // if we reached this point, our algorithm didn't converge to what precision we wanted.
                // still return an intersection point, as it's probably close enough.
                break;
            }
        }

        let best_face = &self.faces[best_face_id.id];
        let points = best_face.closest_points(&self.vertices);
        Some((points.0, points.1, best_face.normal))
    }

    fn compute_silhouette(&mut self, point: usize, id: usize, opp_pt_id: usize) {
        if !self.faces[id].deleted {
            if !self.faces[id].can_be_seen_by(&self.vertices, point, opp_pt_id) {
                self.silhouette.push(SilhouetteEdge::new(id, opp_pt_id));
            } else {
                self.faces[id].deleted = true;

                let adj_pt_id1 = (opp_pt_id + 2) % 3;
                let adj_pt_id2 = opp_pt_id;

                let adj1 = self.faces[id].adj[adj_pt_id1];
                let adj2 = self.faces[id].adj[adj_pt_id2];

                let adj_opp_pt_id1 =
                    self.faces[adj1].next_ccw_pt_id(self.faces[id].pts[adj_pt_id1]);
                let adj_opp_pt_id2 =
                    self.faces[adj2].next_ccw_pt_id(self.faces[id].pts[adj_pt_id2]);

                self.compute_silhouette(point, adj1, adj_opp_pt_id1);
                self.compute_silhouette(point, adj2, adj_opp_pt_id2);
            }
        }
    }

    #[allow(dead_code)]
    #[cfg(feature = "std")]
    fn print_silhouette(&self) {
        use std::{print, println};

        print!("Silhouette points: ");
        for i in 0..self.silhouette.len() {
            let edge = &self.silhouette[i];
            let face = &self.faces[edge.face_id];

            if !face.deleted {
                print!(
                    "({}, {}) ",
                    face.pts[(edge.opp_pt_id + 2) % 3],
                    face.pts[(edge.opp_pt_id + 1) % 3]
                );
            }
        }
        println!();
    }

    #[allow(dead_code)]
    fn check_topology(&self) {
        for i in 0..self.faces.len() {
            let face = &self.faces[i];
            if face.deleted {
                continue;
            }

            // println!("checking {}-th face.", i);
            let adj1 = &self.faces[face.adj[0]];
            let adj2 = &self.faces[face.adj[1]];
            let adj3 = &self.faces[face.adj[2]];

            assert!(!adj1.deleted);
            assert!(!adj2.deleted);
            assert!(!adj3.deleted);

            assert!(face.pts[0] != face.pts[1]);
            assert!(face.pts[0] != face.pts[2]);
            assert!(face.pts[1] != face.pts[2]);

            assert!(adj1.contains_point(face.pts[0]));
            assert!(adj1.contains_point(face.pts[1]));

            assert!(adj2.contains_point(face.pts[1]));
            assert!(adj2.contains_point(face.pts[2]));

            assert!(adj3.contains_point(face.pts[2]));
            assert!(adj3.contains_point(face.pts[0]));

            let opp_pt_id1 = adj1.next_ccw_pt_id(face.pts[0]);
            let opp_pt_id2 = adj2.next_ccw_pt_id(face.pts[1]);
            let opp_pt_id3 = adj3.next_ccw_pt_id(face.pts[2]);

            assert!(!face.contains_point(adj1.pts[opp_pt_id1]));
            assert!(!face.contains_point(adj2.pts[opp_pt_id2]));
            assert!(!face.contains_point(adj3.pts[opp_pt_id3]));

            assert!(adj1.adj[(opp_pt_id1 + 1) % 3] == i);
            assert!(adj2.adj[(opp_pt_id2 + 1) % 3] == i);
            assert!(adj3.adj[(opp_pt_id3 + 1) % 3] == i);
        }
    }
}
