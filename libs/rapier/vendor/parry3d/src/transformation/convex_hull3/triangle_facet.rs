use crate::math::{Real, Vector3};
use crate::shape::Triangle;
use alloc::vec::Vec;
use num::Bounded;

#[derive(Debug)]
pub struct TriangleFacet {
    pub valid: bool,
    pub affinely_dependent: bool,
    pub normal: Vector3,
    pub adj: [usize; 3],
    pub indirect_adj_id: [usize; 3],
    pub pts: [usize; 3],
    pub visible_points: Vec<usize>,
    pub furthest_point: usize,
    pub furthest_distance: Real,
}

impl TriangleFacet {
    pub fn new(p1: usize, p2: usize, p3: usize, points: &[Vector3]) -> TriangleFacet {
        let p1p2 = points[p2] - points[p1];
        let p1p3 = points[p3] - points[p1];

        let normal = p1p2.cross(p1p3).normalize();

        TriangleFacet {
            valid: true,
            affinely_dependent: Triangle::new(points[p1], points[p2], points[p3])
                .is_affinely_dependent(),
            normal,
            adj: [0, 0, 0],
            indirect_adj_id: [0, 0, 0],
            pts: [p1, p2, p3],
            visible_points: Vec::new(),
            furthest_point: Bounded::max_value(),
            furthest_distance: 0.0,
        }
    }

    pub fn add_visible_point(&mut self, pid: usize, points: &[Vector3]) {
        let distance = self.distance_to_point(pid, points);
        assert!(distance > crate::math::DEFAULT_EPSILON);

        if distance > self.furthest_distance {
            self.furthest_distance = distance;
            self.furthest_point = pid;
        }

        self.visible_points.push(pid);
    }

    pub fn distance_to_point(&self, point: usize, points: &[Vector3]) -> Real {
        self.normal.dot(points[point] - points[self.pts[0]])
    }

    pub fn set_facets_adjascency(
        &mut self,
        adj1: usize,
        adj2: usize,
        adj3: usize,
        id_adj1: usize,
        id_adj2: usize,
        id_adj3: usize,
    ) {
        self.indirect_adj_id[0] = id_adj1;
        self.indirect_adj_id[1] = id_adj2;
        self.indirect_adj_id[2] = id_adj3;

        self.adj[0] = adj1;
        self.adj[1] = adj2;
        self.adj[2] = adj3;
    }

    pub fn first_point_from_edge(&self, id: usize) -> usize {
        self.pts[id]
    }

    pub fn second_point_from_edge(&self, id: usize) -> usize {
        self.pts[(id + 1) % 3]
    }

    pub fn can_see_point(&self, point: usize, points: &[Vector3]) -> bool {
        // An affinely-dependent triangle cannot see any point.
        if self.affinely_dependent {
            return false;
        }

        let p0 = points[self.pts[0]];
        let pt = points[point];

        if (pt - p0).dot(self.normal) < crate::math::DEFAULT_EPSILON * 100.0 {
            return false;
        }

        true
    }

    // Check that a given point can see this triangle,
    // making sure that the order of the three indices of
    // this triangle don't affect the test result.
    pub fn order_independent_can_be_seen_by_point(&self, point: usize, points: &[Vector3]) -> bool {
        // An affinely-dependent triangle can be seen by any point.
        if self.affinely_dependent {
            return true;
        }

        for i in 0..3 {
            let p0 = points[self.pts[i]];
            let pt = points[point];

            if (pt - p0).dot(self.normal) >= 0.0 {
                return true;
            }
        }

        false
    }
}
