use crate::math::{Pose, Real, Vector};
use crate::query::{sat, ContactManifold};
#[cfg(feature = "dim3")]
use crate::shape::PolygonalFeature;
use crate::shape::{Capsule, Cuboid, Shape};
#[cfg(feature = "dim2")]
use crate::shape::{CuboidFeature, CuboidFeatureFace};

/// Computes the contact manifold between a cuboid and a capsule, both represented as `Shape` trait-objects.
pub fn contact_manifold_cuboid_capsule_shapes<ManifoldData, ContactData>(
    pos12: &Pose,
    shape1: &dyn Shape,
    shape2: &dyn Shape,
    prediction: Real,
    manifold: &mut ContactManifold<ManifoldData, ContactData>,
) where
    ContactData: Default + Copy,
{
    if let (Some(cube1), Some(capsule2)) = (shape1.as_cuboid(), shape2.as_capsule()) {
        contact_manifold_cuboid_capsule(
            pos12,
            &pos12.inverse(),
            cube1,
            capsule2,
            prediction,
            manifold,
            false,
        );
    } else if let (Some(capsule1), Some(cube2)) = (shape1.as_capsule(), shape2.as_cuboid()) {
        contact_manifold_cuboid_capsule(
            &pos12.inverse(),
            pos12,
            cube2,
            capsule1,
            prediction,
            manifold,
            true,
        );
    }
}

/// Computes the contact manifold between a cuboid and a capsule.
pub fn contact_manifold_cuboid_capsule<'a, ManifoldData, ContactData>(
    pos12: &Pose,
    pos21: &Pose,
    cube1: &'a Cuboid,
    capsule2: &'a Capsule,
    prediction: Real,
    manifold: &mut ContactManifold<ManifoldData, ContactData>,
    flipped: bool,
) where
    ContactData: Default + Copy,
{
    if (!flipped && manifold.try_update_contacts(&pos12))
        || (flipped && manifold.try_update_contacts(&pos21))
    {
        return;
    }

    let segment2 = capsule2.segment;

    /*
     *
     * Vector-Face cases.
     *
     */
    let sep1 =
        sat::cuboid_support_map_find_local_separating_normal_oneway(cube1, &segment2, &pos12);
    if sep1.0 > prediction + capsule2.radius {
        manifold.clear();
        return;
    }

    #[cfg(feature = "dim3")]
    let sep2 = (-Real::MAX, Vector::X);
    #[cfg(feature = "dim2")]
    let sep2 = sat::segment_cuboid_find_local_separating_normal_oneway(&segment2, cube1, &pos21);
    if sep2.0 > prediction + capsule2.radius {
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
    let sep3 = sat::cuboid_segment_find_local_separating_edge_twoway(cube1, &segment2, &pos12);
    if sep3.0 > prediction + capsule2.radius {
        manifold.clear();
        return;
    }

    /*
     *
     * Select the best combination of features
     * and get the polygons to clip.
     *
     */
    let mut best_sep = sep1;

    if sep2.0 > sep1.0 && sep2.0 > sep3.0 {
        best_sep = (sep2.0, pos12.rotation * -sep2.1);
    } else if sep3.0 > sep1.0 {
        best_sep = sep3;
    }

    let feature1;
    let feature2;

    #[cfg(feature = "dim2")]
    {
        feature1 = cube1.support_face(best_sep.1);
        feature2 = CuboidFeatureFace::from(segment2);
    }
    #[cfg(feature = "dim3")]
    {
        feature1 = cube1.polyhedron_support_face(best_sep.1);
        feature2 = PolygonalFeature::from(segment2);
    }

    // We do this clone to perform contact tracking and transfer impulses.
    // TODO: find a more efficient way of doing this.
    let old_manifold_points = manifold.points.clone();
    manifold.clear();

    #[cfg(feature = "dim2")]
    CuboidFeature::face_face_contacts(
        pos12,
        &feature1,
        &best_sep.1,
        &feature2,
        prediction + capsule2.radius,
        manifold,
        flipped,
    );

    #[cfg(feature = "dim3")]
    PolygonalFeature::contacts(
        pos12,
        &feature1,
        &best_sep.1,
        &feature2,
        prediction + capsule2.radius,
        manifold,
        flipped,
    );

    // Adjust points to take the radius into account.
    let normal2 = pos21.rotation * -best_sep.1;

    if flipped {
        manifold.local_n1 = normal2;
        manifold.local_n2 = best_sep.1;

        for point in &mut manifold.points {
            point.local_p1 += manifold.local_n1 * capsule2.radius;
            point.dist -= capsule2.radius;
        }
    } else {
        manifold.local_n1 = best_sep.1;
        manifold.local_n2 = normal2;

        for point in &mut manifold.points {
            point.local_p2 += manifold.local_n2 * capsule2.radius;
            point.dist -= capsule2.radius;
        }
    }

    // Transfer impulses.
    manifold.match_contacts(&old_manifold_points);
}
