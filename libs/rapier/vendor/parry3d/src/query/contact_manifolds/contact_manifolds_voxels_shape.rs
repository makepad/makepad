use crate::bounding_volume::Aabb;
use crate::math::{IVector, Int, Pose, Real, Vector, DIM};
use crate::query::{
    ContactManifold, ContactManifoldsWorkspace, PersistentQueryDispatcher, PointQuery,
    TypedWorkspaceData, WorkspaceData,
};
use crate::shape::{AxisMask, Cuboid, Shape, SupportMap, VoxelData, VoxelType, Voxels};
use crate::utils::hashmap::{Entry, HashMap};
use crate::utils::PoseOpt;
use alloc::{boxed::Box, vec::Vec};
use num::AsPrimitive;

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(Clone)]
pub(crate) struct VoxelsShapeSubDetector {
    pub manifold_id: usize,
    pub selected_contacts: u32,
    pub timestamp: bool,
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Eq, Hash, PartialEq)]
pub(crate) struct VoxelsWorkspaceKey<const N: usize> {
    #[cfg_attr(feature = "serde-serialize", serde(with = "serde_arrays"))]
    idx: [u32; N],
}

impl<const N: usize> From<[u32; N]> for VoxelsWorkspaceKey<N> {
    fn from(idx: [u32; N]) -> Self {
        Self { idx }
    }
}

// NOTE: this is using a similar kind of cache as compound shape and height-field.
//       It is different from the trimesh cash though. Which one is better?
/// A workspace for collision-detection against voxels shape.
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Default)]
pub struct VoxelsShapeContactManifoldsWorkspace<const N: usize> {
    pub(crate) timestamp: bool,
    pub(crate) sub_detectors: HashMap<VoxelsWorkspaceKey<N>, VoxelsShapeSubDetector>,
}

impl<const N: usize> VoxelsShapeContactManifoldsWorkspace<N> {
    /// A new empty workspace for collision-detection against voxels shape.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn ensure_exists(workspace: &mut Option<ContactManifoldsWorkspace>)
    where
        Self: WorkspaceData,
    {
        if workspace
            .as_ref()
            .and_then(|w| {
                w.0.downcast_ref::<VoxelsShapeContactManifoldsWorkspace<N>>()
            })
            .is_some()
        {
            return;
        }

        *workspace = Some(ContactManifoldsWorkspace(Box::new(
            VoxelsShapeContactManifoldsWorkspace::new(),
        )));
    }
}

impl WorkspaceData for VoxelsShapeContactManifoldsWorkspace<2> {
    fn as_typed_workspace_data(&self) -> TypedWorkspaceData<'_> {
        TypedWorkspaceData::VoxelsShapeContactManifoldsWorkspace(self)
    }

    fn clone_dyn(&self) -> Box<dyn WorkspaceData> {
        Box::new(self.clone())
    }
}

/// Computes the contact manifold between a convex shape and a voxels shape, both represented as a `Shape` trait-object.
pub fn contact_manifolds_voxels_shape_shapes<ManifoldData, ContactData>(
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
    if let Some(voxels1) = shape1.as_voxels() {
        contact_manifolds_voxels_shape(
            dispatcher, pos12, voxels1, shape2, prediction, manifolds, workspace, false,
        );
    } else if let Some(voxels2) = shape2.as_voxels() {
        contact_manifolds_voxels_shape(
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

/// Computes the contact manifold between a convex shape and a voxels shape.
pub fn contact_manifolds_voxels_shape<ManifoldData, ContactData>(
    dispatcher: &dyn PersistentQueryDispatcher<ManifoldData, ContactData>,
    pos12: &Pose,
    voxels1: &Voxels,
    shape2: &dyn Shape,
    prediction: Real,
    manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
    workspace: &mut Option<ContactManifoldsWorkspace>,
    flipped: bool,
) where
    ManifoldData: Default + Clone,
    ContactData: Default + Copy,
{
    VoxelsShapeContactManifoldsWorkspace::<2>::ensure_exists(workspace);
    let workspace: &mut VoxelsShapeContactManifoldsWorkspace<2> =
        workspace.as_mut().unwrap().0.downcast_mut().unwrap();
    let new_timestamp = !workspace.timestamp;
    workspace.timestamp = new_timestamp;

    // TODO: avoid reallocating the new `manifolds` vec at each step.
    let mut old_manifolds = core::mem::take(manifolds);

    let radius1 = voxels1.voxel_size() / 2.0;

    let aabb1 = voxels1.local_aabb();
    let aabb2_1 = shape2.compute_aabb(pos12);
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

            let canon1 = CanonicalVoxelShape::from_voxel(voxels1, &vox1);

            // TODO: could we refactor the workspace system between Voxels, HeightField, and CompoundShape?
            //       (and maybe TriMesh too but it’s using a different approach).
            let (sub_detector, manifold_updated) =
                match workspace.sub_detectors.entry(canon1.workspace_key.into()) {
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

                        let vid = vox1.linear_id.flat_id() as u32;
                        let (id1, id2) = if flipped { (0, vid) } else { (vid, 0) };
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

            if !manifold_updated {
                let (canonical_center1, canonical_pseudo_cube1) =
                    canon1.cuboid(voxels1, &vox1, domain2_1);

                let canonical_shape1 = &canonical_pseudo_cube1 as &dyn Shape;
                let canonical_pos12 = Pose::from_translation(-canonical_center1) * pos12;

                // If we already computed contacts in the previous simulation step, their
                // local points are relative to the previously calculated canonical shape
                // which might not have the same local center as the one computed in this
                // step (because it’s based on the position of shape2 relative to voxels1).
                // So we need to adjust the local points to account for the position difference
                // and keep the point at the same "canonica-shape-space" location as in the previous frame.
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
                    manifold.subshape_pos2 = Some(Pose::from_translation(canonical_center1));
                    let _ = dispatcher.contact_manifold_convex_convex(
                        &canonical_pos12.inverse(),
                        shape2,
                        canonical_shape1,
                        None,
                        None,
                        prediction,
                        manifold,
                    );
                } else {
                    manifold.subshape_pos1 = Some(Pose::from_translation(canonical_center1));
                    let _ = dispatcher.contact_manifold_convex_convex(
                        &canonical_pos12,
                        canonical_shape1,
                        shape2,
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
                    let sm2 = shape2
                        .as_support_map()
                        .expect("Unsupported collision pair.");
                    let sp2 = sm2.support_point(pos12, penetration_dir1);
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

#[derive(Copy, Clone, Debug)]
pub(crate) struct CanonicalVoxelShape {
    pub range: [IVector; 2],
    pub workspace_key: [u32; 2],
}

impl CanonicalVoxelShape {
    pub fn from_voxel(voxels: &Voxels, vox: &VoxelData) -> Self {
        let mut key_low = vox.grid_coords;
        let mut key_high = key_low;

        // NOTE: the mins/maxs here are offset by 1 so we can expand past the last voxel if it
        //       happens to also be infinite along the same axis (due to cross-voxels internal edges
        //       detection).
        let mins = voxels.domain()[0] - IVector::splat(1);
        let maxs = voxels.domain()[1];
        let counts = maxs - mins;
        let mask1 = vox.state.free_faces();

        let adjust_canon = |axis: AxisMask, i: usize, key: &mut IVector, val: Int| {
            if !mask1.contains(axis) {
                key[i] = val.as_();
            }
        };

        adjust_canon(AxisMask::X_POS, 0, &mut key_high, maxs[0]);
        adjust_canon(AxisMask::X_NEG, 0, &mut key_low, mins[0]);
        adjust_canon(AxisMask::Y_POS, 1, &mut key_high, maxs[1]);
        adjust_canon(AxisMask::Y_NEG, 1, &mut key_low, mins[1]);

        #[cfg(feature = "dim3")]
        {
            adjust_canon(AxisMask::Z_POS, 2, &mut key_high, maxs[2]);
            adjust_canon(AxisMask::Z_NEG, 2, &mut key_low, mins[2]);
        }

        #[cfg(feature = "dim2")]
        let workspace_id = |vox: IVector| {
            let local = vox - mins;
            (local.x + local.y * counts.x) as u32
        };

        #[cfg(feature = "dim3")]
        let workspace_id = |vox: IVector| {
            let local = vox - mins;
            (local.x + local.y * counts.x + local.z * counts.x * counts.y) as u32
        };

        Self {
            range: [key_low, key_high],
            workspace_key: [workspace_id(key_low), workspace_id(key_high)],
        }
    }

    pub fn cuboid(&self, voxels: &Voxels, vox: &VoxelData, domain2_1: Aabb) -> (Vector, Cuboid) {
        let radius = voxels.voxel_size() / 2.0;
        let mut canonical_mins = voxels.voxel_center(self.range[0]);
        let mut canonical_maxs = voxels.voxel_center(self.range[1]);

        for k in 0..DIM {
            if self.range[0][k] != vox.grid_coords[k] {
                canonical_mins[k] = canonical_mins[k].max(domain2_1.mins[k]);
            }

            if self.range[1][k] != vox.grid_coords[k] {
                canonical_maxs[k] = canonical_maxs[k].min(domain2_1.maxs[k]);
            }
        }

        let canonical_half_extents = (canonical_maxs - canonical_mins) / 2.0 + radius;
        let canonical_center = canonical_mins.midpoint(canonical_maxs);
        let canonical_cube = Cuboid::new(canonical_half_extents);

        (canonical_center, canonical_cube)
    }
}
