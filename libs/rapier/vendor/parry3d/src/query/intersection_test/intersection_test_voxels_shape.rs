use crate::math::Pose;
use crate::query::PersistentQueryDispatcher;
use crate::shape::{Cuboid, Shape, VoxelType, Voxels};

/// Checks for any intersection between voxels and an arbitrary shape, both represented as a `Shape` trait-object.
pub fn intersection_test_voxels_shape_shapes(
    dispatcher: &dyn PersistentQueryDispatcher,
    pos12: &Pose,
    shape1: &dyn Shape,
    shape2: &dyn Shape,
) -> bool {
    if let Some(voxels1) = shape1.as_voxels() {
        intersection_test_voxels_shape(dispatcher, pos12, voxels1, shape2)
    } else if let Some(voxels2) = shape2.as_voxels() {
        intersection_test_voxels_shape(dispatcher, &pos12.inverse(), voxels2, shape1)
    } else {
        false
    }
}

/// Checks for any intersection between voxels and an arbitrary shape.
pub fn intersection_test_voxels_shape(
    dispatcher: &dyn PersistentQueryDispatcher,
    pos12: &Pose,
    voxels1: &Voxels,
    shape2: &dyn Shape,
) -> bool {
    let radius1 = voxels1.voxel_size() / 2.0;
    let aabb1 = voxels1.local_aabb();
    let aabb2_1 = shape2.compute_aabb(pos12);

    if let Some(intersection_aabb1) = aabb1.intersection(&aabb2_1) {
        for vox1 in voxels1.voxels_intersecting_local_aabb(&intersection_aabb1) {
            let vox_type1 = vox1.state.voxel_type();

            if vox_type1 == VoxelType::Empty {
                continue;
            }

            let center1 = vox1.center;
            let cuboid1 = Cuboid::new(radius1);
            let cuboid_pose12 = Pose::from_translation(-center1) * pos12;

            if dispatcher
                .intersection_test(&cuboid_pose12, &cuboid1, shape2)
                .unwrap_or(false)
            {
                return true;
            }
        }
    }

    false
}

/// Checks for any intersection between voxels and an arbitrary shape.
pub fn intersection_test_shape_voxels(
    dispatcher: &dyn PersistentQueryDispatcher,
    pos12: &Pose,
    shape1: &dyn Shape,
    voxels2: &Voxels,
) -> bool {
    intersection_test_voxels_shape(dispatcher, &pos12.inverse(), voxels2, shape1)
}
