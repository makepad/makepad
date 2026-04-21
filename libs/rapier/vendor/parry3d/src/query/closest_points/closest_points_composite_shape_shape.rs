use crate::bounding_volume::Aabb;
use crate::math::{Pose, Real};
use crate::partitioning::BvhNode;
use crate::query::{ClosestPoints, QueryDispatcher};
use crate::shape::{CompositeShapeRef, Shape, TypedCompositeShape};
use crate::utils::PoseOpt;

impl<S: ?Sized + TypedCompositeShape> CompositeShapeRef<'_, S> {
    /// Returns the closest points between `self` and the given `shape2` positioned at
    /// `pose12` relative to `self`.
    ///
    /// Returns the index of the sub-shape of `self` involved in the contact as well as the closest
    /// points information.
    ///
    /// Returns `ClosestPoints::Disjoint` if `self` and `shape2` are separated by a distance larger
    /// than `margin`.
    ///
    /// Returns `None` if no closest point could be calculated (e.g. if the `dispatcher` doesn’t
    /// support the involved shapes at all, or if `self` is empty).
    pub fn closest_points_to_shape<D: ?Sized + QueryDispatcher>(
        &self,
        dispatcher: &D,
        pose12: &Pose,
        shape2: &dyn Shape,
        margin: Real,
    ) -> Option<(u32, ClosestPoints)> {
        let ls_aabb2 = shape2.compute_aabb(pose12);
        let msum_shift = -ls_aabb2.center();
        let msum_margin = ls_aabb2.half_extents();

        self.0
            .bvh()
            .find_best(
                margin,
                |node: &BvhNode, _| {
                    // Compute the minkowski sum of the two Aabbs.
                    let msum = Aabb {
                        mins: node.mins() + msum_shift - msum_margin,
                        maxs: node.maxs() + msum_shift + msum_margin,
                    };
                    msum.distance_to_origin()
                },
                |part_id, _| {
                    self.0
                        .map_untyped_part_at(part_id, |part_pos1, part_g1, _| {
                            if let Ok(mut pts) = dispatcher.closest_points(
                                &part_pos1.inv_mul(pose12),
                                part_g1,
                                shape2,
                                margin,
                            ) {
                                let cost = match &mut pts {
                                    ClosestPoints::WithinMargin(p1, p2) => {
                                        *p1 = part_pos1.transform_point(*p1);
                                        let p2_1 = pose12 * *p2;
                                        (*p1 - p2_1).length()
                                    }
                                    ClosestPoints::Intersecting => -Real::MAX,
                                    ClosestPoints::Disjoint => Real::MAX,
                                };
                                (cost, pts)
                            } else {
                                (Real::MAX, ClosestPoints::Disjoint)
                            }
                        })
                },
            )
            .map(|(part_id, (_, pts))| (part_id, pts))
    }
}

/// Closest points between a composite shape and any other shape.
pub fn closest_points_composite_shape_shape<D, G1>(
    dispatcher: &D,
    pos12: &Pose,
    g1: &G1,
    g2: &dyn Shape,
    margin: Real,
) -> ClosestPoints
where
    D: ?Sized + QueryDispatcher,
    G1: ?Sized + TypedCompositeShape,
{
    CompositeShapeRef(g1)
        .closest_points_to_shape(dispatcher, pos12, g2, margin)
        .map(|cp| cp.1)
        .unwrap_or(ClosestPoints::Disjoint)
}

/// Closest points between a shape and a composite shape.
pub fn closest_points_shape_composite_shape<D, G2>(
    dispatcher: &D,
    pos12: &Pose,
    g1: &dyn Shape,
    g2: &G2,
    margin: Real,
) -> ClosestPoints
where
    D: ?Sized + QueryDispatcher,
    G2: ?Sized + TypedCompositeShape,
{
    closest_points_composite_shape_shape(dispatcher, &pos12.inverse(), g2, g1, margin).flipped()
}
