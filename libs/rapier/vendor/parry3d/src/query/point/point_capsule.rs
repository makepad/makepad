use crate::approx::AbsDiffEq;
use crate::math::{Real, Vector, VectorExt};
use crate::query::{PointProjection, PointQuery};
use crate::shape::{Capsule, FeatureId, Segment};

impl PointQuery for Capsule {
    #[inline]
    fn project_local_point(&self, pt: Vector, solid: bool) -> PointProjection {
        let seg = Segment::new(self.segment.a, self.segment.b);
        let proj = seg.project_local_point(pt, solid);
        let dproj = pt - proj.point;
        let (dir, dist) = dproj.normalize_and_length();
        if dist >= Real::default_epsilon() {
            let inside = dist <= self.radius;
            if solid && inside {
                return PointProjection::new(true, pt);
            } else {
                return PointProjection::new(inside, proj.point + dir * self.radius);
            }
        } else if solid {
            return PointProjection::new(true, pt);
        }

        #[cfg(feature = "dim2")]
        if let Some(dir) = seg.normal() {
            PointProjection::new(true, proj.point + dir * self.radius)
        } else {
            // The segment has no normal, likely because it degenerates to a point.
            PointProjection::new(true, proj.point + Vector::ith(1, self.radius))
        }

        #[cfg(feature = "dim3")]
        if let Some(dir) = seg.direction() {
            use crate::utils::WBasis;
            let dir = dir.orthonormal_basis()[0];
            PointProjection::new(true, proj.point + dir * self.radius)
        } else {
            // The segment has no normal, likely because it degenerates to a point.
            PointProjection::new(true, proj.point + Vector::ith(1, self.radius))
        }
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, pt: Vector) -> (PointProjection, FeatureId) {
        (self.project_local_point(pt, false), FeatureId::Face(0))
    }
}
