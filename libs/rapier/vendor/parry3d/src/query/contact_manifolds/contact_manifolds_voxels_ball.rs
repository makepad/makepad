use crate::bounding_volume::BoundingVolume;
use crate::math::{Pose, Real, Vector, VectorExt};
use crate::query::{ContactManifold, PointQuery, TrackedContact};
use crate::shape::{
    Ball, Cuboid, OctantPattern, PackedFeatureId, Shape, VoxelState, VoxelType, Voxels,
};
use alloc::vec::Vec;

/// Computes the contact manifold between a convex shape and a ball, both represented as a `Shape` trait-object.
pub fn contact_manifolds_voxels_ball_shapes<ManifoldData, ContactData>(
    pos12: &Pose,
    shape1: &dyn Shape,
    shape2: &dyn Shape,
    prediction: Real,
    manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
) where
    ManifoldData: Default,
    ContactData: Default + Copy,
{
    if let (Some(voxels1), Some(ball2)) = (shape1.as_voxels(), shape2.as_ball()) {
        contact_manifolds_voxels_ball(pos12, voxels1, ball2, prediction, manifolds, false);
    } else if let (Some(ball1), Some(voxels2)) = (shape1.as_ball(), shape2.as_voxels()) {
        contact_manifolds_voxels_ball(
            &pos12.inverse(),
            voxels2,
            ball1,
            prediction,
            manifolds,
            true,
        );
    }
}

/// Computes the contact manifold between a convex shape and a ball.
pub fn contact_manifolds_voxels_ball<'a, ManifoldData, ContactData>(
    pos12: &Pose,
    voxels1: &'a Voxels,
    ball2: &'a Ball,
    prediction: Real,
    manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
    flipped: bool,
) where
    ManifoldData: Default,
    ContactData: Default + Copy,
{
    // TODO: don’t generate one manifold per voxel.
    manifolds.clear();

    let radius2 = ball2.radius;
    let center2 = Vector::ZERO; // The ball’s center.
    let radius1 = voxels1.voxel_size() / 2.0;

    // FIXME: optimize this.
    let aabb1 = voxels1.local_aabb().loosened(prediction / 2.0);
    let aabb2 = ball2.aabb(pos12).loosened(prediction / 2.0);
    if let Some(aabb_intersection) = aabb1.intersection(&aabb2) {
        for vox1 in voxels1.voxels_intersecting_local_aabb(&aabb_intersection) {
            match vox1.state.voxel_type() {
                #[cfg(feature = "dim2")]
                VoxelType::Vertex | VoxelType::Face => { /* Ok */ }
                #[cfg(feature = "dim3")]
                VoxelType::Vertex | VoxelType::Edge | VoxelType::Face => { /* Ok */ }
                _ => continue,
            }

            detect_hit_voxel_ball(
                *pos12,
                vox1.center,
                radius1,
                vox1.state,
                center2,
                radius2,
                prediction,
                flipped,
                manifolds,
            );
        }
    }
}

pub(crate) fn detect_hit_voxel_ball<ManifoldData, ContactData>(
    pos12: Pose,
    center1: Vector,
    radius1: Vector,
    data1: VoxelState,
    center2: Vector,
    radius2: Real,
    prediction: Real,
    flipped: bool,
    manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
) where
    ManifoldData: Default,
    ContactData: Default + Copy,
{
    let center2_1 = pos12 * center2;
    let projection = project_point_on_pseudo_cube(center1, radius1, data1.octant_mask(), center2_1);

    if let Some((local_n1, dist_to_voxel)) = projection {
        let dist = dist_to_voxel - radius2;

        if dist <= prediction {
            let local_n2 = pos12.rotation.inverse() * -local_n1;
            let local_p1 = center2_1 - local_n1 * dist_to_voxel;
            let local_p2 = center2 + local_n2 * radius2;
            let contact_point = TrackedContact::<ContactData>::flipped(
                local_p1,
                local_p2,
                PackedFeatureId::UNKNOWN,
                PackedFeatureId::UNKNOWN,
                dist,
                flipped,
            );

            let mut manifold = ContactManifold::<ManifoldData, ContactData>::new();
            manifold.points.push(contact_point);

            if flipped {
                manifold.local_n1 = local_n2;
                manifold.local_n2 = local_n1;
            } else {
                manifold.local_n1 = local_n1;
                manifold.local_n2 = local_n2;
            }

            manifolds.push(manifold);
        }
    }
}

pub fn project_point_on_pseudo_cube(
    voxel_center: Vector,
    voxel_radius: Vector,
    voxel_mask: u32,
    point: Vector,
) -> Option<(Vector, Real)> {
    let dpos = point - voxel_center;

    // This maps our easy-to map octant_key, to the vertex key as
    // used by the Aabb::FACES_VERTEX_IDS and Aabb::EDGES_VERTEX_IDS
    // arrays. Could we find a way to avoid this map by computing the
    // correct key right away?
    //
    // NOTE: in 2D the array is [0, 1, 3, 2] which matches the one in 3D.
    const AABB_OCTANT_KEYS: [u32; 8] = [0, 1, 3, 2, 4, 5, 7, 6];
    #[cfg(feature = "dim2")]
    let octant_key = ((dpos.x >= 0.0) as usize) | (((dpos.y >= 0.0) as usize) << 1);
    #[cfg(feature = "dim3")]
    let octant_key = ((dpos.x >= 0.0) as usize)
        | (((dpos.y >= 0.0) as usize) << 1)
        | (((dpos.z >= 0.0) as usize) << 2);
    let aabb_octant_key = AABB_OCTANT_KEYS[octant_key];
    let dpos_signs = dpos.map(|x| x.signum());
    let unit_dpos = dpos.abs() / voxel_radius; // Project the point in "local unit octant space".

    // Extract the feature pattern specific to the selected octant.
    let pattern = (voxel_mask >> (aabb_octant_key * 3)) & 0b0111;

    let unit_result = match pattern {
        OctantPattern::INTERIOR => None,
        OctantPattern::VERTEX => {
            // PERF: inline the projection on cuboid to improve performances further.
            //       In particular we already know on what octant we are to compute the
            //       collision.
            let cuboid = Cuboid::new(Vector::splat(1.0));
            let unit_dpos_pt = unit_dpos;
            let proj = cuboid.project_local_point(unit_dpos_pt, false);
            let normal = unit_dpos_pt - proj.point;
            let (normal, dist) = normal.normalize_and_length();
            (dist != 0.0).then_some((normal, dist))
        }
        #[cfg(feature = "dim3")]
        OctantPattern::EDGE_X | OctantPattern::EDGE_Y | OctantPattern::EDGE_Z => {
            let cuboid = Cuboid::new(Vector::splat(1.0));
            let unit_dpos_pt = unit_dpos;
            let proj = cuboid.project_local_point(unit_dpos_pt, false);
            let normal = unit_dpos_pt - proj.point;
            let (normal, dist) = normal.normalize_and_length();
            (dist != 0.0).then_some((normal, dist))
        }
        #[cfg(feature = "dim2")]
        OctantPattern::FACE_X | OctantPattern::FACE_Y => {
            let i1 = pattern as usize - OctantPattern::FACE_X as usize;
            let i2 = (i1 + 1) % 2;

            if unit_dpos[i2] > 1.0 || unit_dpos[i2] < 0.0 {
                None
            } else {
                let dist = unit_dpos[i1] - 1.0; // Subtract 1 to get the distance wrt. the boundary of the unit voxel.
                Some((Vector::ith(i1, 1.0), dist))
            }
        }
        #[cfg(feature = "dim3")]
        OctantPattern::FACE_X | OctantPattern::FACE_Y | OctantPattern::FACE_Z => {
            let i1 = pattern as usize - OctantPattern::FACE_X as usize;
            let i2 = (i1 + 1) % 3;
            let i3 = (i1 + 2) % 3;

            if unit_dpos[i2] > 1.0
                || unit_dpos[i2] < 0.0
                || unit_dpos[i3] > 1.0
                || unit_dpos[i3] < 0.0
            {
                None
            } else {
                let dist = unit_dpos[i1] - 1.0; // Subtract 1 to get the distance wrt. the boundary of the unit voxel.
                Some((Vector::ith(i1, 1.0), dist))
            }
        }
        _ => unreachable!(),
    };

    unit_result.map(|(n, d)| {
        let scaled_n = n * (dpos_signs) * (voxel_radius);
        let (n, scaled_d) = scaled_n.normalize_and_length();
        (n, d * scaled_d)
    })
}
