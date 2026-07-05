// Port of box3d/src/convex_manifold.c
// Convex collide functions: sphere/capsule/hull pairs producing LocalManifolds.
//
// LocalManifold convention: `points` is a Vec cleared at the point where the C
// code sets pointCount, then pushed; point_count() == points.len().

use crate::b3_assert;
use crate::b3_validate;
use crate::constants::{linear_slop, min_capsule_length, speculative_distance};
use crate::core::NULL_INDEX;
use crate::manifold::{
    clip_polygon, edge_edge_separation, find_incident_face, flip_pair, make_feature_pair,
    validate_polygon, ClipVertex, EdgeQuery, FaceQuery, FeatureOwner, FEATURE_PAIR_SINGLE,
    MAX_CLIP_POINTS,
};
use crate::math_functions::{
    abs_float, add, cross, distance as vec_distance, dot, get_length_and_normalize, invert_transform,
    inv_rotate_vector, inv_transform_point, length_squared, lerp, line_distance,
    make_matrix_from_quat, max_float, min_float, min_int, mul_add, mul_mv, mul_sub, mul_sv, neg,
    normalize, point_to_segment_distance, rotate_vector, segment_distance, sub, transform_point,
    Plane, Transform, Vec3,
};
use crate::math_internal::{
    arbitrary_perp, is_within_segments, make_plane_from_normal_and_point, plane_separation,
    transform_plane,
};
use crate::types::{
    Capsule, DistanceInput, HullData, LocalManifold, LocalManifoldPoint, SATCache,
    SeparatingFeature, ShapeProxy, SimplexCache, Sphere,
};

// SeparatingFeature values as u8 for matching against SATCache.type_
const T_INVALID_AXIS: u8 = SeparatingFeature::InvalidAxis as u8;
const T_FACE_AXIS_A: u8 = SeparatingFeature::FaceAxisA as u8;
const T_FACE_AXIS_B: u8 = SeparatingFeature::FaceAxisB as u8;
const T_EDGE_PAIR_AXIS: u8 = SeparatingFeature::EdgePairAxis as u8;
const T_MANUAL_FACE_AXIS_A: u8 = SeparatingFeature::ManualFaceAxisA as u8;
const T_MANUAL_FACE_AXIS_B: u8 = SeparatingFeature::ManualFaceAxisB as u8;
const T_MANUAL_EDGE_PAIR_AXIS: u8 = SeparatingFeature::ManualEdgePairAxis as u8;

#[inline]
fn is_minkowski_face_isolated(a: Vec3, b: Vec3, n: Vec3) -> bool {
    // An isolated edge (e.g. like in a capsule) defines a circle through the
    // origin on the Gauss map. So testing for overlap between this circle and
    // the arc AB simplifies to a simple plane test.
    let an = dot(a, n);
    let bn = dot(b, n);

    an * bn <= 0.0
}

// bxa = cross(b, a) and dxc = cross(d, c)
// but in practice we use the edge vector between the faces for robustness
#[inline]
fn is_minkowski_face(a: Vec3, b: Vec3, bxa: Vec3, c: Vec3, d: Vec3, dxc: Vec3) -> bool {
    // Two edges build a face on the Minkowski sum if the associated arcs ab and cd
    // intersect on the Gauss map. The associated arcs are defined by the adjacent
    // face normals of each edge.
    let cba = dot(c, bxa);
    let dba = dot(d, bxa);
    let adc = dot(a, dxc);
    let bdc = dot(b, dxc);

    cba * dba < 0.0 && adc * bdc < 0.0 && cba * bdc > 0.0
}

fn clip_segment(segment: &mut [ClipVertex; 2], plane: Plane) -> i32 {
    let mut vertex_count: i32 = 0;
    let vertex1 = segment[0];
    let vertex2 = segment[1];

    let distance1 = plane_separation(plane, vertex1.position);
    let distance2 = plane_separation(plane, vertex2.position);

    // If the points are behind the plane
    if distance1 <= 0.0 {
        segment[vertex_count as usize] = vertex1;
        vertex_count += 1;
    }
    if distance2 <= 0.0 {
        segment[vertex_count as usize] = vertex2;
        vertex_count += 1;
    }

    // If the points are on different sides of the plane
    if distance1 * distance2 < 0.0 {
        // Find intersection point of edge and plane
        let t = distance1 / (distance1 - distance2);
        segment[vertex_count as usize].position =
            add(mul_sv(1.0 - t, vertex1.position), mul_sv(t, vertex2.position));
        segment[vertex_count as usize].pair = if distance1 > 0.0 { vertex1.pair } else { vertex2.pair };
        vertex_count += 1;
    }

    vertex_count
}

fn clip_segment_to_hull_face(segment: &mut [ClipVertex; 2], hull: &HullData, ref_face: i32) -> i32 {
    let faces = &hull.faces;
    let planes = &hull.planes;
    let edges = &hull.edges;
    let points = &hull.points;

    let ref_plane = *hull_at(planes, ref_face as usize);

    let face = *hull_at(faces, ref_face as usize);

    let mut edge_index = face.edge as i32;

    loop {
        let edge = *hull_at(edges, edge_index as usize);
        let next_edge_index = edge.next as i32;
        let next = *hull_at(edges, next_edge_index as usize);

        let vertex1 = *hull_at(points, edge.origin as usize);
        let vertex2 = *hull_at(points, next.origin as usize);
        let tangent = normalize(sub(vertex2, vertex1));
        let binormal = cross(tangent, ref_plane.normal);

        let point_count = clip_segment(segment, make_plane_from_normal_and_point(binormal, vertex1));
        if point_count < 2 {
            return 0;
        }

        edge_index = next_edge_index;
        if edge_index == face.edge as i32 {
            break;
        }
    }

    2
}

fn query_face_direction_hull_and_capsule(hull: &HullData, capsule: &Capsule, capsule_transform: Transform) -> FaceQuery {
    let mut max_face_index: i32 = -1;
    let mut max_vertex_index: i32 = -1;
    let mut max_face_separation = -f32::MAX;
    let planes = &hull.planes;

    let capsule_points = [
        transform_point(capsule_transform, capsule.center1),
        transform_point(capsule_transform, capsule.center2),
    ];

    for face_index in 0..hull.face_count() {
        let plane = planes[face_index as usize];

        let vertex_index = crate::distance::get_point_support(&capsule_points, 2, neg(plane.normal));
        let support = capsule_points[vertex_index as usize];
        let separation = plane_separation(plane, support);
        if separation > max_face_separation {
            max_vertex_index = vertex_index;
            max_face_index = face_index;
            max_face_separation = separation;
        }
    }

    FaceQuery {
        separation: max_face_separation,
        face_index: (max_face_index as u8) as i32,
        vertex_index: (max_vertex_index as u8) as i32,
    }
}

/// Hull-topology indexing for the SAT hot loops. Indices come from the
/// hull's own connectivity (edge.origin / edge.face / twin, and support
/// results), which is validated at construction (see hull.rs
/// is_valid_hull_impl and the create_hull asserts) and immutable afterwards
/// (Arc<HullData>). With the `unchecked-hulls` feature the release-build
/// bounds checks are elided — C's indexing is checkless here, and these
/// data-dependent checks are the measured residue on hull-heavy scenes.
/// Debug builds always verify, so every `cargo test` run exercises the
/// contract. Without the feature this is ordinary checked indexing.
#[inline(always)]
pub(crate) fn hull_at<T>(slice: &[T], i: usize) -> &T {
    #[cfg(feature = "unchecked-hulls")]
    {
        debug_assert!(i < slice.len());
        // SAFETY: hull topology invariants, validated at construction (see
        // doc comment above); debug builds assert.
        unsafe { slice.get_unchecked(i) }
    }
    #[cfg(not(feature = "unchecked-hulls"))]
    {
        &slice[i]
    }
}

// Standalone like C (see the note on collide_hulls).
#[inline(never)]
fn query_face_directions(hull_a: &HullData, hull_b: &HullData, relative_transform: Transform) -> FaceQuery {
    // We perform all computations in local space of the second hull
    let transform = invert_transform(relative_transform);
    let planes_a = &hull_a.planes;
    let points_b = &hull_b.points;

    let mut max_face_index: i32 = -1;
    let mut max_vertex_index: i32 = -1;
    let mut max_face_separation = -f32::MAX;

    for face_index in 0..hull_a.face_count() {
        let plane = transform_plane(transform, *hull_at(planes_a, face_index as usize));

        let vertex_index = crate::hull::find_hull_support_vertex(hull_b, neg(plane.normal));
        let support = *hull_at(points_b, vertex_index as usize);
        let separation = plane_separation(plane, support);
        if separation > max_face_separation {
            max_face_index = face_index;
            max_vertex_index = vertex_index;
            max_face_separation = separation;
        }
    }

    FaceQuery {
        separation: max_face_separation,
        face_index: (max_face_index as u8) as i32,
        vertex_index: (max_vertex_index as u8) as i32,
    }
}

fn query_edge_direction_hull_and_capsule(hull: &HullData, capsule: &Capsule, capsule_transform: Transform) -> EdgeQuery {
    // Find axis of minimum penetration
    let mut max_separation = -f32::MAX;
    let mut max_index1: i32 = -1;
    let mut max_index2: i32 = -1;

    // We perform all computations in local space of the hull
    let p1 = transform_point(capsule_transform, capsule.center1);
    let q1 = transform_point(capsule_transform, capsule.center2);
    let e1 = sub(q1, p1);

    let edges = &hull.edges;
    let points = &hull.points;
    let planes = &hull.planes;

    let mut index: i32 = 0;
    while index < hull.edge_count() {
        let edge = *hull_at(edges, index as usize);
        let twin = *hull_at(edges, index as usize + 1);
        b3_assert!(edge.twin as i32 == index + 1 && twin.twin as i32 == index);

        let p2 = *hull_at(points, edge.origin as usize);
        let q2 = *hull_at(points, twin.origin as usize);
        let e2 = sub(q2, p2);

        let u2 = hull_at(planes, edge.face as usize).normal;
        let v2 = hull_at(planes, twin.face as usize).normal;

        if is_minkowski_face_isolated(u2, v2, e1) {
            // We can pass any point on the edge and choose
            // the edge centers for better numerical precision.
            let c1 = mul_sv(0.5, add(q1, p1));
            let c2 = hull.center;
            let separation = edge_edge_separation(q1, e1, c1, q2, e2, c2);
            if separation > max_separation {
                // Note: We don't exit early if we find a separating axis here since we want to
                // find the best one for caching and account for the convex radius later.
                max_separation = separation;
                max_index1 = 0;
                max_index2 = index;
            }
        }

        index += 2;
    }

    // Save result
    EdgeQuery {
        separation: max_separation,
        index_a: (max_index1 as u8) as i32,
        index_b: (max_index2 as u8) as i32,
    }
}

fn query_edge_directions(hull_a: &HullData, hull_b: &HullData, transform_b_to_a: Transform) -> EdgeQuery {
    // Find axis of minimum penetration
    let mut max_separation = -f32::MAX;
    let mut max_index_a = NULL_INDEX;
    let mut max_index_b = NULL_INDEX;

    let edges_a = &hull_a.edges;
    let points_a = &hull_a.points;
    let planes_a = &hull_a.planes;
    let edges_b = &hull_b.edges;
    let points_b = &hull_b.points;
    let planes_b = &hull_b.planes;

    // Work in frame A
    let matrix = make_matrix_from_quat(transform_b_to_a.q);

    // Arranged to minimize transform operations
    let mut index_b: i32 = 0;
    while index_b < hull_b.edge_count() {
        let edge_b = *hull_at(edges_b, index_b as usize);
        let twin_b = *hull_at(edges_b, index_b as usize + 1);
        b3_assert!(edge_b.twin as i32 == index_b + 1 && twin_b.twin as i32 == index_b);

        let mut q_b = *hull_at(points_b, twin_b.origin as usize);
        let e_b = mul_mv(matrix, sub(q_b, *hull_at(points_b, edge_b.origin as usize)));
        q_b = add(mul_mv(matrix, q_b), transform_b_to_a.p);

        let u_b = mul_mv(matrix, hull_at(planes_b, edge_b.face as usize).normal);
        let v_b = mul_mv(matrix, hull_at(planes_b, twin_b.face as usize).normal);

        let mut index_a: i32 = 0;
        while index_a < hull_a.edge_count() {
            let edge_a = *hull_at(edges_a, index_a as usize);
            let twin_a = *hull_at(edges_a, index_a as usize + 1);
            b3_assert!(edge_a.twin as i32 == index_a + 1 && twin_a.twin as i32 == index_a);

            let q_a = *hull_at(points_a, twin_a.origin as usize);
            let e_a = sub(q_a, *hull_at(points_a, edge_a.origin as usize));
            let u_a = hull_at(planes_a, edge_a.face as usize).normal;
            let v_a = hull_at(planes_a, twin_a.face as usize).normal;

            let is_minkowski;
            {
                // Two edges build a face on the Minkowski sum if the associated arcs AB and CD
                // intersect on the Gauss map. The associated arcs are defined by the adjacent
                // face normals of each edge.
                let cba = dot(u_b, e_a);
                let dba = dot(v_b, e_a);
                let adc = -dot(u_a, e_b);
                let bdc = -dot(v_a, e_b);

                is_minkowski = cba * dba < 0.0 && adc * bdc < 0.0 && cba * bdc > 0.0;
            }

            if is_minkowski {
                let center_a = hull_a.center;
                let center_b = transform_point(transform_b_to_a, hull_b.center);
                let separation = edge_edge_separation(q_a, e_a, center_a, q_b, e_b, center_b);

                if separation > max_separation {
                    // Continues to find the maximum separating axis
                    max_separation = separation;
                    max_index_a = index_a;
                    max_index_b = index_b;
                }
            }

            index_a += 2;
        }

        index_b += 2;
    }

    EdgeQuery {
        separation: max_separation,
        index_a: max_index_a,
        index_b: max_index_b,
    }
}

// Reduce the manifold points to a maximum of 4 points.
// Note: this modifies the input point array to improve performance
fn reduce_manifold_points(manifold: &mut LocalManifold, capacity: i32, points: &mut [LocalManifoldPoint], count: i32) {
    if capacity < 4 {
        return;
    }

    let mut count = count;

    if count <= 4 {
        manifold.points.clear();
        for i in 0..count {
            manifold.points.push(points[i as usize]);
        }

        return;
    }

    let normal = manifold.normal;
    let speculative_distance = speculative_distance();
    let tol_sqr = speculative_distance * speculative_distance;

    // This bias is very important for contact point consistency across time steps.
    // It creates a pecking order to avoid flickering between candidates with similar scores.
    let bias = 0.95;

    // Step 1: find extreme point that is touching
    let mut best_index = NULL_INDEX;
    let mut best_score = -f32::MAX;

    // Arbitrary tangent direction
    let search_direction = arbitrary_perp(normal);
    for index in 0..count {
        let pt = &points[index as usize];

        if pt.separation > speculative_distance {
            continue;
        }

        // The deeper the better
        let score = -pt.separation + dot(search_direction, pt.point);
        if bias * score > best_score {
            best_index = index;
            best_score = score;
        }
    }

    b3_validate!(0 <= best_index && best_index < count);
    if best_index == NULL_INDEX {
        manifold.points.clear();
        return;
    }

    manifold.points.clear();
    manifold.points.push(points[best_index as usize]);

    // Remove best point from array
    points[best_index as usize] = points[count as usize - 1];
    count -= 1;

    let a = manifold.points[0].point;

    // Step 2: Find farthest point in 2D
    best_score = 0.0;
    best_index = NULL_INDEX;
    let mut max_distance_squared = 0.0;

    for index in 0..count {
        let p = points[index as usize].point;
        let d = sub(p, a);
        let v = mul_sub(d, dot(d, normal), normal);
        let distance_squared = length_squared(v);
        max_distance_squared = max_float(max_distance_squared, distance_squared);
        let separation = max_float(0.0, -points[index as usize].separation);
        let score = distance_squared + 4.0 * separation * separation;
        if bias * score > best_score {
            best_score = score;
            best_index = index;
        }
    }
    let _ = max_distance_squared;

    if best_score < tol_sqr {
        return;
    }

    b3_assert!(0 <= best_index && best_index < count);
    manifold.points.push(points[best_index as usize]);

    // Remove best point from array
    points[best_index as usize] = points[count as usize - 1];
    count -= 1;

    let b = manifold.points[1].point;

    // Step 3: Find the point with the maximum triangular area
    best_score = tol_sqr;
    best_index = NULL_INDEX;
    let mut best_signed_area = 0.0;
    let ba = sub(b, a);
    for index in 0..count {
        let p = points[index as usize].point;
        let signed_area = dot(normal, cross(ba, sub(p, a)));
        let score = abs_float(signed_area);
        if bias * score >= best_score {
            best_score = score;
            best_index = index;
            best_signed_area = signed_area;
        }
    }

    if best_index == NULL_INDEX {
        return;
    }

    b3_assert!(best_index != NULL_INDEX);

    manifold.points.push(points[best_index as usize]);
    points[best_index as usize] = points[count as usize - 1];
    count -= 1;

    let c = manifold.points[2].point;

    // Step 4: get the point that adds the most area outside the current triangle
    best_score = tol_sqr;
    best_index = NULL_INDEX;
    let sign = if best_signed_area < 0.0 { -1.0 } else { 1.0 };
    for index in 0..count {
        let p = points[index as usize].point;
        let u1 = sign * dot(normal, cross(sub(p, a), ba));
        let u2 = sign * dot(normal, cross(sub(p, b), sub(c, b)));
        let u3 = sign * dot(normal, cross(sub(p, c), sub(a, c)));
        let score = max_float(u1, max_float(u2, u3));

        if bias * score > best_score {
            best_score = score;
            best_index = index;
        }
    }

    if best_index != NULL_INDEX {
        manifold.points.push(points[best_index as usize]);
    }
}

/// Collide two spheres.
pub fn collide_spheres(manifold: &mut LocalManifold, capacity: i32, sphere_a: &Sphere, sphere_b: &Sphere, transform_b_to_a: Transform) {
    // Note: the C version relies on the caller to zero pointCount before this
    // call; the port clears here for consistency with the other collide functions.
    manifold.points.clear();

    if capacity == 0 {
        return;
    }

    // Work in shapeB coordinates
    let center1 = sphere_a.center;
    let center2 = transform_point(transform_b_to_a, sphere_b.center);

    let total_radius = sphere_a.radius + sphere_b.radius;
    let offset = sub(center2, center1);
    let distance_sq = length_squared(offset);

    if distance_sq > total_radius * total_radius {
        // We found a separating axis
        return;
    }

    let mut normal = Vec3 { x: 0.0, y: 1.0, z: 0.0 };
    let distance = distance_sq.sqrt();
    if distance * distance > 1000.0 * f32::MIN_POSITIVE {
        normal = mul_sv(1.0 / distance, offset);
    }

    // Contact at the midpoint
    // 0.5 * ( ((c1 + rA*n) + c2) - rB*n )
    let point = mul_sv(
        0.5,
        mul_sub(add(mul_add(center1, sphere_a.radius, normal), center2), sphere_b.radius, normal),
    );

    // Manifold in frame B
    manifold.normal = normal;

    let mut pt = LocalManifoldPoint::default();
    pt.point = point;
    pt.separation = distance - total_radius;
    pt.pair = FEATURE_PAIR_SINGLE;
    manifold.points.push(pt);
}

/// Collide a capsule and a sphere.
pub fn collide_capsule_and_sphere(manifold: &mut LocalManifold, capacity: i32, capsule_a: &Capsule, sphere_b: &Sphere, transform_b_to_a: Transform) {
    manifold.points.clear();

    if capacity < 1 {
        return;
    }

    // Work in shape B coordinates
    let center = transform_point(transform_b_to_a, sphere_b.center);
    let center1 = capsule_a.center1;
    let center2 = capsule_a.center2;

    let total_radius = sphere_b.radius + capsule_a.radius;

    let closest_point = point_to_segment_distance(center1, center2, center);
    let offset = sub(center, closest_point);
    let distance_sq = length_squared(offset);

    if distance_sq > total_radius * total_radius {
        // We found a separating axis
        return;
    }

    let mut normal = Vec3 { x: 0.0, y: 1.0, z: 0.0 };
    let distance = distance_sq.sqrt();
    if distance * distance > 1000.0 * f32::MIN_POSITIVE {
        normal = mul_sv(1.0 / distance, offset);
    }

    // Contact at the midpoint
    // 0.5 * (((center - sB*n) + closestPoint) + cA*n)
    let point = mul_sv(
        0.5,
        mul_add(add(mul_sub(center, sphere_b.radius, normal), closest_point), capsule_a.radius, normal),
    );

    // Manifold in frame B
    manifold.normal = normal;

    let mut pt = LocalManifoldPoint::default();
    pt.point = point;
    pt.separation = distance - total_radius;
    pt.pair = FEATURE_PAIR_SINGLE;
    manifold.points.push(pt);
}

/// Collide a hull and a sphere.
pub fn collide_hull_and_sphere(manifold: &mut LocalManifold, capacity: i32, hull_a: &HullData, sphere_b: &Sphere, transform_b_to_a: Transform, cache: &mut SimplexCache) {
    manifold.points.clear();

    if capacity == 0 {
        return;
    }

    let center = transform_point(transform_b_to_a, sphere_b.center);

    let speculative_distance = speculative_distance();

    // Work in shapeA coordinates

    let center_slice = [center];
    let distance_input = DistanceInput {
        proxy_a: ShapeProxy { points: &hull_a.points, radius: 0.0 },
        proxy_b: ShapeProxy { points: &center_slice, radius: 0.0 },
        transform: Transform::IDENTITY,
        use_radii: false,
    };

    let radius_a = 0.0;
    let radius_b = sphere_b.radius;
    let radius = radius_a + radius_b;

    let distance_output = crate::distance::shape_distance(&distance_input, cache, None);

    if distance_output.distance > radius + speculative_distance {
        // We found a separating axis
        *cache = SimplexCache::default();
        return;
    }

    if distance_output.distance > 100.0 * f32::EPSILON {
        // Shallow penetration
        let normal = normalize(sub(distance_output.point_b, distance_output.point_a));

        // cA is the projection of the sphere center onto to the hull (pointA if radiusA == 0).
        let c_a = mul_add(center, radius_a - dot(sub(center, distance_output.point_a), normal), normal);

        // cB is the deepest point on the sphere with respect to the reference f
        let c_b = mul_sub(center, radius_b, normal);

        let point = lerp(c_a, c_b, 0.5);

        // Manifold in frame A
        manifold.normal = normal;

        let mut pt = LocalManifoldPoint::default();
        pt.point = point;
        pt.separation = distance_output.distance - radius;
        pt.pair = FEATURE_PAIR_SINGLE;
        manifold.points.push(pt);
    } else {
        // Deep penetration
        let mut best_index: i32 = -1;
        let mut best_distance = -f32::MAX;
        let planes = &hull_a.planes;

        for index in 0..hull_a.face_count() {
            let plane = planes[index as usize];

            let distance = plane_separation(plane, center);
            if distance > best_distance {
                best_index = index;
                best_distance = distance;
            }
        }
        b3_assert!(best_index >= 0);

        let normal = planes[best_index as usize].normal;

        // cA is the projection of the sphere center onto to the hull
        let c_a = mul_add(center, radius_a - dot(sub(center, distance_output.point_a), normal), normal);

        // cB is the deepest point on the sphere with respect to the reference f
        let c_b = mul_sub(center, radius_b, normal);

        let point = lerp(c_a, c_b, 0.5);

        // Manifold in frame A
        manifold.normal = normal;

        let mut pt = LocalManifoldPoint::default();
        pt.point = point;
        pt.separation = best_distance - radius;
        pt.pair = FEATURE_PAIR_SINGLE;
        manifold.points.push(pt);
    }
}

/// Collide two capsules.
pub fn collide_capsules(manifold: &mut LocalManifold, capacity: i32, capsule_a: &Capsule, capsule_b: &Capsule, transform_b_to_a: Transform) {
    manifold.points.clear();

    if capacity < 2 {
        return;
    }

    // Work in shapeA coordinates
    let center_a1 = capsule_a.center1;
    let center_a2 = capsule_a.center2;
    let center_b1 = transform_point(transform_b_to_a, capsule_b.center1);
    let center_b2 = transform_point(transform_b_to_a, capsule_b.center2);

    let radius = capsule_a.radius + capsule_b.radius;
    let max_distance = radius + speculative_distance();

    let result = segment_distance(center_a1, center_a2, center_b1, center_b2);
    let offset = sub(result.point2, result.point1);
    let distance_squared = length_squared(offset);
    let linear_slop = linear_slop();
    let min_distance = 0.01 * linear_slop;

    if distance_squared > max_distance * max_distance || distance_squared < min_distance * min_distance {
        // We found a separating axis
        return;
    }

    let mut length_a = 0.0;
    let segment_a = sub(center_a2, center_a1);
    let edge_a = get_length_and_normalize(&mut length_a, segment_a);
    if length_a < min_capsule_length() {
        return;
    }

    let mut length_b = 0.0;
    let segment_b = sub(center_b2, center_b1);
    let edge_b = get_length_and_normalize(&mut length_b, segment_b);
    if length_b < min_capsule_length() {
        return;
    }

    // Parallel edges: |eA x eB| = sin(alpha)
    const ALPHA_TOL: f32 = 0.05;
    const ALPHA_TOL_SQR: f32 = ALPHA_TOL * ALPHA_TOL;
    let axis = cross(edge_a, edge_b);

    // Try to create two contact points if the capsules are nearly parallel
    if length_squared(axis) < ALPHA_TOL_SQR {
        // Clip segment B against side planes of segment A

        // Sides planes of A
        let planes_a = [
            Plane { normal: neg(edge_a), offset: -dot(edge_a, capsule_a.center1) },
            Plane { normal: edge_a, offset: dot(edge_a, capsule_a.center2) },
        ];

        // Clip points for B
        let mut vertices_b = [ClipVertex::default(); 2];
        vertices_b[0].position = center_b1;
        vertices_b[0].separation = 0.0;
        vertices_b[0].pair = make_feature_pair(FeatureOwner::ShapeA, 0, FeatureOwner::ShapeA, 0);
        vertices_b[1].position = center_b2;
        vertices_b[1].separation = 0.0;
        vertices_b[1].pair = make_feature_pair(FeatureOwner::ShapeA, 1, FeatureOwner::ShapeA, 1);

        let mut point_count = clip_segment(&mut vertices_b, planes_a[0]);
        if point_count == 2 {
            point_count = clip_segment(&mut vertices_b, planes_a[1]);
        }

        if point_count == 2 {
            // Closest points on A to the clipped points on B.
            let closest_point1 = point_to_segment_distance(center_a1, center_a2, vertices_b[0].position);
            let closest_point2 = point_to_segment_distance(center_a1, center_a2, vertices_b[1].position);

            let distance1 = vec_distance(closest_point1, vertices_b[0].position);
            let distance2 = vec_distance(closest_point2, vertices_b[1].position);
            if distance1 <= radius && distance2 <= radius {
                if distance1 < min_distance || distance2 < min_distance {
                    // Avoid divide by zero
                    return;
                }

                let normal1 = mul_sv(1.0 / distance1, sub(vertices_b[0].position, closest_point1));
                let normal2 = mul_sv(1.0 / distance2, sub(vertices_b[1].position, closest_point2));
                let normal = normalize(add(normal1, normal2));
                let radius_a = capsule_a.radius;
                let radius_b = capsule_b.radius;

                // Contact is at the midpoint: 0.5 * (((vB.pos + rA*nK) + cP) - rB*n)
                let point1 = mul_sv(
                    0.5,
                    mul_sub(add(mul_add(vertices_b[0].position, radius_a, normal1), closest_point1), radius_b, normal),
                );
                let point2 = mul_sv(
                    0.5,
                    mul_sub(add(mul_add(vertices_b[1].position, radius_a, normal2), closest_point2), radius_b, normal),
                );

                // Manifold in frame A
                manifold.normal = normal;

                let mut pt1 = LocalManifoldPoint::default();
                pt1.point = point1;
                pt1.separation = distance1 - radius;
                pt1.pair = vertices_b[0].pair;
                manifold.points.push(pt1);

                let mut pt2 = LocalManifoldPoint::default();
                pt2.point = point2;
                pt2.separation = distance2 - radius;
                pt2.pair = vertices_b[1].pair;
                manifold.points.push(pt2);

                return;
            }
        }
    }

    let mut distance = 0.0;
    let normal = get_length_and_normalize(&mut distance, offset);
    // Contact at the midpoint 0.5 * (((p1 + rA*n) + p2) - rB*n)
    let point = mul_sv(
        0.5,
        mul_sub(add(mul_add(result.point1, capsule_a.radius, normal), result.point2), capsule_b.radius, normal),
    );

    // Manifold in frame A
    manifold.normal = normal;

    let mut pt = LocalManifoldPoint::default();
    pt.point = point;
    pt.separation = distance - radius;
    pt.pair = FEATURE_PAIR_SINGLE;
    manifold.points.push(pt);
}

fn build_hull_face_and_capsule_contact(manifold: &mut LocalManifold, hull_a: &HullData, capsule_b: &Capsule, transform_b_to_a: Transform, query: FaceQuery) -> bool {
    // Work in shapeA coordinates
    let planes = &hull_a.planes;

    // Clip the capsule edge against the side planes of the reference face
    let ref_face = query.face_index;
    let ref_plane = planes[ref_face as usize];

    let mut segment_b = [ClipVertex::default(); 2];
    segment_b[0].position = transform_point(transform_b_to_a, capsule_b.center1);
    segment_b[0].separation = 0.0;
    segment_b[0].pair = make_feature_pair(FeatureOwner::ShapeA, 0, FeatureOwner::ShapeA, 0);
    segment_b[1].position = transform_point(transform_b_to_a, capsule_b.center2);
    segment_b[1].separation = 0.0;
    segment_b[1].pair = make_feature_pair(FeatureOwner::ShapeA, 1, FeatureOwner::ShapeA, 1);

    let point_count = clip_segment_to_hull_face(&mut segment_b, hull_a, ref_face);
    if point_count < 2 {
        return false;
    }

    let distance1 = plane_separation(ref_plane, segment_b[0].position);
    let distance2 = plane_separation(ref_plane, segment_b[1].position);
    let speculative_distance = speculative_distance();

    if distance1 <= speculative_distance || distance2 <= speculative_distance {
        let normal = ref_plane.normal;
        let point1 = mul_sub(segment_b[0].position, 0.5 * (distance1 + capsule_b.radius), normal);
        let point2 = mul_sub(segment_b[1].position, 0.5 * (distance2 + capsule_b.radius), normal);

        // Manifold in frame A
        manifold.normal = normal;
        manifold.points.clear();

        let mut pt1 = LocalManifoldPoint::default();
        pt1.point = point1;
        pt1.separation = distance1 - capsule_b.radius;
        pt1.pair = segment_b[0].pair;
        manifold.points.push(pt1);

        let mut pt2 = LocalManifoldPoint::default();
        pt2.point = point2;
        pt2.separation = distance2 - capsule_b.radius;
        pt2.pair = segment_b[1].pair;
        manifold.points.push(pt2);

        return true;
    }

    false
}

#[inline]
fn deepest_point_separation(manifold: &LocalManifold) -> f32 {
    // Deepest point
    let mut min_separation = f32::MAX;
    let point_count = manifold.point_count();
    for i in 0..point_count {
        min_separation = min_float(min_separation, manifold.points[i as usize].separation);
    }

    min_separation
}

fn build_hull_and_capsule_edge_contact(manifold: &mut LocalManifold, capacity: i32, hull_a: &HullData, capsule_b: &Capsule, transform_b_to_a: Transform, query: EdgeQuery) -> bool {
    if capacity < 1 {
        return false;
    }

    // Work in shapeA coordinates

    let pc = transform_point(transform_b_to_a, capsule_b.center1);
    let qc = transform_point(transform_b_to_a, capsule_b.center2);
    let ec = sub(qc, pc);

    let edges = &hull_a.edges;
    let points = &hull_a.points;

    let edge2 = edges[query.index_b as usize];
    let twin2 = edges[edge2.twin as usize];
    let ch = hull_a.center;
    let ph = points[edge2.origin as usize];
    let qh = points[twin2.origin as usize];
    let eh = sub(qh, ph);

    let mut normal = cross(ec, eh);
    normal = normalize(normal);

    // Normal should point outward from hull
    if dot(normal, sub(ph, ch)) < 0.0 {
        normal = neg(normal);
    }

    let result = line_distance(ph, eh, pc, ec);

    if !is_within_segments(&result) {
        // closest point beyond end points
        return false;
    }

    let point = mul_sv(0.5, add(mul_sub(result.point1, capsule_b.radius, normal), result.point2));

    let separation = dot(normal, sub(result.point2, result.point1));
    b3_validate!(abs_float(separation - query.separation) < linear_slop());

    // Manifold in frame A
    manifold.normal = normal;
    manifold.points.clear();

    let mut pt = LocalManifoldPoint::default();
    pt.point = point;
    pt.separation = separation - capsule_b.radius;
    pt.pair = make_feature_pair(FeatureOwner::ShapeA, query.index_a, FeatureOwner::ShapeB, query.index_b);
    manifold.points.push(pt);
    true
}

/// Collide a hull and a capsule.
pub fn collide_hull_and_capsule(manifold: &mut LocalManifold, capacity: i32, hull_a: &HullData, capsule_b: &Capsule, transform_b_to_a: Transform, cache: &mut SimplexCache) {
    manifold.points.clear();

    if capacity < 2 {
        return;
    }

    // Work in shapeA coordinates
    let capsule_points = [capsule_b.center1, capsule_b.center2];
    let distance_input = DistanceInput {
        proxy_a: ShapeProxy { points: &hull_a.points, radius: 0.0 },
        proxy_b: ShapeProxy { points: &capsule_points, radius: 0.0 },
        transform: transform_b_to_a,
        use_radii: false,
    };

    let distance_output = crate::distance::shape_distance(&distance_input, cache, None);
    let speculative_distance = speculative_distance();

    if distance_output.distance > capsule_b.radius + speculative_distance {
        // We found a separating axis
        *cache = SimplexCache::default();
        return;
    }

    if distance_output.distance > 100.0 * f32::EPSILON {
        let planes = &hull_a.planes;

        // Shallow penetration
        let delta = distance_output.normal;
        let ref_face = crate::hull::find_hull_support_face(hull_a, delta);
        let ref_plane = planes[ref_face as usize];

        // Try to create two contact points if closest
        // points difference is nearly parallel to face normal
        const K_TOLERANCE: f32 = 0.998;
        if abs_float(dot(ref_plane.normal, delta)) > K_TOLERANCE {
            // Clip capsule segment against side planes of reference face
            let mut vertices_b = [ClipVertex::default(); 2];
            vertices_b[0].position = transform_point(transform_b_to_a, capsule_b.center1);
            vertices_b[0].separation = 0.0;
            vertices_b[0].pair = make_feature_pair(FeatureOwner::ShapeA, 0, FeatureOwner::ShapeA, 0);
            vertices_b[1].position = transform_point(transform_b_to_a, capsule_b.center2);
            vertices_b[1].separation = 0.0;
            vertices_b[1].pair = make_feature_pair(FeatureOwner::ShapeA, 1, FeatureOwner::ShapeA, 1);

            let point_count = clip_segment_to_hull_face(&mut vertices_b, hull_a, ref_face);

            if point_count == 2 {
                let distance1 = plane_separation(ref_plane, vertices_b[0].position);
                let distance2 = plane_separation(ref_plane, vertices_b[1].position);
                if distance1 <= capsule_b.radius + speculative_distance
                    || distance2 <= capsule_b.radius + speculative_distance
                {
                    let normal = ref_plane.normal;
                    let point1 = mul_sub(vertices_b[0].position, 0.5 * (capsule_b.radius + distance1), normal);
                    let point2 = mul_sub(vertices_b[1].position, 0.5 * (capsule_b.radius + distance2), normal);

                    // Manifold in frame A
                    manifold.normal = normal;
                    manifold.points.clear();

                    let mut pt1 = LocalManifoldPoint::default();
                    pt1.point = point1;
                    pt1.separation = distance1 - capsule_b.radius;
                    pt1.pair = vertices_b[0].pair;
                    manifold.points.push(pt1);

                    let mut pt2 = LocalManifoldPoint::default();
                    pt2.point = point2;
                    pt2.separation = distance2 - capsule_b.radius;
                    pt2.pair = vertices_b[1].pair;
                    manifold.points.push(pt2);

                    return;
                }
            }
        }

        // Create contact from closest points
        let point = mul_sv(0.5, add(mul_sub(distance_output.point_a, capsule_b.radius, delta), distance_output.point_b));

        // Manifold in frame A
        manifold.normal = delta;
        manifold.points.clear();

        let mut pt = LocalManifoldPoint::default();
        pt.point = point;
        pt.separation = distance_output.distance - capsule_b.radius;
        pt.pair = FEATURE_PAIR_SINGLE;
        manifold.points.push(pt);
        return;
    }

    // Deep penetration

    let face_query = query_face_direction_hull_and_capsule(hull_a, capsule_b, transform_b_to_a);
    if face_query.separation > capsule_b.radius {
        // We found a separating axis
        return;
    }

    let edge_query = query_edge_direction_hull_and_capsule(hull_a, capsule_b, transform_b_to_a);
    if edge_query.separation > capsule_b.radius {
        // We found a separating axis
        return;
    }

    // Create face contact
    let mut face_separation = face_query.separation - capsule_b.radius;
    build_hull_face_and_capsule_contact(manifold, hull_a, capsule_b, transform_b_to_a, face_query);
    if manifold.point_count() > 1 {
        // If ( Out.PointCount <= 1 ) -> Compare with unclipped separation
        // If ( Out.PointCount > 1 ) -> Be aggressive and compare with clipped separation
        // Face contact can be empty if it does not realize the axis of minimum penetration
        face_separation = deepest_point_separation(manifold);
    }
    b3_validate!(face_separation <= 0.0);

    // Face contact can be empty if it does not realize the axis of minimum penetration.
    // Create edge contact if face contact fails or edge contact is significantly better!
    const K_REL_EDGE_TOLERANCE: f32 = 0.90;
    let k_abs_tolerance = 0.5 * linear_slop();
    let edge_separation = edge_query.separation - capsule_b.radius;
    if manifold.point_count() == 0 || edge_separation > K_REL_EDGE_TOLERANCE * face_separation + k_abs_tolerance {
        // Edge contact
        build_hull_and_capsule_edge_contact(manifold, capacity, hull_a, capsule_b, transform_b_to_a, edge_query);
    }
}

fn build_polygon(out: &mut [ClipVertex], transform: Transform, hull: &HullData, inc_face: i32, ref_plane: Plane) -> i32 {
    let faces = &hull.faces;
    let edges = &hull.edges;
    let points = &hull.points;

    let face = *hull_at(faces, inc_face as usize);
    let mut edge_index = face.edge as i32;
    b3_assert!(hull_at(edges, edge_index as usize).face as i32 == inc_face);

    let mut out_count: i32 = 0;

    let matrix = make_matrix_from_quat(transform.q);

    loop {
        let edge = *hull_at(edges, edge_index as usize);

        let next_edge_index = edge.next as i32;
        let next = *hull_at(edges, next_edge_index as usize);

        let mut vertex = ClipVertex::default();
        vertex.position = add(mul_mv(matrix, *hull_at(points, next.origin as usize)), transform.p);
        vertex.separation = plane_separation(ref_plane, vertex.position);
        vertex.pair = make_feature_pair(FeatureOwner::ShapeB, edge_index, FeatureOwner::ShapeB, next_edge_index);

        out[out_count as usize] = vertex;
        out_count += 1;

        edge_index = next_edge_index;

        if !(edge_index != face.edge as i32 && out_count < MAX_CLIP_POINTS as i32) {
            break;
        }
    }

    b3_validate!(validate_polygon(out, out_count));

    out_count
}

fn build_face_a_contact(manifold: &mut LocalManifold, capacity: i32, hull_a: &HullData, hull_b: &HullData, transform_b_to_a: Transform, query: FaceQuery, cache: &mut SATCache) -> bool {
    let faces_a = &hull_a.faces;
    let edges_a = &hull_a.edges;
    let planes_a = &hull_a.planes;
    let points_a = &hull_a.points;

    // Reference face
    let ref_face = query.face_index;
    let ref_plane = *hull_at(planes_a, ref_face as usize);

    // Find incident face
    let ref_normal_in_b = inv_rotate_vector(transform_b_to_a.q, ref_plane.normal);
    let inc_face = find_incident_face(hull_b, ref_normal_in_b, query.vertex_index);

    // Build clip polygon from incident face in frame A
    let mut buffer1 = [ClipVertex::default(); MAX_CLIP_POINTS];
    let mut buffer2 = [ClipVertex::default(); MAX_CLIP_POINTS];
    let mut point_count = build_polygon(&mut buffer1, transform_b_to_a, hull_b, inc_face, ref_plane);

    // Clip incident face against side planes of reference face
    let mut input: &mut [ClipVertex] = &mut buffer1;
    let mut output: &mut [ClipVertex] = &mut buffer2;

    let face = *hull_at(faces_a, ref_face as usize);
    let mut edge_index = face.edge as i32;

    loop {
        let edge = *hull_at(edges_a, edge_index as usize);
        let next_edge_index = edge.next as i32;
        let next = *hull_at(edges_a, next_edge_index as usize);
        let vertex1 = *hull_at(points_a, edge.origin as usize);
        let vertex2 = *hull_at(points_a, next.origin as usize);
        let tangent = normalize(sub(vertex2, vertex1));
        let binormal = cross(tangent, ref_plane.normal);

        let clip_plane = make_plane_from_normal_and_point(binormal, vertex1);

        point_count = clip_polygon(output, input, point_count, clip_plane, edge_index, ref_plane);
        b3_assert!(point_count <= MAX_CLIP_POINTS as i32);

        std::mem::swap(&mut output, &mut input);

        if point_count < 3 {
            *cache = SATCache::default();
            return false;
        }

        edge_index = next_edge_index;
        if edge_index == face.edge as i32 {
            break;
        }
    }

    let point_count = min_int(point_count, MAX_CLIP_POINTS as i32);

    let mut points = [LocalManifoldPoint::default(); MAX_CLIP_POINTS];
    let mut min_separation = f32::MAX;

    manifold.normal = ref_plane.normal;

    for i in 0..point_count {
        let clip_point = &input[i as usize];
        let pt = &mut points[i as usize];
        *pt = LocalManifoldPoint::default();

        // Using the half-way point keeps the points in the same position when swapping
        // reference face from A to B.
        let point = mul_sub(clip_point.position, 0.5 * clip_point.separation, ref_plane.normal);

        // Old way of pushing onto the reference face:
        // point = clip_point.position - clip_point.separation * ref_plane.normal

        pt.point = point;
        pt.separation = clip_point.separation;
        pt.pair = clip_point.pair;

        min_separation = min_float(min_separation, clip_point.separation);
    }

    if min_separation >= speculative_distance() {
        *cache = SATCache::default();
        return false;
    }

    reduce_manifold_points(manifold, capacity, &mut points, point_count);

    // Save cache
    cache.separation = min_separation;
    cache.type_ = SeparatingFeature::FaceAxisA as u8;
    cache.index_a = query.face_index as u8;
    cache.index_b = query.vertex_index as u8;

    true
}

fn build_face_b_contact(manifold: &mut LocalManifold, capacity: i32, hull_a: &HullData, hull_b: &HullData, transform_b_to_a: Transform, query: FaceQuery, cache: &mut SATCache) -> bool {
    let transform_a_to_b = invert_transform(transform_b_to_a);
    let touching = build_face_a_contact(manifold, capacity, hull_b, hull_a, transform_a_to_b, query, cache);
    if !touching {
        return false;
    }

    // Results are in frame B, need to transform them into frame A
    let matrix = make_matrix_from_quat(transform_b_to_a.q);

    // Transform and flip normal so it points from A to B, even though the B has the
    // reference face.
    manifold.normal = neg(mul_mv(matrix, manifold.normal));
    cache.type_ = SeparatingFeature::FaceAxisB as u8;
    cache.index_a = query.vertex_index as u8;
    cache.index_b = query.face_index as u8;

    // Transform points from frame B to frame A.
    // Also flip the pairs to ensure correct matches.
    for i in 0..manifold.point_count() {
        let pt = &mut manifold.points[i as usize];
        pt.point = add(mul_mv(matrix, pt.point), transform_b_to_a.p);
        pt.pair = flip_pair(pt.pair);
    }

    true
}

fn build_edge_contact(manifold: &mut LocalManifold, hull_a: &HullData, hull_b: &HullData, transform_b_to_a: Transform, query: EdgeQuery, cache: &mut SATCache) -> bool {
    // Work in shapeA coordinates
    let edges_a = &hull_a.edges;
    let points_a = &hull_a.points;

    let edges_b = &hull_b.edges;
    let points_b = &hull_b.points;

    let edge_a = edges_a[query.index_a as usize];
    let twin_a = edges_a[edge_a.twin as usize];
    let center_a = hull_a.center;
    let p_a = points_a[edge_a.origin as usize];
    let q_a = points_a[twin_a.origin as usize];
    let e_a = sub(q_a, p_a);

    let edge_b = edges_b[query.index_b as usize];
    let twin_b = edges_b[edge_b.twin as usize];
    let p_b = transform_point(transform_b_to_a, points_b[edge_b.origin as usize]);
    let q_b = transform_point(transform_b_to_a, points_b[twin_b.origin as usize]);
    let e_b = sub(q_b, p_b);

    let mut normal = cross(e_a, e_b);
    normal = normalize(normal);

    if dot(normal, sub(p_a, center_a)) < 0.0 {
        normal = neg(normal);
    }

    let result = line_distance(p_a, e_a, p_b, e_b);

    if !is_within_segments(&result) {
        *cache = SATCache::default();
        return false;
    }

    // This can slide off the end from caching
    let separation = dot(normal, sub(result.point2, result.point1));

    // todo I suspect this could trip if the cache becomes invalid
    // b3_validate!(abs_float(separation - query.separation) < linear_slop());

    let point = mul_sv(0.5, add(result.point1, result.point2));

    // Result in frame A
    manifold.normal = normal;
    manifold.points.clear();

    let mut pt = LocalManifoldPoint::default();
    pt.point = point;
    pt.separation = separation;
    pt.pair = make_feature_pair(FeatureOwner::ShapeA, query.index_a, FeatureOwner::ShapeB, query.index_b);
    manifold.points.push(pt);

    // Save cache
    cache.separation = separation;
    cache.type_ = SeparatingFeature::EdgePairAxis as u8;
    cache.index_a = query.index_a as u8;
    cache.index_b = query.index_b as u8;

    true
}

/// Collide two hulls.
// Standalone like C (b3CollideHulls / b3QueryFaceDirections / the compute
// fns are separate symbols): LLVM+PGO merged the whole manifold pipeline
// into 2-3k-instruction bodies paying spills and I-cache pressure — the
// measured cost concentration on low-recycle scenes (junkyard/washer).
#[inline(never)]
pub fn collide_hulls(manifold: &mut LocalManifold, capacity: i32, hull_a: &HullData, hull_b: &HullData, transform_b_to_a: Transform, cache: &mut SATCache) {
    manifold.points.clear();

    if capacity < 4 {
        return;
    }

    // Work in shapeA coordinates
    let speculative_distance = speculative_distance();

    let linear_slop = linear_slop();
    let edges_a = &hull_a.edges;
    let planes_a = &hull_a.planes;
    let points_a = &hull_a.points;

    let edges_b = &hull_b.edges;
    let planes_b = &hull_b.planes;
    let points_b = &hull_b.points;

    // Attempt to use the cache to speed up collision
    match cache.type_ {
        T_INVALID_AXIS => {
            *cache = SATCache::default();
        }

        T_FACE_AXIS_A => {
            b3_assert!((cache.index_a as i32) < hull_a.face_count());

            // Check for separation using cached face
            let plane = planes_a[cache.index_a as usize];
            let search_direction_in_b = neg(inv_rotate_vector(transform_b_to_a.q, plane.normal));
            let vertex_index = crate::hull::find_hull_support_vertex(hull_b, search_direction_in_b);
            let support = transform_point(transform_b_to_a, points_b[vertex_index as usize]);
            let separation = plane_separation(plane, support);

            if separation >= speculative_distance {
                // Cache hit, shapes are separated
                return;
            }

            {
                // Attempt face contact using cached feature
                let face_query = FaceQuery {
                    separation: 0.0,
                    face_index: cache.index_a as i32,
                    vertex_index,
                };

                let mut local_cache = SATCache::default();
                let touching =
                    build_face_a_contact(manifold, capacity, hull_a, hull_b, transform_b_to_a, face_query, &mut local_cache);
                if touching && abs_float(cache.separation - local_cache.separation) < linear_slop {
                    // Cache hit, contact points generated
                    return;
                }
            }
        }

        T_FACE_AXIS_B => {
            b3_assert!((cache.index_b as i32) < hull_b.face_count());

            // Check for separation using cached face
            let plane = planes_b[cache.index_b as usize];
            let search_direction_in_a = neg(rotate_vector(transform_b_to_a.q, plane.normal));
            let vertex_index = crate::hull::find_hull_support_vertex(hull_a, search_direction_in_a);
            let support = inv_transform_point(transform_b_to_a, points_a[vertex_index as usize]);
            let separation = plane_separation(plane, support);

            if separation >= speculative_distance {
                // Cache hit, shapes are separated
                return;
            }

            {
                // Attempt face contact using cached feature
                let face_query = FaceQuery {
                    separation: 0.0,
                    face_index: cache.index_b as i32,
                    vertex_index,
                };

                let mut local_cache = SATCache::default();
                let touching =
                    build_face_b_contact(manifold, capacity, hull_a, hull_b, transform_b_to_a, face_query, &mut local_cache);
                if touching && abs_float(cache.separation - local_cache.separation) < linear_slop {
                    // Cache hit, contact points generated
                    return;
                }
            }
        }

        T_EDGE_PAIR_AXIS => {
            let index1 = cache.index_a as i32;
            let edge1 = edges_a[index1 as usize];
            let twin1 = edges_a[index1 as usize + 1];
            b3_assert!(edge1.twin as i32 == index1 + 1 && twin1.twin as i32 == index1);

            let p1 = points_a[edge1.origin as usize];
            let q1 = points_a[twin1.origin as usize];
            let e1 = sub(q1, p1);

            let u1 = planes_a[edge1.face as usize].normal;
            let v1 = planes_a[twin1.face as usize].normal;

            let index2 = cache.index_b as i32;
            let edge2 = edges_b[index2 as usize];
            let twin2 = edges_b[index2 as usize + 1];
            b3_assert!(edge2.twin as i32 == index2 + 1 && twin2.twin as i32 == index2);

            let p2 = transform_point(transform_b_to_a, points_b[edge2.origin as usize]);
            let q2 = transform_point(transform_b_to_a, points_b[twin2.origin as usize]);
            let e2 = sub(q2, p2);

            let u2 = rotate_vector(transform_b_to_a.q, planes_b[edge2.face as usize].normal);
            let v2 = rotate_vector(transform_b_to_a.q, planes_b[twin2.face as usize].normal);

            // flipping the signs of u2 and v2
            // cross(v2, u2) == cross(-v2, -u2)
            // so we still use -e2
            // but we can also use e1 = cross(u1, v1) and e2 = cross(u2, v2)
            let is_minkowski = is_minkowski_face(u1, v1, e1, neg(u2), neg(v2), e2);
            if is_minkowski {
                // Transform reference center of the first hull into local space of the second hull
                let c1 = hull_a.center;
                let c2 = transform_point(transform_b_to_a, hull_b.center);

                let separation = edge_edge_separation(p1, e1, c1, p2, e2, c2);
                if separation > speculative_distance {
                    // Cache hit, shapes are separated
                    return;
                }

                {
                    // Try to rebuild contact from last features
                    let edge_query = EdgeQuery {
                        index_a: cache.index_a as i32,
                        index_b: cache.index_b as i32,
                        separation: 0.0,
                    };

                    let mut local_cache = SATCache::default();
                    let touching = build_edge_contact(manifold, hull_a, hull_b, transform_b_to_a, edge_query, &mut local_cache);
                    if touching && abs_float(cache.separation - local_cache.separation) < linear_slop {
                        // Cache hit, contact point generated
                        return;
                    }
                }
            }
        }

        // This case is for testing
        T_MANUAL_FACE_AXIS_A => {
            let face_query_a = query_face_directions(hull_a, hull_b, transform_b_to_a);
            build_face_a_contact(manifold, capacity, hull_a, hull_b, transform_b_to_a, face_query_a, cache);
            return;
        }

        // This case is for testing
        T_MANUAL_FACE_AXIS_B => {
            let face_query_b = query_face_directions(hull_b, hull_a, invert_transform(transform_b_to_a));
            build_face_b_contact(manifold, capacity, hull_a, hull_b, transform_b_to_a, face_query_b, cache);
            return;
        }

        // This case is for testing
        T_MANUAL_EDGE_PAIR_AXIS => {
            let edge_query = query_edge_directions(hull_a, hull_b, transform_b_to_a);
            if edge_query.index_a != NULL_INDEX {
                build_edge_contact(manifold, hull_a, hull_b, transform_b_to_a, edge_query, cache);
            }
            return;
        }

        _ => {
            b3_assert!(false);
        }
    }

    manifold.points.clear();
    *cache = SATCache::default();

    // Find axis of minimum penetration
    let face_query_a = query_face_directions(hull_a, hull_b, transform_b_to_a);
    if face_query_a.separation > speculative_distance {
        b3_assert!(face_query_a.face_index < hull_a.face_count());
        b3_assert!(face_query_a.vertex_index < hull_b.vertex_count());

        // We found a separating axis
        cache.separation = face_query_a.separation;
        cache.type_ = SeparatingFeature::FaceAxisA as u8;
        cache.index_a = face_query_a.face_index as u8;
        cache.index_b = face_query_a.vertex_index as u8;
        return;
    }

    let face_query_b = query_face_directions(hull_b, hull_a, invert_transform(transform_b_to_a));
    if face_query_b.separation > speculative_distance {
        b3_assert!(face_query_b.face_index < hull_b.face_count());
        b3_assert!(face_query_b.vertex_index < hull_a.vertex_count());

        // We found a separating axis
        cache.separation = face_query_b.separation;
        cache.type_ = SeparatingFeature::FaceAxisB as u8;
        cache.index_a = face_query_b.vertex_index as u8;
        cache.index_b = face_query_b.face_index as u8;
        return;
    }

    let edge_query = query_edge_directions(hull_a, hull_b, transform_b_to_a);
    if edge_query.separation > speculative_distance {
        // We found a separating axis
        cache.separation = edge_query.separation;
        cache.type_ = SeparatingFeature::EdgePairAxis as u8;
        cache.index_a = edge_query.index_a as u8;
        cache.index_b = edge_query.index_b as u8;
        return;
    }

    // Always build a face contact (e.g. Jenga problem)
    let face_separation_a = face_query_a.separation;
    let face_separation_b = face_query_b.separation;
    b3_validate!(face_separation_a <= speculative_distance && face_separation_b <= speculative_distance);

    if face_separation_b > face_separation_a + 0.5 * linear_slop {
        // Face contact B
        build_face_b_contact(manifold, capacity, hull_a, hull_b, transform_b_to_a, face_query_b, cache);
    } else {
        // Face contact A
        build_face_a_contact(manifold, capacity, hull_a, hull_b, transform_b_to_a, face_query_a, cache);
    }

    if edge_query.index_a == NULL_INDEX {
        // There are no valid edge pairs (all edges parallel)
        return;
    }

    let clipped_face_separation = cache.separation;

    b3_validate!(edge_query.separation <= speculative_distance);

    // todo get rid of relative tolerance
    const K_REL_EDGE_TOLERANCE: f32 = 0.90;
    let k_abs_tolerance = 0.5 * linear_slop;

    // Face contact can be empty if it does not realize the axis of minimum penetration.
    // Create edge contact if face contact fails or edge contact is significantly better!
    if manifold.point_count() == 0 || edge_query.separation > K_REL_EDGE_TOLERANCE * clipped_face_separation + k_abs_tolerance {
        // Edge contact
        let mut edge_manifold = LocalManifold::default();

        build_edge_contact(&mut edge_manifold, hull_a, hull_b, transform_b_to_a, edge_query, cache);

        // It is possible with speculation to have vertex-vertex collision that is missed by SAT.
        if edge_manifold.point_count() == 1 {
            // Copy edge manifold out, being careful to preserve manifold point buffer.
            let edge_point = edge_manifold.points[0];
            let mut points = std::mem::take(&mut manifold.points);
            points.clear();
            points.push(edge_point);
            *manifold = edge_manifold;
            manifold.points = points;
        }
    }
}
