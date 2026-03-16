use crate::math::ComplexField;

use crate::math::{Real, Vector};
use crate::query::{PointProjection, PointQuery};
use crate::shape::{Ball, FeatureId};

impl PointQuery for Ball {
    #[inline]
    fn project_local_point(&self, pt: Vector, solid: bool) -> PointProjection {
        let distance_squared = pt.length_squared();

        let inside = distance_squared <= self.radius * self.radius;

        if inside && solid {
            PointProjection::new(true, pt)
        } else {
            let proj = pt * (self.radius / <Real as ComplexField>::sqrt(distance_squared));
            PointProjection::new(inside, proj)
        }
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, pt: Vector) -> (PointProjection, FeatureId) {
        (self.project_local_point(pt, false), FeatureId::Face(0))
    }

    #[inline]
    fn distance_to_local_point(&self, pt: Vector, solid: bool) -> Real {
        let dist = pt.length() - self.radius;

        if solid && dist < 0.0 {
            0.0
        } else {
            dist
        }
    }

    #[inline]
    fn contains_local_point(&self, pt: Vector) -> bool {
        pt.length_squared() <= self.radius * self.radius
    }
}
