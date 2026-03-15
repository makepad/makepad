use crate::math::{Real, Vector};
use crate::query::{PointProjection, PointQuery};
use crate::shape::{FeatureId, HalfSpace};

impl PointQuery for HalfSpace {
    #[inline]
    fn project_local_point(&self, pt: Vector, solid: bool) -> PointProjection {
        let d = self.normal.dot(pt);
        let inside = d <= 0.0;

        if inside && solid {
            PointProjection::new(true, pt)
        } else {
            PointProjection::new(inside, pt + (-self.normal * d))
        }
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, pt: Vector) -> (PointProjection, FeatureId) {
        (self.project_local_point(pt, false), FeatureId::Face(0))
    }

    #[inline]
    fn distance_to_local_point(&self, pt: Vector, solid: bool) -> Real {
        let dist = self.normal.dot(pt);

        if dist < 0.0 && solid {
            0.0
        } else {
            // This will automatically be negative if the point is inside.
            dist
        }
    }

    #[inline]
    fn contains_local_point(&self, pt: Vector) -> bool {
        self.normal.dot(pt) <= 0.0
    }
}
