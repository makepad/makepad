use crate::bounding_volume::BoundingVolume;
use crate::math::{Pose, Real, Vector};
use crate::query::contact_manifolds::{CanonicalVoxelShape, VoxelsShapeContactManifoldsWorkspace};
use crate::query::details::VoxelsShapeSubDetector;
use crate::query::{
    ContactManifold, ContactManifoldsWorkspace, PersistentQueryDispatcher, PointQuery,
    TypedWorkspaceData, WorkspaceData,
};
use crate::shape::{Cuboid, Shape, SupportMap, VoxelData, VoxelType, Voxels};
use crate::utils::hashmap::Entry;
use crate::utils::PoseOpt;
use alloc::{boxed::Box, vec::Vec};

impl WorkspaceData for VoxelsShapeContactManifoldsWorkspace<4> {
    fn as_typed_workspace_data(&self) -> TypedWorkspaceData<'_> {
        TypedWorkspaceData::VoxelsVoxelsContactManifoldsWorkspace(self)
    }

    fn clone_dyn(&self) -> Box<dyn WorkspaceData> {
        Box::new(self.clone())
    }
}

/// Computes the contact manifold between a convex shape and a ball, both represented as a `Shape` trait-object.
pub fn contact_manifolds_voxels_voxels_shapes<ManifoldData, ContactData>(
    dispatcher: &dyn PersistentQueryDispatcher<ManifoldData, ContactData>,
    pos12: &Pose,
    shape1: &dyn Shape,
    shape2: &dyn Shape,
    prediction: Real,
    manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
    workspace: &mut Option<ContactManifoldsWorkspace>,
) where
    ManifoldData: Default + Clone,
    ContactData: Default + Copy,
{
    if let (Some(voxels1), Some(voxels2)) = (shape1.as_voxels(), shape2.as_voxels()) {
        contact_manifolds_voxels_voxels(
            dispatcher, pos12, voxels1, voxels2, prediction, manifolds, workspace,
        );
    }
}

/// Computes the contact manifold between a convex shape and a ball.
pub fn contact_manifolds_voxels_voxels<'a, ManifoldData, ContactData>(
    dispatcher: &dyn PersistentQueryDispatcher<ManifoldData, ContactData>,
    pos12: &Pose,
    voxels1: &'a Voxels,
    voxels2: &'a Voxels,
    prediction: Real,
    manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
    workspace: &mut Option<ContactManifoldsWorkspace>,
) where
    ManifoldData: Default + Clone,
    ContactData: Default + Copy,
{
    VoxelsShapeContactManifoldsWorkspace::<4>::ensure_exists(workspace);
    let workspace: &mut VoxelsShapeContactManifoldsWorkspace<4> =
        workspace.as_mut().unwrap().0.downcast_mut().unwrap();
    let new_timestamp = !workspace.timestamp;
    workspace.timestamp = new_timestamp;

    // TODO: avoid reallocating the new `manifolds` vec at each step.
    let mut old_manifolds = core::mem::take(manifolds);

    let radius1 = voxels1.voxel_size() / 2.0;
    let radius2 = voxels2.voxel_size() / 2.0;

    let aabb1 = voxels1.local_aabb().loosened(prediction / 2.0);
    let aabb2 = voxels2.local_aabb().loosened(prediction / 2.0);

    let pos21 = pos12.inverse();

    let mut checked_hits = 0;

    if let Some((intersection_aabb1, intersection_aabb2)) =
        aabb1.aligned_intersections(pos12, &aabb2)
    {
        let domain_margin = (radius1 + radius2) * 10.0;
        let full_domain2_1 = voxels2.compute_aabb(pos12).add_half_extents(domain_margin);
        let domain2_1 = full_domain2_1
            .intersection(&aabb1.add_half_extents(domain_margin))
            .unwrap_or(full_domain2_1);
        let full_domain1_2 = voxels1.compute_aabb(&pos21).add_half_extents(domain_margin);
        let domain1_2 = full_domain1_2
            .intersection(&aabb2.add_half_extents(domain_margin))
            .unwrap_or(full_domain1_2);

        let mut detect_hit = |canon1: CanonicalVoxelShape,
                              canon2: CanonicalVoxelShape,
                              vox1: &VoxelData,
                              vox2: &VoxelData| {
            // Compute canonical shapes and dispatch.
            let workspace_key = [
                canon1.workspace_key[0],
                canon1.workspace_key[1],
                canon2.workspace_key[0],
                canon2.workspace_key[1],
            ];

            checked_hits += 1;

            // TODO: could we refactor the workspace system with the Voxels-Shape
            //       collision detection system?
            let (sub_detector, manifold_updated) =
                match workspace.sub_detectors.entry(workspace_key.into()) {
                    Entry::Occupied(entry) => {
                        let sub_detector = entry.into_mut();

                        if sub_detector.timestamp != new_timestamp {
                            let manifold = old_manifolds[sub_detector.manifold_id].take();
                            *sub_detector = VoxelsShapeSubDetector {
                                manifold_id: manifolds.len(),
                                timestamp: new_timestamp,
                                selected_contacts: 0,
                            };

                            manifolds.push(manifold);
                            (sub_detector, false)
                        } else {
                            (sub_detector, true)
                        }
                    }
                    Entry::Vacant(entry) => {
                        let sub_detector = VoxelsShapeSubDetector {
                            manifold_id: manifolds.len(),
                            selected_contacts: 0,
                            timestamp: new_timestamp,
                        };

                        manifolds.push(ContactManifold::with_data(
                            vox1.linear_id.flat_id() as u32,
                            vox2.linear_id.flat_id() as u32,
                            ManifoldData::default(),
                        ));

                        (entry.insert(sub_detector), false)
                    }
                };

            /*
             * Update the contact manifold if needed.
             */
            let manifold = &mut manifolds[sub_detector.manifold_id];
            let (canonical_center1, canonical_pseudo_cube1) =
                canon1.cuboid(voxels1, vox1, domain2_1);
            let (canonical_center2, canonical_pseudo_cube2) =
                canon2.cuboid(voxels2, vox2, domain1_2);

            if !manifold_updated {
                let canonical_pos12 = Pose::from_translation(-canonical_center1)
                    * pos12
                    * Pose::from_translation(canonical_center2);

                // If we already computed contacts in the previous simulation step, their
                // local points are relative to the previously calculated canonical shape
                // which might not have the same local center as the one computed in this
                // step (because it’s based on the position of shape2 relative to voxels1).
                // So we need to adjust the local points to account for the position difference
                // and keep the point at the same "canonical-shape-space" location as in the previous frame.
                let prev_center1 = manifold
                    .subshape_pos1
                    .as_ref()
                    .map(|p| p.translation)
                    .unwrap_or_default();
                let delta_center1 = canonical_center1 - prev_center1;
                let prev_center2 = manifold
                    .subshape_pos2
                    .as_ref()
                    .map(|p| p.translation)
                    .unwrap_or_default();
                let delta_center2 = canonical_center2 - prev_center2;

                for pt in &mut manifold.points {
                    pt.local_p1 -= delta_center1;
                    pt.local_p2 -= delta_center2;
                }

                // Update contacts.
                manifold.subshape_pos1 = Some(Pose::from_translation(canonical_center1));
                manifold.subshape_pos2 = Some(Pose::from_translation(canonical_center2));
                let _ = dispatcher.contact_manifold_convex_convex(
                    &canonical_pos12,
                    &canonical_pseudo_cube1,
                    &canonical_pseudo_cube2,
                    None,
                    None,
                    prediction,
                    manifold,
                );
            }

            /*
             * Filter-out points that don’t belong to this block.
             */
            let test_voxel1 = Cuboid::new(radius1 + Vector::splat(1.0e-2));
            let test_voxel2 = Cuboid::new(radius2 + Vector::splat(1.0e-2));
            let penetration_dir1 = manifold.local_n1;

            for (i, pt) in manifold.points.iter().enumerate() {
                if pt.dist < 0.0 {
                    // If this is a penetration, double-check that we are not hitting the
                    // interior of the infinitely expanded canonical shape by checking if
                    // the opposite normal had led to a better vector.
                    let cuboid1 = Cuboid::new(radius1);
                    let cuboid2 = Cuboid::new(radius2);
                    let sp1 = cuboid1.local_support_point(-penetration_dir1) + vox1.center;
                    let sp2 = cuboid2.support_point(
                        &(pos12 * Pose::from_translation(vox2.center)),
                        penetration_dir1,
                    );
                    let test_dist = (sp2 - sp1).dot(-penetration_dir1);
                    let keep = test_dist < pt.dist;

                    if !keep {
                        // We don’t want to keep this point as it is an incorrect penetration
                        // caused by the canonical shape expansion.
                        continue;
                    }
                }

                let pt_in_voxel_space1 =
                    manifold.subshape_pos1.transform_point(pt.local_p1) - vox1.center;
                let pt_in_voxel_space2 =
                    manifold.subshape_pos2.transform_point(pt.local_p2) - vox2.center;
                sub_detector.selected_contacts |=
                    ((test_voxel1.contains_local_point(pt_in_voxel_space1) as u32) << i)
                        & ((test_voxel2.contains_local_point(pt_in_voxel_space2) as u32) << i);
            }
        };

        for vox1 in voxels1.voxels_intersecting_local_aabb(&intersection_aabb1) {
            let type1 = vox1.state.voxel_type();
            match type1 {
                #[cfg(feature = "dim2")]
                VoxelType::Vertex => { /* Ok */ }
                #[cfg(feature = "dim3")]
                VoxelType::Vertex | VoxelType::Edge => { /* Ok */ }
                _ => continue,
            }

            let canon1 = CanonicalVoxelShape::from_voxel(voxels1, &vox1);
            let centered_aabb1_2 = Cuboid::new(radius1 + Vector::splat(prediction))
                .compute_aabb(&(pos21 * Pose::from_translation(vox1.center)));

            for vox2 in voxels2.voxels_intersecting_local_aabb(&centered_aabb1_2) {
                let type2 = vox2.state.voxel_type();

                #[cfg(feature = "dim2")]
                match (type1, type2) {
                    (VoxelType::Vertex, VoxelType::Vertex)
                    | (VoxelType::Vertex, VoxelType::Face)
                    | (VoxelType::Face, VoxelType::Vertex) => { /* OK */ }
                    _ => continue, /* Ignore */
                }

                #[cfg(feature = "dim3")]
                match (type1, type2) {
                    (VoxelType::Vertex, VoxelType::Vertex)
                    | (VoxelType::Vertex, VoxelType::Edge)
                    | (VoxelType::Vertex, VoxelType::Face)
                    | (VoxelType::Edge, VoxelType::Vertex)
                    | (VoxelType::Edge, VoxelType::Edge)
                    | (VoxelType::Face, VoxelType::Vertex) => { /* OK */ }
                    _ => continue, /* Ignore */
                }

                let canon2 = CanonicalVoxelShape::from_voxel(voxels2, &vox2);
                detect_hit(canon1, canon2, &vox1, &vox2);
            }
        }

        for vox2 in voxels2.voxels_intersecting_local_aabb(&intersection_aabb2) {
            let type2 = vox2.state.voxel_type();
            match type2 {
                #[cfg(feature = "dim2")]
                VoxelType::Vertex => { /* Ok */ }
                #[cfg(feature = "dim3")]
                VoxelType::Vertex | VoxelType::Edge => { /* Ok */ }
                _ => continue,
            }

            let canon2 = CanonicalVoxelShape::from_voxel(voxels2, &vox2);
            let centered_aabb2_1 = Cuboid::new(radius2 + Vector::splat(prediction))
                .compute_aabb(&(pos12 * Pose::from_translation(vox2.center)));

            for vox1 in voxels1.voxels_intersecting_local_aabb(&centered_aabb2_1) {
                let type1 = vox1.state.voxel_type();

                #[cfg(feature = "dim2")]
                match (type1, type2) {
                    (VoxelType::Vertex, VoxelType::Vertex)
                    | (VoxelType::Vertex, VoxelType::Face)
                    | (VoxelType::Face, VoxelType::Vertex) => { /* OK */ }
                    _ => continue, /* Ignore */
                }

                #[cfg(feature = "dim3")]
                match (type1, type2) {
                    (VoxelType::Vertex, VoxelType::Vertex)
                    | (VoxelType::Vertex, VoxelType::Edge)
                    | (VoxelType::Vertex, VoxelType::Face)
                    | (VoxelType::Edge, VoxelType::Vertex)
                    | (VoxelType::Edge, VoxelType::Edge)
                    | (VoxelType::Face, VoxelType::Vertex) => { /* OK */ }
                    _ => continue, /* Ignore */
                }

                let canon1 = CanonicalVoxelShape::from_voxel(voxels1, &vox1);
                detect_hit(canon1, canon2, &vox1, &vox2);
            }
        }

        // Remove contacts marked as ignored.
        for sub_detector in workspace.sub_detectors.values() {
            if sub_detector.timestamp == new_timestamp {
                let manifold = &mut manifolds[sub_detector.manifold_id];
                let mut k = 0;
                manifold.points.retain(|_| {
                    let keep = (sub_detector.selected_contacts & (1 << k)) != 0;
                    k += 1;
                    keep
                });
            }
        }
    }

    // Remove detectors which no longer index a valid manifold.
    workspace
        .sub_detectors
        .retain(|_, detector| detector.timestamp == new_timestamp);
}
