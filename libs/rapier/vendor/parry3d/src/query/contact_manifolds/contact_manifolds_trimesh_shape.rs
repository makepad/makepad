use alloc::{boxed::Box, vec::Vec};

use crate::bounding_volume::{Aabb, BoundingVolume};
use crate::math::{Pose, Real};
use crate::query::contact_manifolds::contact_manifolds_workspace::{
    TypedWorkspaceData, WorkspaceData,
};
use crate::query::contact_manifolds::ContactManifoldsWorkspace;
use crate::query::details::NormalConstraints;
use crate::query::query_dispatcher::PersistentQueryDispatcher;
use crate::query::ContactManifold;
use crate::shape::{Shape, TriMesh};

#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[cfg_attr(
    feature = "rkyv",
    derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)
)]
#[derive(Clone)]
pub struct TriMeshShapeContactManifoldsWorkspace {
    interferences: Vec<u32>,
    local_aabb2: Aabb,
    old_interferences: Vec<u32>,
}

impl Default for TriMeshShapeContactManifoldsWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

impl TriMeshShapeContactManifoldsWorkspace {
    pub fn new() -> Self {
        Self {
            interferences: Vec::new(),
            local_aabb2: Aabb::new_invalid(),
            old_interferences: Vec::new(),
        }
    }
}

/// Computes the contact manifold between a triangle-mesh an a shape, both represented as `Shape` trait-objects.
pub fn contact_manifolds_trimesh_shape_shapes<ManifoldData, ContactData>(
    dispatcher: &dyn PersistentQueryDispatcher<ManifoldData, ContactData>,
    pos12: &Pose,
    shape1: &dyn Shape,
    shape2: &dyn Shape,
    prediction: Real,
    manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
    workspace: &mut Option<ContactManifoldsWorkspace>,
) where
    ManifoldData: Default,
    ContactData: Default + Copy,
{
    if let Some(trimesh1) = shape1.as_trimesh() {
        contact_manifolds_trimesh_shape(
            dispatcher, pos12, trimesh1, shape2, prediction, manifolds, workspace, false,
        )
    } else if let Some(trimesh2) = shape2.as_trimesh() {
        contact_manifolds_trimesh_shape(
            dispatcher,
            &pos12.inverse(),
            trimesh2,
            shape1,
            prediction,
            manifolds,
            workspace,
            true,
        )
    }
}

fn ensure_workspace_exists(workspace: &mut Option<ContactManifoldsWorkspace>) {
    if workspace
        .as_mut()
        .and_then(|w| w.0.downcast_mut::<TriMeshShapeContactManifoldsWorkspace>())
        .is_some()
    {
        return;
    }

    *workspace = Some(ContactManifoldsWorkspace(Box::new(
        TriMeshShapeContactManifoldsWorkspace::new(),
    )));
}

/// Computes the contact manifold between a triangle-mesh and a shape.
pub fn contact_manifolds_trimesh_shape<ManifoldData, ContactData>(
    dispatcher: &dyn PersistentQueryDispatcher<ManifoldData, ContactData>,
    pos12: &Pose,
    trimesh1: &TriMesh,
    shape2: &dyn Shape,
    prediction: Real,
    manifolds: &mut Vec<ContactManifold<ManifoldData, ContactData>>,
    workspace: &mut Option<ContactManifoldsWorkspace>,
    flipped: bool,
) where
    ManifoldData: Default,
    ContactData: Default + Copy,
{
    ensure_workspace_exists(workspace);
    let workspace: &mut TriMeshShapeContactManifoldsWorkspace =
        workspace.as_mut().unwrap().0.downcast_mut().unwrap();

    /*
     * Compute interferences.
     */
    // TODO: somehow precompute the Aabb and reuse it?
    let mut new_local_aabb2 = shape2.compute_aabb(pos12).loosened(prediction);
    let same_local_aabb2 = workspace.local_aabb2.contains(&new_local_aabb2);
    let mut old_manifolds = Vec::new();

    if !same_local_aabb2 {
        let extra_margin =
            (new_local_aabb2.maxs - new_local_aabb2.mins).map(|e| (e / 10.0).min(0.1));
        new_local_aabb2.mins -= extra_margin;
        new_local_aabb2.maxs += extra_margin;

        let local_aabb2 = new_local_aabb2; // .loosened(prediction * 2.0); // TODO: what would be the best value?
        core::mem::swap(
            &mut workspace.old_interferences,
            &mut workspace.interferences,
        );

        core::mem::swap(manifolds, &mut old_manifolds);

        // NOTE: This assertion may fire due to the invalid triangle_ids that the
        // near-phase may return (due to SIMD sentinels).
        //
        // assert_eq!(
        //     workspace
        //         .old_interferences
        //         .len()
        //         .min(trimesh1.num_triangles()),
        //     workspace.old_manifolds.len()
        // );

        workspace.interferences.clear();
        workspace
            .interferences
            .extend(trimesh1.bvh().intersect_aabb(&local_aabb2));
        workspace.local_aabb2 = local_aabb2;
    }

    /*
     * Dispatch to the specific solver by keeping the previous manifold if we already had one.
     */
    let new_interferences = &workspace.interferences;
    let mut old_inter_it = workspace.old_interferences.drain(..).peekable();
    let mut old_manifolds_it = old_manifolds.drain(..);

    // TODO: don't redispatch at each frame (we should probably do the same as
    // the heightfield).
    for (i, triangle_id) in new_interferences.iter().enumerate() {
        if *triangle_id >= trimesh1.num_triangles() as u32 {
            // Because of SIMD padding, the broad-phase may return triangle indices greater
            // than the max.
            continue;
        }

        if !same_local_aabb2 {
            loop {
                match old_inter_it.peek() {
                    Some(old_triangle_id) if *old_triangle_id < *triangle_id => {
                        let _ = old_inter_it.next();
                        let _ = old_manifolds_it.next();
                    }
                    _ => break,
                }
            }

            let manifold = if old_inter_it.peek() != Some(triangle_id) {
                let (id1, id2) = if flipped {
                    (0, *triangle_id)
                } else {
                    (*triangle_id, 0)
                };
                ContactManifold::with_data(id1, id2, ManifoldData::default())
            } else {
                // We already have a manifold for this triangle.
                let _ = old_inter_it.next();
                old_manifolds_it.next().unwrap()
            };

            manifolds.push(manifold);
        }

        let manifold = &mut manifolds[i];
        let triangle1 = trimesh1.triangle(*triangle_id);
        let triangle_normals1 = trimesh1.triangle_normal_constraints(*triangle_id);
        let normal_constraints1 = triangle_normals1
            .as_ref()
            .map(|proj| proj as &dyn NormalConstraints);

        if flipped {
            let _ = dispatcher.contact_manifold_convex_convex(
                &pos12.inverse(),
                shape2,
                &triangle1,
                None,
                normal_constraints1,
                prediction,
                manifold,
            );
        } else {
            let _ = dispatcher.contact_manifold_convex_convex(
                pos12,
                &triangle1,
                shape2,
                normal_constraints1,
                None,
                prediction,
                manifold,
            );
        }
    }
}

impl WorkspaceData for TriMeshShapeContactManifoldsWorkspace {
    fn as_typed_workspace_data(&self) -> TypedWorkspaceData<'_> {
        TypedWorkspaceData::TriMeshShapeContactManifoldsWorkspace(self)
    }

    fn clone_dyn(&self) -> Box<dyn WorkspaceData> {
        Box::new(self.clone())
    }
}
