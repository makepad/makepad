use crate::bounding_volume::BoundingVolume;
use crate::math::Pose;
use crate::partitioning::BvhNode;
use crate::query::QueryDispatcher;
use crate::shape::{CompositeShapeRef, Shape, TypedCompositeShape};
use crate::utils::PoseOpt;

impl<S: ?Sized + TypedCompositeShape> CompositeShapeRef<'_, S> {
    /// Returns the index of the shape in `self` that intersects the given other `shape` positioned
    /// at `pose12` relative to `self`.
    ///
    /// Returns `None` if no intersection is found.
    pub fn intersects_shape<D: ?Sized + QueryDispatcher>(
        &self,
        dispatcher: &D,
        pose12: &Pose,
        shape: &dyn Shape,
    ) -> Option<u32> {
        let ls_aabb2 = shape.compute_aabb(pose12);
        self.0
            .bvh()
            .leaves(|node: &BvhNode| node.aabb().intersects(&ls_aabb2))
            .find(|leaf_id| {
                self.0
                    .map_untyped_part_at(*leaf_id, |part_pose1, sub1, _| {
                        dispatcher.intersection_test(&part_pose1.inv_mul(pose12), sub1, shape)
                            == Ok(true)
                    })
                    .unwrap_or(false)
            })
    }
}

/// Intersection test between a composite shape (`Mesh`, `Compound`) and any other shape.
pub fn intersection_test_composite_shape_shape<D, G1>(
    dispatcher: &D,
    pos12: &Pose,
    g1: &G1,
    g2: &dyn Shape,
) -> bool
where
    D: ?Sized + QueryDispatcher,
    G1: ?Sized + TypedCompositeShape,
{
    CompositeShapeRef(g1)
        .intersects_shape(dispatcher, pos12, g2)
        .is_some()
}

/// Proximity between a shape and a composite (`Mesh`, `Compound`) shape.
pub fn intersection_test_shape_composite_shape<D, G2>(
    dispatcher: &D,
    pos12: &Pose,
    g1: &dyn Shape,
    g2: &G2,
) -> bool
where
    D: ?Sized + QueryDispatcher,
    G2: ?Sized + TypedCompositeShape,
{
    intersection_test_composite_shape_shape(dispatcher, &pos12.inverse(), g2, g1)
}
