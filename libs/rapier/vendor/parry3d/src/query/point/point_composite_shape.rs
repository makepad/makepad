#![allow(unused_parens)] // Needed by the macro.

use crate::math::{Real, Vector};
use crate::partitioning::BvhNode;
use crate::query::{PointProjection, PointQuery, PointQueryWithLocation};
use crate::shape::{
    CompositeShapeRef, FeatureId, SegmentPointLocation, TriMesh, TrianglePointLocation,
    TypedCompositeShape,
};

use crate::shape::{Compound, Polyline};

impl<S: TypedCompositeShape> CompositeShapeRef<'_, S> {
    /// Project a point on this composite shape.
    ///
    /// Returns the projected point as well as the index of the sub-shape of `self` that was hit.
    /// The third tuple element contains some shape-specific information about the projected point.
    #[inline]
    pub fn project_local_point_and_get_location(
        &self,
        point: Vector,
        max_dist: Real,
        solid: bool,
    ) -> Option<(
        u32,
        (
            PointProjection,
            <S::PartShape as PointQueryWithLocation>::Location,
        ),
    )>
    where
        S::PartShape: PointQueryWithLocation,
    {
        self.0
            .bvh()
            .find_best(
                max_dist,
                |node: &BvhNode, _best_so_far| node.aabb().distance_to_local_point(point, true),
                |primitive, _best_so_far| {
                    let proj = self.0.map_typed_part_at(primitive, |pose, shape, _| {
                        if let Some(pose) = pose {
                            shape.project_point_and_get_location(pose, point, solid)
                        } else {
                            shape.project_local_point_and_get_location(point, solid)
                        }
                    })?;
                    let cost = (proj.0.point - point).length();
                    Some((cost, proj))
                },
            )
            .map(|(best_id, (_, (proj, location)))| (best_id, (proj, location)))
    }

    /// Project a point on this composite shape.
    ///
    /// Returns the projected point as well as the index of the sub-shape of `self` that was hit.
    /// If `solid` is `false` then the point will be projected to the closest boundary of `self` even
    /// if it is contained by one of its sub-shapes.
    pub fn project_local_point(&self, point: Vector, solid: bool) -> (u32, PointProjection) {
        let (best_id, (_, proj)) = self
            .0
            .bvh()
            .find_best(
                Real::MAX,
                |node: &BvhNode, _best_so_far| node.aabb().distance_to_local_point(point, true),
                |primitive, _best_so_far| {
                    let proj = self.0.map_typed_part_at(primitive, |pose, shape, _| {
                        if let Some(pose) = pose {
                            shape.project_point(pose, point, solid)
                        } else {
                            shape.project_local_point(point, solid)
                        }
                    })?;
                    let dist = (proj.point - point).length();
                    Some((dist, proj))
                },
            )
            .unwrap();
        (best_id, proj)
    }

    /// Project a point on this composite shape.
    ///
    /// Returns the projected point as well as the index of the sub-shape of `self` that was hit.
    /// The third tuple element contains some shape-specific information about the shape feature
    /// hit by the projection.
    #[inline]
    pub fn project_local_point_and_get_feature(
        &self,
        point: Vector,
    ) -> (u32, (PointProjection, FeatureId)) {
        let (best_id, (_, (proj, feature_id))) = self
            .0
            .bvh()
            .find_best(
                Real::MAX,
                |node: &BvhNode, _best_so_far| node.aabb().distance_to_local_point(point, true),
                |primitive, _best_so_far| {
                    let proj = self.0.map_typed_part_at(primitive, |pose, shape, _| {
                        if let Some(pose) = pose {
                            shape.project_point_and_get_feature(pose, point)
                        } else {
                            shape.project_local_point_and_get_feature(point)
                        }
                    })?;
                    let cost = (proj.0.point - point).length();
                    Some((cost, proj))
                },
            )
            .unwrap();
        (best_id, (proj, feature_id))
    }

    // TODO: implement distance_to_point too?

    /// Returns the index of any sub-shape of `self` that contains the given point.
    #[inline]
    pub fn contains_local_point(&self, point: Vector) -> Option<u32> {
        self.0
            .bvh()
            .leaves(|node: &BvhNode| node.aabb().contains_local_point(point))
            .find(|leaf_id| {
                self.0
                    .map_typed_part_at(*leaf_id, |pose, shape, _| {
                        if let Some(pose) = pose {
                            shape.contains_point(pose, point)
                        } else {
                            shape.contains_local_point(point)
                        }
                    })
                    .unwrap_or(false)
            })
    }
}

impl PointQuery for Polyline {
    #[inline]
    fn project_local_point(&self, point: Vector, solid: bool) -> PointProjection {
        CompositeShapeRef(self).project_local_point(point, solid).1
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, point: Vector) -> (PointProjection, FeatureId) {
        let (seg_id, (proj, feature)) =
            CompositeShapeRef(self).project_local_point_and_get_feature(point);
        let polyline_feature = self.segment_feature_to_polyline_feature(seg_id, feature);
        (proj, polyline_feature)
    }

    // TODO: implement distance_to_point too?

    #[inline]
    fn contains_local_point(&self, point: Vector) -> bool {
        CompositeShapeRef(self)
            .contains_local_point(point)
            .is_some()
    }
}

impl PointQuery for TriMesh {
    #[inline]
    fn project_local_point(&self, point: Vector, solid: bool) -> PointProjection {
        CompositeShapeRef(self).project_local_point(point, solid).1
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, point: Vector) -> (PointProjection, FeatureId) {
        #[cfg(feature = "dim3")]
        if self.pseudo_normals().is_some() {
            // If we can, in 3D, take the pseudo-normals into account.
            let (proj, (id, _feature)) = self.project_local_point_and_get_location(point, false);
            let feature_id = FeatureId::Face(id);
            return (proj, feature_id);
        }

        let solid = cfg!(feature = "dim2");
        let (tri_id, proj) = CompositeShapeRef(self).project_local_point(point, solid);
        (proj, FeatureId::Face(tri_id))
    }

    // TODO: implement distance_to_point too?

    #[inline]
    fn contains_local_point(&self, point: Vector) -> bool {
        #[cfg(feature = "dim3")]
        if self.pseudo_normals.is_some() {
            // If we can, in 3D, take the pseudo-normals into account.
            return self
                .project_local_point_and_get_location(point, true)
                .0
                .is_inside;
        }

        CompositeShapeRef(self)
            .contains_local_point(point)
            .is_some()
    }

    /// Projects a point on `self` transformed by `m`, unless the projection lies further than the given max distance.
    fn project_local_point_with_max_dist(
        &self,
        pt: Vector,
        solid: bool,
        max_dist: Real,
    ) -> Option<PointProjection> {
        self.project_local_point_and_get_location_with_max_dist(pt, solid, max_dist)
            .map(|proj| proj.0)
    }
}

impl PointQuery for Compound {
    #[inline]
    fn project_local_point(&self, point: Vector, solid: bool) -> PointProjection {
        CompositeShapeRef(self).project_local_point(point, solid).1
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, point: Vector) -> (PointProjection, FeatureId) {
        (
            CompositeShapeRef(self)
                .project_local_point_and_get_feature(point)
                .1
                 .0,
            FeatureId::Unknown,
        )
    }

    #[inline]
    fn contains_local_point(&self, point: Vector) -> bool {
        CompositeShapeRef(self)
            .contains_local_point(point)
            .is_some()
    }
}

impl PointQueryWithLocation for Polyline {
    type Location = (u32, SegmentPointLocation);

    #[inline]
    fn project_local_point_and_get_location(
        &self,
        point: Vector,
        solid: bool,
    ) -> (PointProjection, Self::Location) {
        let (seg_id, (proj, loc)) = CompositeShapeRef(self)
            .project_local_point_and_get_location(point, Real::MAX, solid)
            .unwrap();
        (proj, (seg_id, loc))
    }
}

impl PointQueryWithLocation for TriMesh {
    type Location = (u32, TrianglePointLocation);

    #[inline]
    #[allow(unused_mut)] // Because we need mut in 3D but not in 2D.
    fn project_local_point_and_get_location(
        &self,
        point: Vector,
        solid: bool,
    ) -> (PointProjection, Self::Location) {
        self.project_local_point_and_get_location_with_max_dist(point, solid, Real::MAX)
            .unwrap()
    }

    /// Projects a point on `self`, with a maximum projection distance.
    fn project_local_point_and_get_location_with_max_dist(
        &self,
        point: Vector,
        solid: bool,
        max_dist: Real,
    ) -> Option<(PointProjection, Self::Location)> {
        #[allow(unused_mut)] // mut is needed in 3D.
        if let Some((part_id, (mut proj, location))) =
            CompositeShapeRef(self).project_local_point_and_get_location(point, max_dist, solid)
        {
            #[cfg(feature = "dim3")]
            if let Some(pseudo_normals) = self.pseudo_normals_if_oriented() {
                let pseudo_normal = match location {
                    TrianglePointLocation::OnFace(..) | TrianglePointLocation::OnSolid => {
                        Some(self.triangle(part_id).scaled_normal())
                    }
                    TrianglePointLocation::OnEdge(i, _) => pseudo_normals
                        .edges_pseudo_normal
                        .get(part_id as usize)
                        .map(|pn| pn[i as usize]),
                    TrianglePointLocation::OnVertex(i) => {
                        let idx = self.indices()[part_id as usize];
                        pseudo_normals
                            .vertices_pseudo_normal
                            .get(idx[i as usize] as usize)
                            .copied()
                    }
                };

                if let Some(pseudo_normal) = pseudo_normal {
                    let dpt = point - proj.point;
                    proj.is_inside = dpt.dot(pseudo_normal) <= 0.0;
                }
            }

            Some((proj, (part_id, location)))
        } else {
            None
        }
    }
}
