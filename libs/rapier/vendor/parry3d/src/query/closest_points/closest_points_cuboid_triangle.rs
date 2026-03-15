use crate::math::{Pose, Real};
use crate::query::{sat, ClosestPoints, PointQuery};
use crate::shape::{Cuboid, SupportMap, Triangle};

/// Closest points between a cuboid and a triangle.
#[inline]
pub fn closest_points_cuboid_triangle(
    pos12: &Pose,
    cuboid1: &Cuboid,
    triangle2: &Triangle,
    margin: Real,
) -> ClosestPoints {
    let pos21 = pos12.inverse();

    let sep1 =
        sat::cuboid_support_map_find_local_separating_normal_oneway(cuboid1, triangle2, pos12);
    if sep1.0 > margin {
        return ClosestPoints::Disjoint;
    }

    let sep2 = sat::triangle_cuboid_find_local_separating_normal_oneway(triangle2, cuboid1, &pos21);
    if sep2.0 > margin {
        return ClosestPoints::Disjoint;
    }

    #[cfg(feature = "dim2")]
    let sep3 = (-Real::MAX, crate::math::Vector::Y); // This case does not exist in 2D.
    #[cfg(feature = "dim3")]
    let sep3 = sat::cuboid_triangle_find_local_separating_edge_twoway(cuboid1, triangle2, pos12);
    if sep3.0 > margin {
        return ClosestPoints::Disjoint;
    }

    if sep1.0 <= 0.0 && sep2.0 <= 0.0 && sep3.0 <= 0.0 {
        return ClosestPoints::Intersecting;
    }

    // The best separating axis is face-vertex.
    if sep1.0 >= sep2.0 && sep1.0 >= sep3.0 {
        // To compute the closest points, we need to project the support point
        // from triangle2 on the support-face of triangle1. For simplicity, we just
        // project the support point from triangle2 on cuboid1 itself (not just the face).
        let pt2_1 = triangle2.support_point(pos12, -sep1.1);
        let proj1 = cuboid1.project_local_point(pt2_1, true);
        if (proj1.point - pt2_1).length_squared() > margin * margin {
            return ClosestPoints::Disjoint;
        } else {
            return ClosestPoints::WithinMargin(proj1.point, pos21 * pt2_1);
        }
    }

    // The best separating axis is vertex-face.
    if sep2.0 >= sep1.0 && sep2.0 >= sep3.0 {
        // To compute the actual closest points, we need to project the support point
        // from cuboid1 on the support-face of triangle2. For simplicity, we just
        // project the support point from cuboid1 on triangle2 itself (not just the face).
        let pt1_2 = cuboid1.support_point(&pos21, -sep2.1);
        let proj2 = triangle2.project_local_point(pt1_2, true);

        if (proj2.point - pt1_2).length_squared() > margin * margin {
            return ClosestPoints::Disjoint;
        } else {
            return ClosestPoints::WithinMargin(pos12 * pt1_2, proj2.point);
        }
    }

    // The best separating axis is edge-edge.
    #[cfg(feature = "dim3")]
    if sep3.0 >= sep2.0 && sep3.0 >= sep1.0 {
        // To compute the actual distance, we need to compute the closest
        // points between the two edges that generated the separating axis.
        let edge1 = cuboid1.local_support_edge_segment(sep3.1);
        let edge2 = triangle2.local_support_edge_segment(pos21.rotation * -sep3.1);
        return super::closest_points_segment_segment(pos12, &edge1, &edge2, margin);
    }

    unreachable!()
}

/// Closest points between a triangle and a cuboid.
#[inline]
pub fn closest_points_triangle_cuboid(
    pos12: &Pose,
    triangle1: &Triangle,
    cuboid2: &Cuboid,
    margin: Real,
) -> ClosestPoints {
    closest_points_cuboid_triangle(&pos12.inverse(), cuboid2, triangle1, margin).flipped()
}
