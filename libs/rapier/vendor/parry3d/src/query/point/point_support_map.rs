use crate::math::{Pose, Vector};
#[cfg(feature = "alloc")]
use crate::query::epa::EPA;
use crate::query::gjk::{self, ConstantOrigin, CsoPoint, VoronoiSimplex};
use crate::query::{PointProjection, PointQuery};
#[cfg(feature = "dim2")]
#[cfg(feature = "alloc")]
use crate::shape::ConvexPolygon;
#[cfg(feature = "dim3")]
#[cfg(feature = "alloc")]
use crate::shape::ConvexPolyhedron;
use crate::shape::{FeatureId, SupportMap};

/// Projects a point on a shape using the GJK algorithm.
pub fn local_point_projection_on_support_map<G>(
    shape: &G,
    simplex: &mut VoronoiSimplex,
    point: Vector,
    solid: bool,
) -> PointProjection
where
    G: SupportMap,
{
    #[cfg(feature = "dim2")]
    let m = Pose::new(-point, 0.0);
    #[cfg(feature = "dim3")]
    let m = Pose::new(-point, Vector::ZERO);
    #[cfg(feature = "dim2")]
    let m_inv = Pose::new(point, 0.0);
    #[cfg(feature = "dim3")]
    let m_inv = Pose::new(point, Vector::ZERO);
    let dir = (-m.translation).try_normalize().unwrap_or(Vector::X);
    let support_point = CsoPoint::from_shapes(&m_inv, shape, &ConstantOrigin, dir);

    simplex.reset(support_point);

    if let Some(proj) = gjk::project_origin(&m, shape, simplex) {
        PointProjection::new(false, proj)
    } else if solid {
        PointProjection::new(true, point)
    } else {
        let mut epa = EPA::new();
        if let Some(pt) = epa.project_origin(&m, shape, simplex) {
            PointProjection::new(true, pt)
        } else {
            // return match minkowski_sampling::project_origin(&m, shape, simplex) {
            //     Some(p) => PointProjection::new(true, p + point),
            //     None => PointProjection::new(true, *point),
            // };

            //// All failed.
            PointProjection::new(true, point)
        }
    }
}

#[cfg(feature = "dim3")]
impl PointQuery for ConvexPolyhedron {
    #[inline]
    fn project_local_point(&self, point: Vector, solid: bool) -> PointProjection {
        local_point_projection_on_support_map(self, &mut VoronoiSimplex::new(), point, solid)
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, point: Vector) -> (PointProjection, FeatureId) {
        let proj = self.project_local_point(point, false);
        let dpt = point - proj.point;
        let local_dir = if proj.is_inside { -dpt } else { dpt };

        if let Some(local_dir) = (local_dir).try_normalize() {
            let feature = self.support_feature_id_toward(local_dir);
            (proj, feature)
        } else {
            (proj, FeatureId::Unknown)
        }
    }
}

#[cfg(feature = "dim2")]
impl PointQuery for ConvexPolygon {
    #[inline]
    fn project_local_point(&self, point: Vector, solid: bool) -> PointProjection {
        local_point_projection_on_support_map(self, &mut VoronoiSimplex::new(), point, solid)
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, point: Vector) -> (PointProjection, FeatureId) {
        let proj = self.project_local_point(point, false);
        let dpt = point - proj.point;
        let local_dir = if proj.is_inside { -dpt } else { dpt };

        if let Some(local_dir) = (local_dir).try_normalize() {
            let feature = self.support_feature_id_toward(local_dir);
            (proj, feature)
        } else {
            (proj, FeatureId::Unknown)
        }
    }
}
