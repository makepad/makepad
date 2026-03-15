use crate::math::{Pose, Real, Vector};
use crate::query::contact_manifolds::{NormalConstraints, NormalConstraintsPair};
use crate::query::{
    self,
    gjk::{GJKResult, VoronoiSimplex},
    ContactManifold, TrackedContact,
};
use crate::shape::{PackedFeatureId, PolygonalFeature, PolygonalFeatureMap, Shape};

/// Computes the contact manifold between two convex shapes implementing the `PolygonalSupportMap`
/// trait, both represented as `Shape` trait-objects.
pub fn contact_manifold_pfm_pfm_shapes<ManifoldData, ContactData>(
    pos12: &Pose,
    shape1: &dyn Shape,
    shape2: &dyn Shape,
    normal_constraints1: Option<&dyn NormalConstraints>,
    normal_constraints2: Option<&dyn NormalConstraints>,
    prediction: Real,
    manifold: &mut ContactManifold<ManifoldData, ContactData>,
) where
    ManifoldData: Default,
    ContactData: Default + Copy,
{
    if let (Some((pfm1, border_radius1)), Some((pfm2, border_radius2))) = (
        shape1.as_polygonal_feature_map(),
        shape2.as_polygonal_feature_map(),
    ) {
        contact_manifold_pfm_pfm(
            pos12,
            pfm1,
            border_radius1,
            normal_constraints1,
            pfm2,
            border_radius2,
            normal_constraints2,
            prediction,
            manifold,
        );
    }
}

/// Computes the contact manifold between two convex shapes implementing the `PolygonalSupportMap` trait.
pub fn contact_manifold_pfm_pfm<'a, ManifoldData, ContactData, S1, S2>(
    pos12: &Pose,
    pfm1: &'a S1,
    border_radius1: Real,
    normal_constraints1: Option<&dyn NormalConstraints>,
    pfm2: &'a S2,
    border_radius2: Real,
    normal_constraints2: Option<&dyn NormalConstraints>,
    prediction: Real,
    manifold: &mut ContactManifold<ManifoldData, ContactData>,
) where
    S1: ?Sized + PolygonalFeatureMap,
    S2: ?Sized + PolygonalFeatureMap,
    ManifoldData: Default,
    ContactData: Default + Copy,
{
    // We use very small thresholds for the manifold update because something to high would
    // cause numerical drifts with the effect of introducing bumps in
    // what should have been smooth rolling motions.
    if manifold.try_update_contacts_eps(pos12, crate::utils::COS_1_DEGREES, 1.0e-6) {
        return;
    }

    let init_dir = (manifold.local_n1).try_normalize();
    let total_prediction = prediction + border_radius1 + border_radius2;
    let contact = query::details::contact_support_map_support_map_with_params(
        pos12,
        pfm1,
        pfm2,
        total_prediction,
        &mut VoronoiSimplex::new(),
        init_dir,
    );

    let old_manifold_points = manifold.points.clone();
    manifold.clear();

    match contact {
        GJKResult::ClosestPoints(p1, p2_1, dir) => {
            let mut local_n1 = dir;
            let mut local_n2 = pos12.rotation.inverse() * -dir;
            let dist = (p2_1 - p1).dot(local_n1);

            if !(normal_constraints1, normal_constraints2).project_local_normals(
                pos12,
                &mut local_n1,
                &mut local_n2,
            ) {
                // The contact got completely discarded by the normal correction.
                return;
            }

            let mut feature1 = PolygonalFeature::default();
            let mut feature2 = PolygonalFeature::default();
            pfm1.local_support_feature(local_n1, &mut feature1);
            pfm2.local_support_feature(local_n2, &mut feature2);

            PolygonalFeature::contacts(
                pos12,
                &pos12.inverse(),
                local_n1,
                local_n2,
                &feature1,
                &feature2,
                manifold,
                false,
            );

            if (cfg!(feature = "dim3") || cfg!(feature = "dim2") && manifold.points.is_empty())
                // If normal constraints changed the GJK direction, it is no longer valid so we cant use it for this additional contact.
                && local_n1 == dir
            {
                let contact = TrackedContact::new(
                    p1,
                    pos12.inverse_transform_point(p2_1),
                    PackedFeatureId::UNKNOWN, // TODO: We don't know what features are involved.
                    PackedFeatureId::UNKNOWN,
                    (p2_1 - p1).dot(local_n1),
                );
                manifold.points.push(contact);
            }

            if normal_constraints1.is_some() || normal_constraints2.is_some() {
                // HACK: some normal correction can lead to very incorrect penetration
                //       depth, e.g., if the other object extends very far toward that direction.
                //       This is caused by the locality of the convex/convex check.
                //       I haven’t found a good mathematically robust approach to account for
                //       that locally, so for now, we eliminate points that are large divergence
                //       relative to the unconstrained penetration distance.
                manifold
                    .points
                    .retain(|pt| dist >= 0.0 || pt.dist >= 0.0 || pt.dist >= dist * 5.0);
            }

            // Adjust points to take the radius into account.
            if border_radius1 != 0.0 || border_radius2 != 0.0 {
                for contact in &mut manifold.points {
                    contact.local_p1 += local_n1 * border_radius1;
                    contact.local_p2 += local_n2 * border_radius2;
                    contact.dist -= border_radius1 + border_radius2;
                }
            }

            manifold.local_n1 = local_n1;
            manifold.local_n2 = local_n2;
        }
        GJKResult::NoIntersection(dir) => {
            // Use the manifold normal as a cache.
            manifold.local_n1 = dir;
        }
        _ => {
            // Reset the cached direction.
            manifold.local_n1 = Vector::ZERO;
        }
    }

    // Transfer impulses.
    manifold.match_contacts(&old_manifold_points);
}
