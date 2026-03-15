use crate::bounding_volume::Aabb;
use crate::math::{Pose, Real, Vector};
use crate::partitioning::BvhNode;
use crate::query::shape_cast::ShapeCastOptions;
use crate::query::{QueryDispatcher, Ray, RayCast, ShapeCastHit};
use crate::shape::{CompositeShapeRef, Shape, TypedCompositeShape};

impl<S: ?Sized + TypedCompositeShape> CompositeShapeRef<'_, S> {
    /// Performs a shape-cast between `self` and a `shape2` positioned at `pose12` and subject to
    /// a linear velocity `vel12`, relative to `self`.
    ///
    /// Returns the shape-cast hit (if any) as well as the index of the sub-shape of `self` involved
    /// in the hit.
    pub fn cast_shape<D: ?Sized + QueryDispatcher>(
        &self,
        dispatcher: &D,
        pose12: &Pose,
        vel12: Vector,
        g2: &dyn Shape,
        options: ShapeCastOptions,
    ) -> Option<(u32, ShapeCastHit)> {
        let ls_aabb2 = g2.compute_aabb(pose12);
        let ray = Ray::new(Vector::ZERO, vel12);
        let msum_shift = -ls_aabb2.center();
        let msum_margin = ls_aabb2.half_extents() + Vector::splat(options.target_distance);

        self.0.bvh().find_best(
            options.max_time_of_impact,
            |node: &BvhNode, best_so_far| {
                // Compute the minkowski sum of the two Aabbs.
                let msum = Aabb {
                    mins: node.mins() + msum_shift - msum_margin,
                    maxs: node.maxs() + msum_shift + msum_margin,
                };

                // Compute the time of impact.
                msum.cast_local_ray(&ray, best_so_far, true)
                    .unwrap_or(Real::MAX)
            },
            |part_id, _| {
                self.0
                    .map_untyped_part_at(part_id, |part_pose1, part_g1, _| {
                        if let Some(part_pose1) = part_pose1 {
                            dispatcher
                                .cast_shapes(
                                    &part_pose1.inv_mul(pose12),
                                    part_pose1.rotation.inverse() * vel12,
                                    part_g1,
                                    g2,
                                    options,
                                )
                                .ok()?
                                .map(|hit| hit.transform1_by(part_pose1))
                        } else {
                            dispatcher
                                .cast_shapes(pose12, vel12, part_g1, g2, options)
                                .ok()?
                        }
                    })?
            },
        )
    }
}

/// Time Of Impact of a composite shape with any other shape, under translational movement.
pub fn cast_shapes_composite_shape_shape<D, G1>(
    dispatcher: &D,
    pos12: &Pose,
    vel12: Vector,
    g1: &G1,
    g2: &dyn Shape,
    options: ShapeCastOptions,
) -> Option<ShapeCastHit>
where
    D: ?Sized + QueryDispatcher,
    G1: ?Sized + TypedCompositeShape,
{
    CompositeShapeRef(g1)
        .cast_shape(dispatcher, pos12, vel12, g2, options)
        .map(|hit| hit.1)
}

/// Time Of Impact of any shape with a composite shape, under translational movement.
pub fn cast_shapes_shape_composite_shape<D, G2>(
    dispatcher: &D,
    pos12: &Pose,
    vel12: Vector,
    g1: &dyn Shape,
    g2: &G2,
    options: ShapeCastOptions,
) -> Option<ShapeCastHit>
where
    D: ?Sized + QueryDispatcher,
    G2: ?Sized + TypedCompositeShape,
{
    cast_shapes_composite_shape_shape(
        dispatcher,
        &pos12.inverse(),
        -(pos12.rotation.inverse() * vel12),
        g2,
        g1,
        options,
    )
    .map(|time_of_impact| time_of_impact.swapped())
}
