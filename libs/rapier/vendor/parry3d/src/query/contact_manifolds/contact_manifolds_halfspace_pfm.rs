use crate::math::{Pose, Real};
use crate::query::{ContactManifold, TrackedContact};
use crate::shape::{HalfSpace, PackedFeatureId, PolygonalFeature, PolygonalFeatureMap, Shape};

/// Computes the contact manifold between a convex shape and a ball, both represented as a `Shape` trait-object.
pub fn contact_manifold_halfspace_pfm_shapes<ManifoldData, ContactData>(
    pos12: &Pose,
    shape1: &dyn Shape,
    shape2: &dyn Shape,
    prediction: Real,
    manifold: &mut ContactManifold<ManifoldData, ContactData>,
) where
    ContactData: Default + Copy,
{
    if let (Some(halfspace1), Some((pfm2, border_radius2))) =
        (shape1.as_halfspace(), shape2.as_polygonal_feature_map())
    {
        contact_manifold_halfspace_pfm(
            pos12,
            halfspace1,
            pfm2,
            border_radius2,
            prediction,
            manifold,
            false,
        );
    } else if let (Some((pfm1, border_radius1)), Some(halfspace2)) =
        (shape1.as_polygonal_feature_map(), shape2.as_halfspace())
    {
        contact_manifold_halfspace_pfm(
            &pos12.inverse(),
            halfspace2,
            pfm1,
            border_radius1,
            prediction,
            manifold,
            true,
        );
    }
}

/// Computes the contact manifold between a convex shape and a ball.
pub fn contact_manifold_halfspace_pfm<'a, ManifoldData, ContactData, S2>(
    pos12: &Pose,
    halfspace1: &'a HalfSpace,
    pfm2: &'a S2,
    border_radius2: Real,
    prediction: Real,
    manifold: &mut ContactManifold<ManifoldData, ContactData>,
    flipped: bool,
) where
    S2: ?Sized + PolygonalFeatureMap,
    ContactData: Default + Copy,
{
    let normal1_2 = pos12.rotation.inverse() * halfspace1.normal;
    let mut feature2 = PolygonalFeature::default();
    pfm2.local_support_feature(-normal1_2, &mut feature2);

    // We do this clone to perform contact tracking and transfer impulses.
    // TODO: find a more efficient way of doing this.
    let old_manifold_points = core::mem::take(&mut manifold.points);

    for i in 0..feature2.num_vertices {
        let vtx2 = feature2.vertices[i];
        let vtx2_1 = pos12 * vtx2;
        let dist_to_plane = vtx2_1.dot(halfspace1.normal);

        if dist_to_plane - border_radius2 <= prediction {
            // Keep this contact point.
            manifold.points.push(TrackedContact::flipped(
                vtx2_1 - halfspace1.normal * dist_to_plane,
                vtx2 - normal1_2 * border_radius2,
                PackedFeatureId::face(0),
                feature2.vids[i],
                dist_to_plane - border_radius2,
                flipped,
            ));
        }
    }

    if flipped {
        manifold.local_n1 = -normal1_2;
        manifold.local_n2 = halfspace1.normal;
    } else {
        manifold.local_n1 = halfspace1.normal;
        manifold.local_n2 = -normal1_2;
    }

    // println!("Found contacts: {}", manifold.points.len());

    // Transfer impulses.
    manifold.match_contacts(&old_manifold_points);
}
