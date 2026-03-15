use crate::bounding_volume::Aabb;
use crate::math::{Pose, Real, Vector};
use crate::query::details::{
    CanonicalVoxelShape, VoxelsShapeContactManifoldsWorkspace, VoxelsShapeSubDetector,
};
use crate::query::{
    ContactManifold, ContactManifoldsWorkspace, PersistentQueryDispatcher, PointQuery,
    TypedWorkspaceData, WorkspaceData,
};
use crate::shape::{CompositeShape, Cuboid, Shape, SupportMap, VoxelType, Voxels};
use crate::utils::hashmap::Entry;
use crate::utils::PoseOpt;
use alloc::{boxed::Box, vec::Vec};

impl WorkspaceData for VoxelsShapeContactManifoldsWorkspace<3> {
    fn as_typed_workspace_data(&self) -> TypedWorkspaceData<'_> {
        TypedWorkspaceData::VoxelsCompositeShapeContactManifoldsWorkspace(self)
    }

    fn clone_dyn(&self) -> Box<dyn WorkspaceData> {
        Box::new(self.clone())
    }
}

/// Computes the contact manifold between voxels and a composite shape, both represented as a `Shape` trait-object.
pub fn contact_manifolds_voxels_composite_shape_shapes<ManifoldData, ContactData>(
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
    if let (Some(voxels1), Some(shape2)) = (shape1.as_voxels(), shape2.as_composite_shape()) {
        contact_manifolds_voxels_composite_shape(
            dispatcher, pos12, voxels1, shape2, prediction, manifolds, workspace, false,
        );
    } else if let (Some(shape1), Some(voxels2)) = (shape1.as_composite_shape(), shape2.as_voxels())
    {
        contact_manifolds_voxels_composite_shape(
            dispatcher,
            &pos12.inverse(),
            voxels2,
            shape1,
            prediction,
            manifolds,
            workspace,
            true,
        );
    }
}

/// Computes the contact manifold between voxels and a composite shape.
pub fn contact_manifolds_voxels_composite_shape<ManifoldData, ContactData>(
    dispatcher: &dyn PersistentQueryDispatcher<ManifoldData, ContactData>,
    pos12: &Pose,
    voxels1: &Voxels,
    shape2: &dyn CompositeShape,
    prediction: Real,
    manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
    workspace: &mut Option<ContactManifoldsWorkspace>,
    flipped: bool,
) where
    ManifoldData: Default + Clone,
    ContactData: Default + Copy,
{
    VoxelsShapeContactManifoldsWorkspace::<3>::ensure_exists(workspace);
    let workspace: &mut VoxelsShapeContactManifoldsWorkspace<3> =
        workspace.as_mut().unwrap().0.downcast_mut().unwrap();
    let new_timestamp = !workspace.timestamp;
    workspace.timestamp = new_timestamp;

    // TODO: avoid reallocating the new `manifolds` vec at each step.
    let mut old_manifolds = core::mem::take(manifolds);
    let bvh2 = shape2.bvh();

    let radius1 = voxels1.voxel_size() / 2.0;

    let aabb1 = voxels1.local_aabb();
    let aabb2_1 = shape2.bvh().root_aabb().transform_by(pos12);
    let domain2_1 = Aabb {
        mins: aabb2_1.mins - radius1 * 10.0,
        maxs: aabb2_1.maxs + radius1 * 10.0,
    };

    if let Some(intersection_aabb1) = aabb1.intersection(&aabb2_1) {
        for vox1 in voxels1.voxels_intersecting_local_aabb(&intersection_aabb1) {
            let vox_type1 = vox1.state.voxel_type();

            // TODO: would be nice to have a strategy to handle interior voxels for depenetration.
            if vox_type1 == VoxelType::Empty || vox_type1 == VoxelType::Interior {
                continue;
            }

            // PERF: could we avoid repeated BVH traversals involving the same canonical shape?
            //       The issue is that we need to figure out what contact manifolds are associated
            //       to that canonical shape so we can update the included contact bitmask (and
            //       one way of figuring this out is to re-traverse the bvh).
            let canon1 = CanonicalVoxelShape::from_voxel(voxels1, &vox1);
            let (canonical_center1, canonical_shape1) = canon1.cuboid(voxels1, &vox1, domain2_1);
            let canonical_pose12 = Pose::from_translation(-canonical_center1) * pos12;

            let mut detect_hit = |leaf2: u32| {
                let key = [canon1.workspace_key[0], canon1.workspace_key[1], leaf2];

                // TODO: could we refactor the workspace system between Voxels, HeightField, and CompoundShape?
                //       (and maybe TriMesh too but it's using a different approach).
                let (sub_detector, manifold_updated) =
                    match workspace.sub_detectors.entry(key.into()) {
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

                            let vox_id = vox1.linear_id.flat_id() as u32;
                            let (id1, id2) = if flipped {
                                (leaf2, vox_id)
                            } else {
                                (vox_id, leaf2)
                            };
                            manifolds.push(ContactManifold::with_data(
                                id1,
                                id2,
                                ManifoldData::default(),
                            ));

                            (entry.insert(sub_detector), false)
                        }
                    };

                /*
                 * Update the contact manifold if needed.
                 */
                let manifold = &mut manifolds[sub_detector.manifold_id];

                shape2.map_part_at(leaf2, &mut |part_pos2, part_shape2, _| {
                    let relative_pos12 = part_pos2.prepend_to(&canonical_pose12);

                    if !manifold_updated {
                        // If we already computed contacts in the previous simulation step, their
                        // local points are relative to the previously calculated canonical shape
                        // which might not have the same local center as the one computed in this
                        // step (because it’s based on the position of shape2 relative to voxels1).
                        // So we need to adjust the local points to account for the position difference
                        // and keep the point at the same "canonical-shape-space" location as in the previous frame.
                        let prev_center = if flipped {
                            manifold
                                .subshape_pos2
                                .as_ref()
                                .map(|p| p.translation)
                                .unwrap_or_default()
                        } else {
                            manifold
                                .subshape_pos1
                                .as_ref()
                                .map(|p| p.translation)
                                .unwrap_or_default()
                        };
                        let delta_center = canonical_center1 - prev_center;

                        if flipped {
                            for pt in &mut manifold.points {
                                pt.local_p2 -= delta_center;
                            }
                        } else {
                            for pt in &mut manifold.points {
                                pt.local_p1 -= delta_center;
                            }
                        }

                        // Update contacts.
                        if flipped {
                            manifold.subshape_pos1 = part_pos2.copied();
                            manifold.subshape_pos2 =
                                Some(Pose::from_translation(canonical_center1));
                            let _ = dispatcher.contact_manifold_convex_convex(
                                &relative_pos12.inverse(),
                                part_shape2,
                                &canonical_shape1,
                                None,
                                None,
                                prediction,
                                manifold,
                            );
                        } else {
                            manifold.subshape_pos1 =
                                Some(Pose::from_translation(canonical_center1));
                            manifold.subshape_pos2 = part_pos2.copied();
                            let _ = dispatcher.contact_manifold_convex_convex(
                                &relative_pos12,
                                &canonical_shape1,
                                part_shape2,
                                None,
                                None,
                                prediction,
                                manifold,
                            );
                        }
                    }

                    /*
                     * Filter-out points that don’t belong to this block.
                     */
                    let test_voxel = Cuboid::new(radius1 + Vector::splat(1.0e-2));
                    let penetration_dir1 = if flipped {
                        manifold.local_n2
                    } else {
                        manifold.local_n1
                    };

                    for (i, pt) in manifold.points.iter().enumerate() {
                        if pt.dist < 0.0 {
                            // If this is a penetration, double-check that we are not hitting the
                            // interior of the infinitely expanded canonical shape by checking if
                            // the opposite normal had led to a better vector.
                            let cuboid1 = Cuboid::new(radius1);
                            let sp1 = cuboid1.local_support_point(-penetration_dir1) + vox1.center;
                            let sm2 = part_shape2
                                .as_support_map()
                                .expect("Unsupported collision pair.");
                            let sp2 = sm2.support_point(&relative_pos12, penetration_dir1)
                                + canonical_center1;
                            let test_dist = (sp2 - sp1).dot(-penetration_dir1);
                            let keep = test_dist < pt.dist;

                            if !keep {
                                // We don’t want to keep this point as it is an incorrect penetration
                                // caused by the canonical shape expansion.
                                continue;
                            }
                        }

                        let pt_in_voxel_space = if flipped {
                            manifold.subshape_pos2.transform_point(pt.local_p2) - vox1.center
                        } else {
                            manifold.subshape_pos1.transform_point(pt.local_p1) - vox1.center
                        };
                        sub_detector.selected_contacts |=
                            (test_voxel.contains_local_point(pt_in_voxel_space) as u32) << i;
                    }
                });
            };

            let canon_aabb1_2 = canonical_shape1.compute_aabb(&canonical_pose12.inverse());

            for leaf_id in bvh2.intersect_aabb(&canon_aabb1_2) {
                detect_hit(leaf_id);
            }
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

    // Remove detectors which no longer index a valid manifold.
    workspace
        .sub_detectors
        .retain(|_, detector| detector.timestamp == new_timestamp);
}
