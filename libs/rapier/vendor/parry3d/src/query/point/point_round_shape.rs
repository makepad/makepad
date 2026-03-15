use crate::math::Vector;
#[cfg(feature = "alloc")]
use crate::query::gjk::VoronoiSimplex;
use crate::query::{PointProjection, PointQuery};
use crate::shape::{FeatureId, RoundShape, SupportMap};

// TODO: if PointQuery had a `project_point_with_normal` method, we could just
// call this and adjust the projected point accordingly.
impl<S: SupportMap> PointQuery for RoundShape<S> {
    #[inline]
    #[allow(unreachable_code)]
    #[allow(clippy::diverging_sub_expression)]
    fn project_local_point(&self, _point: Vector, _solid: bool) -> PointProjection {
        #[cfg(not(feature = "alloc"))]
        return unimplemented!(
            "The projection of points on a round shape isn't supported without alloc yet."
        );

        #[cfg(feature = "alloc")]
        return crate::query::details::local_point_projection_on_support_map(
            self,
            &mut VoronoiSimplex::new(),
            _point,
            _solid,
        );
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, point: Vector) -> (PointProjection, FeatureId) {
        (self.project_local_point(point, false), FeatureId::Unknown)
    }
}
