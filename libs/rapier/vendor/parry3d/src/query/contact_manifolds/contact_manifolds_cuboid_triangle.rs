#[cfg(feature = "dim2")]
use crate::math::Vector;
use crate::math::{Pose, Real};
use crate::query::contact_manifolds::{NormalConstraints, NormalConstraintsPair};
use crate::query::{sat, ContactManifold};
use crate::shape::PolygonalFeature;
use crate::shape::{Cuboid, Shape, Triangle};

/// Computes the contact manifold between a cuboid and a triangle represented as `Shape` trait-objects.
pub fn contact_manifold_cuboid_triangle_shapes<ManifoldData, ContactData>(
    pos12: &Pose,
    shape1: &dyn Shape,
    shape2: &dyn Shape,
    normal_constraints1: Option<&dyn NormalConstraints>,
    normal_constraints2: Option<&dyn NormalConstraints>,
    prediction: Real,
    manifold: &mut ContactManifold<ManifoldData, ContactData>,
) where
    ContactData: Default + Copy,
{
    if let (Some(cuboid1), Some(triangle2)) = (shape1.as_cuboid(), shape2.as_triangle()) {
        contact_manifold_cuboid_triangle(
            pos12,
            &pos12.inverse(),
            cuboid1,
            triangle2,
            normal_constraints1,
            normal_constraints2,
            prediction,
            manifold,
            false,
        );
    } else if let (Some(triangle1), Some(cuboid2)) = (shape1.as_triangle(), shape2.as_cuboid()) {
        contact_manifold_cuboid_triangle(
            &pos12.inverse(),
            pos12,
            cuboid2,
            triangle1,
            normal_constraints2,
            normal_constraints1,
            prediction,
            manifold,
            true,
        );
    }
}

/// Computes the contact manifold between a cuboid and a triangle.
pub fn contact_manifold_cuboid_triangle<'a, ManifoldData, ContactData>(
    pos12: &Pose,
    pos21: &Pose,
    cuboid1: &'a Cuboid,
    triangle2: &'a Triangle,
    normal_constraints1: Option<&dyn NormalConstraints>,
    normal_constraints2: Option<&dyn NormalConstraints>,
    prediction: Real,
    manifold: &mut ContactManifold<ManifoldData, ContactData>,
    flipped: bool,
) where
    ContactData: Default + Copy,
{
    if (!flipped && manifold.try_update_contacts(pos12))
        || (flipped && manifold.try_update_contacts(pos21))
    {
        return;
    }

    /*
     *
     * Vector-Face cases.
     *
     */
    let sep1 =
        sat::cuboid_support_map_find_local_separating_normal_oneway(cuboid1, triangle2, pos12);
    if sep1.0 > prediction {
        manifold.clear();
        return;
    }

    let sep2 = sat::triangle_cuboid_find_local_separating_normal_oneway(triangle2, cuboid1, pos21);
    if sep2.0 > prediction {
        manifold.clear();
        return;
    }

    /*
     *
     * Edge-Edge cases.
     *
     */
    #[cfg(feature = "dim2")]
    let sep3 = (-Real::MAX, Vector::X); // This case does not exist in 2D.
    #[cfg(feature = "dim3")]
    let sep3 = sat::cuboid_triangle_find_local_separating_edge_twoway(cuboid1, triangle2, pos12);
    if sep3.0 > prediction {
        manifold.clear();
        return;
    }

    /*
     *
     * Select the best combination of features
     * and get the polygons to clip.
     *
     */
    let mut normal1 = sep1.1;
    let mut dist = sep1.0;

    if sep2.0 > sep1.0 && sep2.0 > sep3.0 {
        normal1 = pos12.rotation * -sep2.1;
        dist = sep2.0;
    } else if sep3.0 > sep1.0 {
        normal1 = sep3.1;
        dist = sep3.0;
    }

    // Apply any normal constraint to the separating axis.
    let mut normal2 = pos21.rotation * -normal1;

    if !(normal_constraints1, normal_constraints2).project_local_normals(
        pos12,
        &mut normal1,
        &mut normal2,
    ) {
        manifold.clear();
        return; // The contact got completely discarded by normal correction.
    }

    let feature1;
    let feature2;

    #[cfg(feature = "dim2")]
    {
        feature1 = cuboid1.support_face(normal1);
        feature2 = triangle2.support_face(normal2);
    }
    #[cfg(feature = "dim3")]
    {
        feature1 = cuboid1.support_face(normal1);
        feature2 = PolygonalFeature::from(*triangle2);
    }

    // We do this clone to perform contact tracking and transfer impulses.
    // TODO: find a more efficient way of doing this.
    let old_manifold_points = manifold.points.clone();
    manifold.clear();

    PolygonalFeature::contacts(
        pos12, pos21, normal1, normal2, &feature1, &feature2, manifold, flipped,
    );

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

    if flipped {
        manifold.local_n1 = normal2;
        manifold.local_n2 = normal1;
    } else {
        manifold.local_n1 = normal1;
        manifold.local_n2 = normal2;
    }

    // Transfer impulses.
    manifold.match_contacts(&old_manifold_points);
}
