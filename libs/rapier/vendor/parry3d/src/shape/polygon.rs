#![allow(dead_code)] // TODO: remove this once we support polygons.

use crate::math::{Pose, Vector, Real, Vector};
use parry::bounding_volume::Aabb;

#[derive(Clone)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize),
)]
/// A convex planar polygon.
pub struct Polygon {
    pub(crate) vertices: Vec<Vector>,
    pub(crate) normals: Vec<Vector>,
}

impl Polygon {
    /// Builds a new polygon from a set of vertices and normals.
    ///
    /// The vertices must be ordered in such a way that two consecutive
    /// vertices determines an edge of the polygon. For example `vertices[0], vertices[1]`
    /// is an edge, `vertices[1], vertices[2]` is the next edge, etc. The last edge will
    /// be `vertices[vertices.len() - 1], vertices[0]`.
    /// The vertices must be given in counter-clockwise order.
    /// The vertices must form a convex polygon.
    ///
    /// One normal must be provided per edge and mut point towards the outside of the polygon.
    pub fn new(vertices: Vec<Vector>, normals: Vec<Vector>) -> Self {
        Self { vertices, normals }
    }

    /// Compute the axis-aligned bounding box of the polygon.
    pub fn aabb(&self, pos: &Pose) -> Aabb {
        let p0 = pos * self.vertices[0];
        let mut mins = p0;
        let mut maxs = p0;

        for pt in &self.vertices[1..] {
            let pt = pos * pt;
            mins = mins.min(pt);
            maxs = maxs.max(pt);
        }

        Aabb::new(mins.into(), maxs.into())
    }

    /// The vertices of this polygon.
    pub fn vertices(&self) -> &[Vector] {
        &self.vertices
    }

    pub(crate) fn support_point(&self, dir: Vector) -> usize {
        let mut best_dot = -Real::MAX;
        let mut best_i = 0;

        for (i, pt) in self.vertices.iter().enumerate() {
            let dot = pt.dot(dir);
            if dot > best_dot {
                best_dot = dot;
                best_i = i;
            }
        }

        best_i
    }

    pub(crate) fn support_face(&self, dir: Vector) -> usize {
        let mut max_dot = -Real::MAX;
        let mut max_dot_i = 0;

        for (i, normal) in self.normals.iter().enumerate() {
            let dot = normal.dot(dir);
            if dot > max_dot {
                max_dot = dot;
                max_dot_i = i;
            }
        }

        max_dot_i
    }
}
