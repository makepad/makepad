use crate::math::{Real, Vector};
use crate::query::gjk::{self, CsoPoint};
use crate::query::{PointQuery, PointQueryWithLocation};
use crate::shape::{Segment, SegmentPointLocation, Triangle, TrianglePointLocation};

/// A simplex of dimension up to 2 using Voronoï regions for computing point projections.
#[derive(Clone, Debug)]
pub struct VoronoiSimplex {
    prev_vertices: [usize; 3],
    prev_dim: usize,
    prev_proj: [Real; 2],

    vertices: [CsoPoint; 3],
    proj: [Real; 2],
    dim: usize,
}

impl Default for VoronoiSimplex {
    fn default() -> Self {
        Self::new()
    }
}

impl VoronoiSimplex {
    /// Crates a new empty simplex.
    pub fn new() -> VoronoiSimplex {
        VoronoiSimplex {
            prev_vertices: [0, 1, 2],
            prev_proj: [0.0; 2],
            prev_dim: 0,
            vertices: [CsoPoint::ZERO; 3],
            proj: [0.0; 2],
            dim: 0,
        }
    }

    /// Swap two vertices of this simplex.
    pub fn swap(&mut self, i1: usize, i2: usize) {
        self.vertices.swap(i1, i2);
        self.prev_vertices.swap(i1, i2);
    }

    /// Resets this simplex to a single point.
    pub fn reset(&mut self, pt: CsoPoint) {
        self.prev_dim = 0;
        self.dim = 0;
        self.vertices[0] = pt;
    }

    /// Add a point to this simplex.
    pub fn add_point(&mut self, pt: CsoPoint) -> bool {
        self.prev_dim = self.dim;
        self.prev_proj = self.proj;
        self.prev_vertices = [0, 1, 2];

        for i in 0..self.dim + 1 {
            if (self.vertices[i].point - pt.point).length_squared() < gjk::eps_tol() {
                return false;
            }
        }

        self.dim += 1;
        self.vertices[self.dim] = pt;
        true
    }

    /// Retrieves the barycentric coordinate associated to the `i`-th by the last call to `project_origin_and_reduce`.
    pub fn proj_coord(&self, i: usize) -> Real {
        assert!(i <= self.dim, "Index out of bounds.");
        self.proj[i]
    }

    /// The i-th point of this simplex.
    pub fn point(&self, i: usize) -> &CsoPoint {
        assert!(i <= self.dim, "Index out of bounds.");
        &self.vertices[i]
    }

    /// Retrieves the barycentric coordinate associated to the `i`-th before the last call to `project_origin_and_reduce`.
    pub fn prev_proj_coord(&self, i: usize) -> Real {
        assert!(i <= self.prev_dim, "Index out of bounds.");
        self.prev_proj[i]
    }

    /// The i-th point of the simplex before the last call to `project_origin_and_reduce`.
    pub fn prev_point(&self, i: usize) -> &CsoPoint {
        assert!(i <= self.prev_dim, "Index out of bounds.");
        &self.vertices[self.prev_vertices[i]]
    }

    /// Projects the origin on the boundary of this simplex and reduces `self` the smallest subsimplex containing the origin.
    ///
    /// Returns the result of the projection or Vector::ZERO if the origin lies inside of the simplex.
    /// The state of the simplex before projection is saved, and can be retrieved using the methods prefixed
    /// by `prev_`.
    pub fn project_origin_and_reduce(&mut self) -> Vector {
        if self.dim == 0 {
            self.proj[0] = 1.0;
            self.vertices[0].point
        } else if self.dim == 1 {
            let (proj, location) = Segment::new(self.vertices[0].point, self.vertices[1].point)
                .project_local_point_and_get_location(Vector::ZERO, true);

            match location {
                SegmentPointLocation::OnVertex(0) => {
                    self.proj[0] = 1.0;
                    self.dim = 0;
                }
                SegmentPointLocation::OnVertex(1) => {
                    self.proj[0] = 1.0;
                    self.swap(0, 1);
                    self.dim = 0;
                }
                SegmentPointLocation::OnEdge(coords) => {
                    self.proj = coords;
                }
                _ => unreachable!(),
            }

            proj.point
        } else {
            assert!(self.dim == 2);
            let (proj, location) = Triangle::new(
                self.vertices[0].point,
                self.vertices[1].point,
                self.vertices[2].point,
            )
            .project_local_point_and_get_location(Vector::ZERO, true);

            match location {
                TrianglePointLocation::OnVertex(i) => {
                    self.swap(0, i as usize);
                    self.proj[0] = 1.0;
                    self.dim = 0;
                }
                TrianglePointLocation::OnEdge(0, coords) => {
                    self.proj = coords;
                    self.dim = 1;
                }
                TrianglePointLocation::OnEdge(1, coords) => {
                    self.swap(0, 2);
                    self.proj[0] = coords[1];
                    self.proj[1] = coords[0];
                    self.dim = 1;
                }
                TrianglePointLocation::OnEdge(2, coords) => {
                    self.swap(1, 2);
                    self.proj = coords;
                    self.dim = 1;
                }
                _ => {}
            }

            proj.point
        }
    }

    /// Compute the projection of the origin on the boundary of this simplex.
    pub fn project_origin(&mut self) -> Vector {
        if self.dim == 0 {
            self.vertices[0].point
        } else if self.dim == 1 {
            let seg = Segment::new(self.vertices[0].point, self.vertices[1].point);
            seg.project_local_point(Vector::ZERO, true).point
        } else {
            assert!(self.dim == 2);
            let tri = Triangle::new(
                self.vertices[0].point,
                self.vertices[1].point,
                self.vertices[2].point,
            );
            tri.project_local_point(Vector::ZERO, true).point
        }
    }

    /// Tests if the given point is already a vertex of this simplex.
    pub fn contains_point(&self, pt: Vector) -> bool {
        for i in 0..self.dim + 1 {
            if self.vertices[i].point == pt {
                return true;
            }
        }

        false
    }

    /// The dimension of the smallest subspace that can contain this simplex.
    pub fn dimension(&self) -> usize {
        self.dim
    }

    /// The dimension of the simplex before the last call to `project_origin_and_reduce`.
    pub fn prev_dimension(&self) -> usize {
        self.prev_dim
    }

    /// The maximum squared length of the vertices of this simplex.
    pub fn max_sq_len(&self) -> Real {
        let mut max_sq_len = 0.0;

        for i in 0..self.dim + 1 {
            let norm = self.vertices[i].point.length_squared();

            if norm > max_sq_len {
                max_sq_len = norm
            }
        }

        max_sq_len
    }

    /// Apply a function to all the vertices of this simplex.
    pub fn modify_pnts(&mut self, f: &dyn Fn(&mut CsoPoint)) {
        for i in 0..self.dim + 1 {
            f(&mut self.vertices[i])
        }
    }
}
