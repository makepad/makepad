use crate::bounding_volume::BoundingVolume;
use crate::math::{Pose, Real};
use crate::query::{Contact, QueryDispatcher};
use crate::shape::{CompositeShape, CompositeShapeRef, Shape};
use crate::utils::PoseOpt;

impl<S: ?Sized + CompositeShape> CompositeShapeRef<'_, S> {
    /// Returns the closest/deepest contact between `self` and the given `shape2` positioned at
    /// `pose12` relative to `self`.
    ///
    /// Returns `None` if `self` and `shape2` are separated by a distance larger than
    /// `prediction`. Otherwise, returns the index of the sub-shape of `self` involved in the contact
    /// as well as the contact information.
    pub fn contact_with_shape<D: ?Sized + QueryDispatcher>(
        &self,
        dispatcher: &D,
        pose12: &Pose,
        shape2: &dyn Shape,
        prediction: Real,
    ) -> Option<(u32, Contact)> {
        let ls_aabb2 = shape2.compute_aabb(pose12).loosened(prediction);
        let mut result = None::<(u32, Contact)>;

        for part_id in self.0.bvh().intersect_aabb(&ls_aabb2) {
            self.0.map_part_at(part_id, &mut |part_pos1, part1, _| {
                if let Ok(Some(mut c)) =
                    dispatcher.contact(&part_pos1.inv_mul(pose12), part1, shape2, prediction)
                {
                    let replace = result.is_none_or(|(_, cbest)| c.dist < cbest.dist);

                    if replace {
                        if let Some(part_pos1) = part_pos1 {
                            c.transform1_by_mut(part_pos1);
                        }
                        result = Some((part_id, c))
                    }
                }
            });
        }

        result
    }
}

/// Best contact between a composite shape (`Mesh`, `Compound`) and any other shape.
pub fn contact_composite_shape_shape<D, G1>(
    dispatcher: &D,
    pose12: &Pose,
    g1: &G1,
    g2: &dyn Shape,
    prediction: Real,
) -> Option<Contact>
where
    D: ?Sized + QueryDispatcher,
    G1: ?Sized + CompositeShape,
{
    CompositeShapeRef(g1)
        .contact_with_shape(dispatcher, pose12, g2, prediction)
        .map(|c| c.1)
}

/// Best contact between a shape and a composite (`Mesh`, `Compound`) shape.
pub fn contact_shape_composite_shape<D, G2>(
    dispatcher: &D,
    pose12: &Pose,
    g1: &dyn Shape,
    g2: &G2,
    prediction: Real,
) -> Option<Contact>
where
    D: ?Sized + QueryDispatcher,
    G2: ?Sized + CompositeShape,
{
    contact_composite_shape_shape(dispatcher, &pose12.inverse(), g2, g1, prediction)
        .map(|c| c.flipped())
}
