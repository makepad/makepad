use crate::bounding_volume::Aabb;
use crate::math::{Pose, Real};
use crate::partitioning::BvhNode;
use crate::query::QueryDispatcher;
use crate::shape::{CompositeShapeRef, Shape, TypedCompositeShape};
use crate::utils::PoseOpt;

impl<S: ?Sized + TypedCompositeShape> CompositeShapeRef<'_, S> {
    /// Calculates the closest distance between `self` and the given `shape2` positioned at
    /// `pose12` relative to `self`.
    ///
    /// Returns the distance and the index of the sub-shape of `self` that is closest to `shape2`.
    pub fn distance_to_shape<D: ?Sized + QueryDispatcher>(
        &self,
        dispatcher: &D,
        pose12: &Pose,
        shape2: &dyn Shape,
    ) -> Option<(u32, Real)> {
        let ls_aabb2 = shape2.compute_aabb(pose12);
        let msum_shift = -ls_aabb2.center();
        let msum_margin = ls_aabb2.half_extents();

        self.0.bvh().find_best(
            Real::MAX,
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
                        dispatcher.distance(&part_pos1.inv_mul(pose12), part_g1, shape2)
                    })?
                    .ok()
            },
        )
    }
}

/// Smallest distance between a composite shape and any other shape.
pub fn distance_composite_shape_shape<D, G1>(
    dispatcher: &D,
    pos12: &Pose,
    g1: &G1,
    g2: &dyn Shape,
) -> Real
where
    D: ?Sized + QueryDispatcher,
    G1: ?Sized + TypedCompositeShape,
{
    CompositeShapeRef(g1)
        .distance_to_shape(dispatcher, pos12, g2)
        .unwrap_or((u32::MAX, Real::MAX))
        .1
}

/// Smallest distance between a shape and a composite shape.
pub fn distance_shape_composite_shape<D, G2>(
    dispatcher: &D,
    pos12: &Pose,
    g1: &dyn Shape,
    g2: &G2,
) -> Real
where
    D: ?Sized + QueryDispatcher,
    G2: ?Sized + TypedCompositeShape,
{
    distance_composite_shape_shape(dispatcher, &pos12.inverse(), g2, g1)
}
