use alloc::{boxed::Box, vec::Vec};

use crate::bounding_volume::BoundingVolume;
use crate::math::{Pose, Real};
use crate::query::contact_manifolds::contact_manifolds_workspace::{
    TypedWorkspaceData, WorkspaceData,
};
use crate::query::contact_manifolds::{ContactManifoldsWorkspace, NormalConstraints};
use crate::query::query_dispatcher::PersistentQueryDispatcher;
use crate::query::ContactManifold;
#[cfg(feature = "dim2")]
use crate::shape::Capsule;
use crate::shape::{CompositeShape, HeightField, Shape};
use crate::utils::hashmap::{Entry, HashMap};
use crate::utils::PoseOpt;

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(Clone)]
struct SubDetector {
    manifold_id: usize,
    timestamp: bool,
}

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Default)]
pub struct HeightFieldCompositeShapeContactManifoldsWorkspace {
    timestamp: bool,
    sub_detectors: HashMap<(u32, u32), SubDetector>,
}

impl HeightFieldCompositeShapeContactManifoldsWorkspace {
    pub fn new() -> Self {
        Self::default()
    }
}

fn ensure_workspace_exists(workspace: &mut Option<ContactManifoldsWorkspace>) {
    if workspace
        .as_ref()
        .and_then(|w| {
            w.0.downcast_ref::<HeightFieldCompositeShapeContactManifoldsWorkspace>()
        })
        .is_some()
    {
        return;
    }

    *workspace = Some(ContactManifoldsWorkspace(Box::new(
        HeightFieldCompositeShapeContactManifoldsWorkspace::new(),
    )));
}

/// Computes the contact manifold between an heightfield and a composite shape.
pub fn contact_manifolds_heightfield_composite_shape<ManifoldData, ContactData>(
    dispatcher: &dyn PersistentQueryDispatcher<ManifoldData, ContactData>,
    pos12: &Pose,
    pos21: &Pose,
    heightfield1: &HeightField,
    composite2: &dyn CompositeShape,
    prediction: Real,
    manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
    workspace: &mut Option<ContactManifoldsWorkspace>,
    flipped: bool,
) where
    ManifoldData: Default + Clone,
    ContactData: Default + Copy,
{
    ensure_workspace_exists(workspace);
    let workspace: &mut HeightFieldCompositeShapeContactManifoldsWorkspace =
        workspace.as_mut().unwrap().0.downcast_mut().unwrap();
    let new_timestamp = !workspace.timestamp;
    workspace.timestamp = new_timestamp;

    /*
     * Compute interferences.
     */
    let bvh2 = composite2.bvh();
    let ls_aabb2_1 = bvh2.root_aabb().transform_by(pos12).loosened(prediction);
    let mut old_manifolds = core::mem::take(manifolds);

    heightfield1.map_elements_in_local_aabb(&ls_aabb2_1, &mut |leaf1, part1| {
        #[cfg(feature = "dim2")]
        let sub_shape1 = Capsule::new(part1.a, part1.b, 0.0); // TODO: use a segment instead.
        #[cfg(feature = "dim3")]
        let sub_shape1 = *part1;

        let ls_aabb1_2 = part1.compute_aabb(pos21).loosened(prediction);
        let mut leaf_fn2 = |leaf2: u32| {
            composite2.map_part_at(leaf2, &mut |part_pos2, part_shape2, normal_constraints2| {
                let sub_detector = match workspace.sub_detectors.entry((leaf1, leaf2)) {
                    Entry::Occupied(entry) => {
                        let sub_detector = entry.into_mut();
                        let manifold = old_manifolds[sub_detector.manifold_id].take();
                        sub_detector.manifold_id = manifolds.len();
                        sub_detector.timestamp = new_timestamp;
                        manifolds.push(manifold);
                        sub_detector
                    }
                    Entry::Vacant(entry) => {
                        let sub_detector = SubDetector {
                            manifold_id: manifolds.len(),
                            timestamp: new_timestamp,
                        };

                        let mut manifold = ContactManifold::new();

                        if flipped {
                            manifold.subshape1 = leaf2;
                            manifold.subshape2 = leaf1;
                            manifold.subshape_pos1 = part_pos2.copied();
                        } else {
                            manifold.subshape1 = leaf1;
                            manifold.subshape2 = leaf2;
                            manifold.subshape_pos2 = part_pos2.copied();
                        };

                        manifolds.push(manifold);
                        entry.insert(sub_detector)
                    }
                };

                let manifold = &mut manifolds[sub_detector.manifold_id];

                #[cfg(feature = "dim2")]
                let triangle_normals = None::<()>;
                #[cfg(feature = "dim3")]
                let triangle_normals = heightfield1.triangle_normal_constraints(leaf1);
                let normal_constraints1 = triangle_normals
                    .as_ref()
                    .map(|proj| proj as &dyn NormalConstraints);

                if flipped {
                    let _ = dispatcher.contact_manifold_convex_convex(
                        &part_pos2.inv_mul(pos21),
                        part_shape2,
                        &sub_shape1,
                        normal_constraints2,
                        normal_constraints1,
                        prediction,
                        manifold,
                    );
                } else {
                    let _ = dispatcher.contact_manifold_convex_convex(
                        &part_pos2.prepend_to(pos12),
                        &sub_shape1,
                        part_shape2,
                        normal_constraints1,
                        normal_constraints2,
                        prediction,
                        manifold,
                    );
                }
            });
        };

        for leaf_id in bvh2.intersect_aabb(&ls_aabb1_2) {
            leaf_fn2(leaf_id);
        }
    });

    workspace
        .sub_detectors
        .retain(|_, detector| detector.timestamp == new_timestamp);
}

impl WorkspaceData for HeightFieldCompositeShapeContactManifoldsWorkspace {
    fn as_typed_workspace_data(&self) -> TypedWorkspaceData<'_> {
        TypedWorkspaceData::HeightfieldCompositeShapeContactManifoldsWorkspace(self)
    }

    fn clone_dyn(&self) -> Box<dyn WorkspaceData> {
        Box::new(self.clone())
    }
}
