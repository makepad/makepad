// Port of box3d/src/triangle_manifold.c
// Triangle versus sphere/capsule/hull manifold generation with SAT caching.

use crate::b3_assert;
use crate::b3_validate;
use crate::constants::{linear_slop, speculative_distance};
use crate::contact::FORCE_GHOST_COLLISIONS;
use crate::core::NULL_INDEX;
use crate::distance::shape_distance;
use crate::manifold::{
    clip_polygon, edge_edge_separation, find_incident_face, flip_pair, make_feature_pair,
    ClipVertex, EdgeQuery, FaceQuery, FeatureOwner, FEATURE_PAIR_SINGLE, MAX_CLIP_POINTS,
};
use crate::math_functions::{
    abs_float, add, closest_point_on_triangle, cross, distance_squared, dot, lerp, line_distance,
    max_float, min_float, min_int, mul_sub, mul_sv, neg, normalize, sub, Plane, Transform, Vec3,
};
use crate::math_internal::{make_plane_from_normal_and_point, make_plane_from_points, plane_separation};
use crate::types::{
    Capsule, DistanceInput, HullData, LocalManifold, LocalManifoldPoint, SATCache,
    SeparatingFeature, ShapeProxy, SimplexCache, Sphere, TriangleFeature,
};

// SeparatingFeature values as u8 for matching against SATCache::type_.
const INVALID_AXIS: u8 = SeparatingFeature::InvalidAxis as u8;
const BACKSIDE_AXIS: u8 = SeparatingFeature::BacksideAxis as u8;
const FACE_AXIS_A: u8 = SeparatingFeature::FaceAxisA as u8;
const FACE_AXIS_B: u8 = SeparatingFeature::FaceAxisB as u8;
const EDGE_PAIR_AXIS: u8 = SeparatingFeature::EdgePairAxis as u8;
const MANUAL_FACE_AXIS_A: u8 = SeparatingFeature::ManualFaceAxisA as u8;
const MANUAL_FACE_AXIS_B: u8 = SeparatingFeature::ManualFaceAxisB as u8;
const MANUAL_EDGE_PAIR_AXIS: u8 = SeparatingFeature::ManualEdgePairAxis as u8;

struct TriangleData {
    v1: Vec3,
    v2: Vec3,
    v3: Vec3,
    e1: Vec3,
    e2: Vec3,
    e3: Vec3,
    center: Vec3,
    plane: Plane,
    flags: i32,
}

// Indexed by the 3-bit vertex mask
const S_TRIANGLE_FEATURES: [TriangleFeature; 8] = [
    TriangleFeature::None,         // 000  (unreachable)
    TriangleFeature::Vertex1,      // 001
    TriangleFeature::Vertex2,      // 010
    TriangleFeature::Edge1,        // 011  v1,v2
    TriangleFeature::Vertex3,      // 100
    TriangleFeature::Edge3,        // 101  v1,v3
    TriangleFeature::Edge2,        // 110  v2,v3
    TriangleFeature::TriangleFace, // 111
];

fn get_triangle_feature(cache: &SimplexCache) -> TriangleFeature {
    let count = cache.count as i32;
    b3_assert!(0 < count && count < 4);

    // Bit i set means triangle vertex i participates in the simplex.
    let mut mask: i32 = 0;
    for i in 0..count {
        b3_assert!(cache.index_a[i as usize] < 3);
        mask |= 1 << cache.index_a[i as usize];
    }

    S_TRIANGLE_FEATURES[mask as usize]
}

// Diagnostic globals defined by the C file (never incremented there); kept for parity.
pub static TRIANGLE_CONVEX_CALLS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
pub static TRIANGLE_CACHE_HITS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

pub fn collide_sphere_and_triangle(
    manifold: &mut LocalManifold,
    capacity: i32,
    sphere_a: &Sphere,
    triangle_b: &[Vec3],
) {
    manifold.points.clear();

    if capacity == 0 {
        return;
    }

    let center = sphere_a.center;
    let v1 = triangle_b[0];
    let v2 = triangle_b[1];
    let v3 = triangle_b[2];
    let plane = make_plane_from_points(v1, v2, v3);

    let offset = plane_separation(plane, center);
    if offset < 0.0 {
        // Cull back side collision
        return;
    }

    // Closest point on triangle to sphere center
    let closest = closest_point_on_triangle(v1, v2, v3, center);

    // Test separating axis
    let squared_distance = distance_squared(closest.point, center);
    let speculative = speculative_distance();
    let max_distance = sphere_a.radius + speculative;
    if squared_distance > max_distance * max_distance {
        return;
    }

    let distance = squared_distance.sqrt();
    let normal = if distance * distance > 1000.0 * f32::MIN_POSITIVE {
        mul_sv(1.0 / distance, sub(center, closest.point))
    } else {
        normalize(cross(sub(v2, v1), sub(v3, v1)))
    };

    // contact point mid-way
    let contact_point = mul_sv(0.5, add(sub(center, mul_sv(sphere_a.radius, normal)), closest.point));

    manifold.normal = normal;
    manifold.feature = closest.feature;
    manifold.squared_distance = squared_distance;

    manifold.points.push(LocalManifoldPoint {
        point: contact_point,
        separation: distance - sphere_a.radius,
        pair: FEATURE_PAIR_SINGLE,
        triangle_index: 0,
    });
}

fn clip_segment_to_triangle_face(segment: &mut [ClipVertex; 2], points: &[Vec3], plane: Plane) -> bool {
    let mut vertex1 = points[2];
    for i in 0..3 {
        let vertex2 = points[i];
        let tangent = normalize(sub(vertex2, vertex1));
        let binormal = cross(tangent, plane.normal);

        let clip_plane = make_plane_from_normal_and_point(binormal, vertex1);

        let mut vertex_count = 0usize;
        let p1 = segment[0];
        let p2 = segment[1];

        let distance1 = plane_separation(clip_plane, p1.position);
        let distance2 = plane_separation(clip_plane, p2.position);

        // If the points are behind the plane
        if distance1 <= 0.0 {
            segment[vertex_count] = p1;
            vertex_count += 1;
        }
        if distance2 <= 0.0 {
            segment[vertex_count] = p2;
            vertex_count += 1;
        }

        // If the points are on different sides of the plane
        if distance1 * distance2 < 0.0 {
            // Find intersection point of edge and plane
            let t = distance1 / (distance1 - distance2);
            segment[vertex_count].position = lerp(p1.position, p2.position, t);
            segment[vertex_count].pair = if distance1 > 0.0 { p1.pair } else { p2.pair };
            vertex_count += 1;
        }

        if vertex_count != 2 {
            return false;
        }

        vertex1 = vertex2;
    }

    true
}

fn query_triangle_face_and_capsule(plane: Plane, capsule: &Capsule) -> FaceQuery {
    let separation1 = plane_separation(plane, capsule.center1);
    let separation2 = plane_separation(plane, capsule.center2);

    if separation1 < separation2 {
        return FaceQuery { separation: separation1, face_index: 0, vertex_index: 0 };
    }

    FaceQuery { separation: separation2, face_index: 0, vertex_index: 1 }
}

fn query_triangle_and_capsule_edges(vertices: &[Vec3], capsule: &Capsule) -> EdgeQuery {
    // Work in the local space of the capsule
    let p1 = capsule.center1;
    let p2 = capsule.center2;
    let capsule_edge = sub(p2, p1);

    let capsule_center = lerp(p1, p2, 0.5);

    let triangle_center = mul_sv(1.0 / 3.0, add(vertices[0], add(vertices[1], vertices[2])));

    // Find axis of minimum penetration
    let mut max_separation = -f32::MAX;
    let mut max_index1: i32 = u8::MAX as i32;
    let max_index2: i32 = 0;

    let mut edge_index: i32 = 2;
    let mut v1 = vertices[2];
    for index in 0..3 {
        let v2 = vertices[index];

        let triangle_edge = sub(v2, v1);

        let separation = edge_edge_separation(p1, capsule_edge, capsule_center, v1, triangle_edge, triangle_center);
        if separation > max_separation {
            // Note: We don't exit early if we find a separating axis here since we want to
            // find the best one for caching and account for the convex radius later.
            max_separation = separation;
            max_index1 = edge_index;
        }

        v1 = v2;
        edge_index = index as i32;
    }

    // Save result (the C code casts the indices through uint8_t)
    EdgeQuery {
        separation: max_separation,
        index_a: (max_index1 as u8) as i32,
        index_b: (max_index2 as u8) as i32,
    }
}

fn build_triangle_and_capsule_face_contact(
    manifold: &mut LocalManifold,
    triangle: &[Vec3],
    plane: Plane,
    capsule: &Capsule,
) {
    b3_assert!(manifold.points.is_empty());

    let mut segment = [ClipVertex::default(); 2];
    segment[0].position = capsule.center1;
    segment[0].separation = 0.0;
    segment[0].pair = make_feature_pair(FeatureOwner::ShapeA, 0, FeatureOwner::ShapeA, 0);
    segment[1].position = capsule.center2;
    segment[1].separation = 0.0;
    segment[1].pair = make_feature_pair(FeatureOwner::ShapeA, 1, FeatureOwner::ShapeA, 1);

    let have_points = clip_segment_to_triangle_face(&mut segment, triangle, plane);
    if !have_points {
        return;
    }

    let radius = capsule.radius;
    let distance1 = plane_separation(plane, segment[0].position);
    let distance2 = plane_separation(plane, segment[1].position);

    let speculative = speculative_distance();
    if distance1 > speculative + radius && distance2 > speculative + radius {
        return;
    }

    // Average points. Half-way between capsule bottom and triangle plane.
    let point1 = mul_sub(segment[0].position, 0.5 * (distance1 + capsule.radius), plane.normal);
    let point2 = mul_sub(segment[1].position, 0.5 * (distance2 + capsule.radius), plane.normal);

    manifold.normal = plane.normal;
    manifold.feature = TriangleFeature::TriangleFace;

    manifold.points.push(LocalManifoldPoint {
        point: point1,
        separation: distance1 - capsule.radius,
        pair: segment[0].pair,
        triangle_index: 0,
    });

    manifold.points.push(LocalManifoldPoint {
        point: point2,
        separation: distance2 - capsule.radius,
        pair: segment[1].pair,
        triangle_index: 0,
    });
}

fn build_triangle_and_capsule_edge_contact(
    manifold: &mut LocalManifold,
    triangle: &[Vec3],
    capsule: &Capsule,
    query: EdgeQuery,
) {
    b3_assert!(0 <= query.index_a && query.index_a < 3);

    let p1 = capsule.center1;
    let p2 = capsule.center2;
    let capsule_edge = sub(p2, p1);

    let vs = triangle;

    let triangle_center = mul_sv(1.0 / 3.0, add(vs[0], add(vs[1], vs[2])));
    let v1 = vs[query.index_a as usize];
    let v2 = vs[((query.index_a + 1) % 3) as usize];
    let triangle_edge = sub(v2, v1);

    let mut normal = cross(capsule_edge, triangle_edge);
    normal = normalize(normal);

    // Normal should point away from triangle center
    if dot(normal, sub(v1, triangle_center)) < 0.0 {
        normal = neg(normal);
    }

    let result = line_distance(v1, triangle_edge, p1, capsule_edge);

    if result.fraction1 < 0.0 || 1.0 < result.fraction1 || result.fraction2 < 0.0 || 1.0 < result.fraction2 {
        // closest point beyond end points
        return;
    }

    let point = lerp(mul_sub(result.point1, capsule.radius, normal), result.point2, 0.5);

    let separation = dot(normal, sub(result.point2, result.point1));
    b3_validate!(abs_float(separation - query.separation) < linear_slop());

    manifold.normal = normal;

    let edges_features = [TriangleFeature::Edge1, TriangleFeature::Edge2, TriangleFeature::Edge3];
    manifold.feature = edges_features[query.index_a as usize];

    // The C code overwrites point 0 and sets pointCount = 1 (a prior 2-point
    // face contact is replaced).
    manifold.points.clear();
    manifold.points.push(LocalManifoldPoint {
        point,
        separation: separation - capsule.radius,
        pair: make_feature_pair(FeatureOwner::ShapeA, query.index_a, FeatureOwner::ShapeB, query.index_b),
        triangle_index: 0,
    });
}

pub fn collide_capsule_and_triangle(
    manifold: &mut LocalManifold,
    capacity: i32,
    capsule_a: &Capsule,
    triangle_b: &[Vec3],
    cache: &mut SimplexCache,
) {
    manifold.points.clear();

    if capacity < 2 {
        return;
    }

    let v1 = triangle_b[0];
    let v2 = triangle_b[1];
    let v3 = triangle_b[2];
    let plane = make_plane_from_points(v1, v2, v3);
    let capsule_center = lerp(capsule_a.center1, capsule_a.center2, 0.5);

    let offset = plane_separation(plane, capsule_center);
    if offset < 0.0 {
        // Cull back side collision
        return;
    }

    let capsule_points = [capsule_a.center1, capsule_a.center2];
    let distance_input = DistanceInput {
        proxy_a: ShapeProxy { points: triangle_b, radius: 0.0 },
        proxy_b: ShapeProxy { points: &capsule_points, radius: 0.0 },
        transform: Transform::IDENTITY,
        use_radii: false,
    };

    let distance_output = shape_distance(&distance_input, cache, None);

    let radius = capsule_a.radius;
    if distance_output.distance > radius + speculative_distance() {
        // Shapes are separated, persist the cache
        return;
    }

    if distance_output.distance > 100.0 * f32::EPSILON {
        // Shallow penetration
        let delta = normalize(sub(distance_output.point_b, distance_output.point_a));

        // Try to create two contact points if closest points difference is nearly parallel to face normal
        const K_TOLERANCE: f32 = 0.2;
        let cos_angle = abs_float(dot(plane.normal, delta));
        if cos_angle > K_TOLERANCE {
            // Clip capsule segment against side planes of reference face
            let mut segment = [ClipVertex::default(); 2];
            segment[0].position = capsule_a.center1;
            segment[0].separation = 0.0;
            segment[0].pair = make_feature_pair(FeatureOwner::ShapeA, 0, FeatureOwner::ShapeA, 0);
            segment[1].position = capsule_a.center2;
            segment[1].separation = 0.0;
            segment[1].pair = make_feature_pair(FeatureOwner::ShapeA, 1, FeatureOwner::ShapeA, 1);

            let have_points = clip_segment_to_triangle_face(&mut segment, triangle_b, plane);

            if have_points {
                let distance1 = plane_separation(plane, segment[0].position);
                let distance2 = plane_separation(plane, segment[1].position);

                let normal = plane.normal;
                let point1 = mul_sub(segment[0].position, 0.5 * (radius + distance1), normal);
                let point2 = mul_sub(segment[1].position, 0.5 * (radius + distance2), normal);

                manifold.normal = normal;
                manifold.feature = TriangleFeature::TriangleFace;

                manifold.points.push(LocalManifoldPoint {
                    point: point1,
                    separation: distance1 - radius,
                    pair: segment[0].pair,
                    triangle_index: 0,
                });

                manifold.points.push(LocalManifoldPoint {
                    point: point2,
                    separation: distance2 - radius,
                    pair: segment[1].pair,
                    triangle_index: 0,
                });

                return;
            }
        }

        // Create contact from closest points
        let point = mul_sv(0.5, add(sub(distance_output.point_a, mul_sv(radius, delta)), distance_output.point_b));

        manifold.normal = delta;
        manifold.feature = get_triangle_feature(cache);

        manifold.points.push(LocalManifoldPoint {
            point,
            separation: distance_output.distance - radius,
            pair: FEATURE_PAIR_SINGLE,
            triangle_index: 0,
        });

        return;
    }

    // Deep penetration

    let face_query = query_triangle_face_and_capsule(plane, capsule_a);
    if face_query.separation > radius {
        // Shapes are separated
        return;
    }

    let edge_query = query_triangle_and_capsule_edges(triangle_b, capsule_a);
    if edge_query.separation > radius {
        // Shapes are separated
        return;
    }

    // Create face contact
    let mut face_separation = face_query.separation - radius;
    build_triangle_and_capsule_face_contact(manifold, triangle_b, plane, capsule_a);
    if manifold.point_count() == 2 {
        face_separation = min_float(manifold.points[0].separation, manifold.points[1].separation);
    }
    b3_validate!(face_separation <= 0.0);
    let _ = face_separation;

    // Face contact can be empty if it does not realize the axis of minimum penetration.
    // Create edge contact if face contact fails or edge contact is significantly better!
    const K_REL_EDGE_TOLERANCE: f32 = 0.50;
    let k_abs_tolerance = 1.0 * linear_slop();
    let edge_separation = edge_query.separation - radius;
    if manifold.point_count() == 0 || edge_separation > K_REL_EDGE_TOLERANCE * face_separation + k_abs_tolerance {
        // Edge contact
        build_triangle_and_capsule_edge_contact(manifold, triangle_b, capsule_a, edge_query);
    }
}

#[inline]
fn get_triangle_support(points: &[Vec3], direction: Vec3) -> i32 {
    let mut index = 0;
    let mut distance = dot(points[0], direction);

    let mut d = dot(points[1], direction);
    if d > distance {
        distance = d;
        index = 1;
    }

    d = dot(points[2], direction);
    if d > distance {
        return 2;
    }

    index
}

fn query_triangle_face(triangle: &TriangleData, hull: &HullData) -> FaceQuery {
    let plane = triangle.plane;
    let vertex_index = crate::hull::find_hull_support_vertex(hull, neg(plane.normal));
    let support = hull.points[vertex_index as usize];
    let separation = plane_separation(plane, support);

    FaceQuery {
        separation,
        face_index: 0,
        vertex_index: (vertex_index as u8) as i32,
    }
}

fn query_hull_face(triangle: &TriangleData, hull: &HullData) -> FaceQuery {
    let face_count = hull.face_count();

    let triangle_points = [triangle.v1, triangle.v2, triangle.v3];

    let mut max_face_index: i32 = -1;
    let mut max_vertex_index: i32 = -1;
    let mut max_face_separation = -f32::MAX;

    for face_index in 0..face_count {
        let plane = hull.planes[face_index as usize];

        let vertex_index = get_triangle_support(&triangle_points, neg(plane.normal));
        let support = triangle_points[vertex_index as usize];
        let separation = plane_separation(plane, support);
        if separation > max_face_separation {
            max_face_index = face_index;
            max_vertex_index = vertex_index;
            max_face_separation = separation;
        }
    }

    FaceQuery {
        separation: max_face_separation,
        face_index: max_face_index,
        vertex_index: max_vertex_index,
    }
}

fn test_edge_pairs(triangle: &TriangleData, hull: &HullData) -> EdgeQuery {
    let mut result = EdgeQuery {
        separation: -f32::MAX,
        index_a: NULL_INDEX,
        index_b: NULL_INDEX,
    };

    let triangle_points = [triangle.v1, triangle.v2, triangle.v3];
    let triangle_edges = [triangle.e1, triangle.e2, triangle.e3];
    // int edgeFlags[] = { b3_concaveEdge1, b3_concaveEdge1, b3_concaveEdge3 };

    // The ghost-collision edge filtering is commented out in the C source; the
    // flags are computed but unused there as well.
    let _triangle_flags = if FORCE_GHOST_COLLISIONS { 0xFF } else { triangle.flags };

    let tri_normal = triangle.plane.normal;

    let edge_count = hull.edge_count();

    let mut i: i32 = 0;
    while i < edge_count {
        let edge = &hull.edges[i as usize];
        let twin = &hull.edges[(i + 1) as usize];
        b3_assert!(edge.twin as i32 == i + 1 && twin.twin as i32 == i);

        let hull_point = hull.points[edge.origin as usize];
        let hull_edge = sub(hull.points[twin.origin as usize], hull_point);

        let hull_normal1 = hull.planes[edge.face as usize].normal;
        let hull_normal2 = hull.planes[twin.face as usize].normal;

        for j in 0..3 {
            let tri_edge = triangle_edges[j];

            let cab = dot(hull_normal1, tri_edge);
            let dab = dot(hull_normal2, tri_edge);
            let bcd = dot(tri_normal, hull_edge);
            if cab * dab >= 0.0 || cab * bcd <= 0.0 {
                continue;
            }

            let tri_point = triangle_points[j];
            let separation =
                edge_edge_separation(tri_point, tri_edge, triangle.center, hull_point, hull_edge, hull.center);

            // if ( separation > result.separation && ( edgeFlags[j] & triangleFlags ) == 0 )
            if separation > result.separation {
                // Note: We don't exit early if we find a separating axis here since we want to
                // find the best one for caching.
                result.separation = separation;
                result.index_a = j as i32;
                result.index_b = i;
            }
        }

        i += 2;
    }

    result
}

fn collide_hull_face(
    manifold: &mut LocalManifold,
    point_capacity: i32,
    triangle: &TriangleData,
    hull: &HullData,
    query: FaceQuery,
    cache: &mut SATCache,
) -> f32 {
    manifold.points.clear();

    // Reference hull face
    let ref_face = query.face_index;
    let ref_plane = hull.planes[ref_face as usize];

    // Build clip polygon from triangle face (the incident face).
    // The C version ping-pongs between two buffers; the port clips into a
    // scratch buffer and copies back (ClipVertex is Copy).
    let mut polygon = [ClipVertex::default(); MAX_CLIP_POINTS];
    let mut scratch = [ClipVertex::default(); MAX_CLIP_POINTS];

    let v1 = triangle.v1;
    let v2 = triangle.v2;
    let v3 = triangle.v3;
    polygon[0].position = v1;
    polygon[0].separation = plane_separation(ref_plane, v1);
    polygon[0].pair = make_feature_pair(FeatureOwner::ShapeB, 2, FeatureOwner::ShapeB, 0);
    polygon[1].position = v2;
    polygon[1].separation = plane_separation(ref_plane, v2);
    polygon[1].pair = make_feature_pair(FeatureOwner::ShapeB, 0, FeatureOwner::ShapeB, 1);
    polygon[2].position = v3;
    polygon[2].separation = plane_separation(ref_plane, v3);
    polygon[2].pair = make_feature_pair(FeatureOwner::ShapeB, 1, FeatureOwner::ShapeB, 2);
    let mut point_count: i32 = 3;

    // Clip triangle face against side planes of reference face
    let face_edge = hull.faces[ref_face as usize].edge as i32;
    let mut edge_index = face_edge;

    loop {
        let edge = &hull.edges[edge_index as usize];
        let next_edge_index = edge.next as i32;
        let next = &hull.edges[next_edge_index as usize];
        let vertex1 = hull.points[edge.origin as usize];
        let vertex2 = hull.points[next.origin as usize];
        let tangent = normalize(sub(vertex2, vertex1));
        let binormal = cross(tangent, ref_plane.normal);

        let clip_plane = make_plane_from_normal_and_point(binormal, vertex1);

        point_count = clip_polygon(&mut scratch, &polygon, point_count, clip_plane, edge_index, ref_plane);
        b3_assert!(point_count <= MAX_CLIP_POINTS as i32);

        if point_count < 3 {
            // Using a stale cache
            *cache = SATCache::default();
            return query.separation;
        }

        // Swap buffers, output becomes input for the next clipping plane
        polygon[..point_count as usize].copy_from_slice(&scratch[..point_count as usize]);
        edge_index = next_edge_index;
        if edge_index == face_edge {
            break;
        }
    }

    let point_count = min_int(point_count, point_capacity);
    let mut min_separation = f32::MAX;

    for i in 0..point_count {
        let clip_point = &polygon[i as usize];

        // Move point onto hull face improved culling
        let point = mul_sub(clip_point.position, clip_point.separation, ref_plane.normal);

        manifold.points.push(LocalManifoldPoint {
            point,
            separation: clip_point.separation,
            pair: flip_pair(clip_point.pair),
            triangle_index: 0,
        });

        min_separation = min_float(min_separation, clip_point.separation);
    }

    if min_separation > speculative_distance() {
        // This can occur with a stale SAT cache
        manifold.points.clear();
        *cache = SATCache::default();
        return min_separation;
    }

    manifold.normal = neg(ref_plane.normal);
    manifold.feature = TriangleFeature::HullFace;

    // Save cache
    cache.separation = min_separation;
    cache.type_ = FACE_AXIS_B;
    cache.index_a = query.vertex_index as u8;
    cache.index_b = query.face_index as u8;
    min_separation
}

fn collide_triangle_face(
    manifold: &mut LocalManifold,
    point_capacity: i32,
    triangle: &TriangleData,
    hull: &HullData,
    query: FaceQuery,
    cache: &mut SATCache,
) -> f32 {
    b3_validate!(manifold.points.is_empty());

    // Find incident face
    b3_assert!(query.face_index == 0);
    let ref_plane = triangle.plane;

    let inc_face = find_incident_face(hull, ref_plane.normal, query.vertex_index);

    // Build clip polygon from incident face
    let mut polygon = [ClipVertex::default(); 2 * MAX_CLIP_POINTS];
    let mut scratch = [ClipVertex::default(); 2 * MAX_CLIP_POINTS];
    let mut point_count: i32 = 0;
    let face_edge = hull.faces[inc_face as usize].edge as i32;
    let mut hull_edge_index = face_edge;

    loop {
        let edge = &hull.edges[hull_edge_index as usize];

        let next_edge_index = edge.next as i32;
        let next = &hull.edges[next_edge_index as usize];

        let hull_point = hull.points[next.origin as usize];
        polygon[point_count as usize].position = hull_point;
        polygon[point_count as usize].separation = plane_separation(ref_plane, hull_point);
        polygon[point_count as usize].pair =
            make_feature_pair(FeatureOwner::ShapeB, hull_edge_index, FeatureOwner::ShapeB, next_edge_index);

        point_count += 1;
        hull_edge_index = next_edge_index;
        if !(hull_edge_index != face_edge && point_count < 2 * MAX_CLIP_POINTS as i32) {
            break;
        }
    }

    b3_assert!(point_count >= 3);

    // Clip incident face against side planes of reference face (triangle)
    let triangle_points = [triangle.v1, triangle.v2, triangle.v3];
    let triangle_edges = [triangle.e1, triangle.e2, triangle.e3];

    let mut i: i32 = 0;
    while i < 3 && point_count > 0 {
        let mut side_normal = cross(triangle_edges[i as usize], ref_plane.normal);
        side_normal = normalize(side_normal);

        let clip_plane = make_plane_from_normal_and_point(side_normal, triangle_points[i as usize]);

        point_count = clip_polygon(&mut scratch, &polygon, point_count, clip_plane, i, ref_plane);
        b3_assert!(point_count <= 2 * MAX_CLIP_POINTS as i32);

        polygon[..point_count as usize].copy_from_slice(&scratch[..point_count as usize]);
        i += 1;
    }

    if point_count == 0 {
        // Triangle face clipped away. Invalidate cache.
        *cache = SATCache::default();
        return f32::MAX;
    }

    let point_count = min_int(point_count, point_capacity);

    let mut min_separation = f32::MAX;

    for i in 0..point_count {
        let clip_point = &polygon[i as usize];

        // Move point onto triangle surface for improved culling
        // b3Vec3 point = b3MulSub( clipPoint->position, clipPoint->separation, refPlane.normal );
        let point = clip_point.position;

        manifold.points.push(LocalManifoldPoint {
            point,
            separation: clip_point.separation,
            pair: clip_point.pair,
            triangle_index: 0,
        });

        min_separation = min_float(min_separation, clip_point.separation);
    }

    if min_separation >= speculative_distance() {
        // This can happen if the objects move apart while re-using a cached axis
        manifold.points.clear();
        *cache = SATCache::default();
        return min_separation;
    }

    manifold.normal = ref_plane.normal;
    manifold.feature = TriangleFeature::TriangleFace;

    // Save cache
    cache.separation = min_separation;
    cache.type_ = FACE_AXIS_A;
    cache.index_a = query.face_index as u8;
    cache.index_b = query.vertex_index as u8;
    min_separation
}

fn collide_hull_and_triangle_edges(
    manifold: &mut LocalManifold,
    capacity: i32,
    triangle_point: Vec3,
    triangle_edge: Vec3,
    triangle_center: Vec3,
    hull: &HullData,
    query: EdgeQuery,
    cache: &mut SATCache,
) {
    b3_validate!(query.separation <= 2.0 * speculative_distance());
    b3_assert!(query.index_a < 3);

    let c_a = triangle_center;
    let p_a = triangle_point;
    let e_a = triangle_edge;

    let edge_b = &hull.edges[query.index_b as usize];
    let twin_b = &hull.edges[edge_b.twin as usize];
    let p_b = hull.points[edge_b.origin as usize];
    let q_b = hull.points[twin_b.origin as usize];
    let e_b = sub(q_b, p_b);

    let mut normal = cross(e_a, e_b);
    normal = normalize(normal);

    // Ensure normal points outward from triangle center
    let outward_a = dot(normal, sub(p_a, c_a));

    // Ensure normal points towards hull center
    let outward_b = dot(normal, sub(hull.center, p_b));

    // Use the largest magnitude. The triangle outward value
    // may be unreliable as some angles.
    if abs_float(outward_a) > abs_float(outward_b) {
        if outward_a < 0.0 {
            normal = neg(normal);
        }
    } else if outward_b < 0.0 {
        normal = neg(normal);
    }

    // Get the closest points between the infinite edge lines
    let result = line_distance(p_a, e_a, p_b, e_b);

    // Is one of the closest points outside of the associated edge segment?
    if capacity == 0
        || result.fraction1 < 0.0
        || 1.0 < result.fraction1
        || result.fraction2 < 0.0
        || 1.0 < result.fraction2
    {
        // Invalid edge pair, no points generated
        b3_assert!(manifold.points.is_empty());
        *cache = SATCache::default();
        return;
    }

    // This can slide off the end from caching
    let separation = dot(normal, sub(result.point2, result.point1));
    b3_validate!(abs_float(separation - query.separation) < linear_slop());

    let point = mul_sv(0.5, add(result.point1, result.point2));

    manifold.points.clear();
    manifold.points.push(LocalManifoldPoint {
        point,
        separation,
        pair: make_feature_pair(FeatureOwner::ShapeA, query.index_a, FeatureOwner::ShapeB, query.index_b),
        triangle_index: 0,
    });

    // Save cache
    cache.separation = separation;
    cache.type_ = EDGE_PAIR_AXIS;
    cache.index_a = query.index_a as u8;
    cache.index_b = query.index_b as u8;

    manifold.normal = normal;

    let edges_features = [TriangleFeature::Edge1, TriangleFeature::Edge2, TriangleFeature::Edge3];
    manifold.feature = edges_features[query.index_a as usize];
}

// See "Collision Detection of Convex Polyhedra Based on Duality Transformation"
// Simplified for triangle versus hull
#[inline]
fn is_triangle_minkowski_face(
    tri_normal: Vec3,
    tri_edge: Vec3,
    hull_normal1: Vec3,
    hull_normal2: Vec3,
    hull_edge: Vec3,
) -> bool {
    let cab = dot(hull_normal1, tri_edge);
    let dab = dot(hull_normal2, tri_edge);
    let bcd = dot(tri_normal, hull_edge);
    cab * dab < 0.0 && cab * bcd > 0.0
}

// Computes the manifold in the local space of the hull
pub fn collide_hull_and_triangle(
    manifold: &mut LocalManifold,
    capacity: i32,
    hull_a: &HullData,
    v1: Vec3,
    v2: Vec3,
    v3: Vec3,
    triangle_flags: i32,
    cache: &mut SATCache,
) {
    manifold.points.clear();
    manifold.feature = TriangleFeature::None;

    if capacity < 4 {
        return;
    }

    let triangle_plane = make_plane_from_points(v1, v2, v3);
    let slop = linear_slop();

    let offset = plane_separation(triangle_plane, hull_a.center);
    if cache.type_ == BACKSIDE_AXIS {
        // Use hysteresis to avoid jitter on wavy meshes
        if abs_float(cache.separation - offset) < slop {
            return;
        }

        cache.type_ = INVALID_AXIS;
    }

    if offset < -slop {
        // Cull back side collision. Cache offset to add hysteresis.
        cache.type_ = BACKSIDE_AXIS;
        cache.separation = offset;
        return;
    }

    let triangle_center = mul_sv(1.0 / 3.0, add(v1, add(v2, v3)));
    let triangle_points = [v1, v2, v3];
    let triangle_edges = [sub(v2, v1), sub(v3, v2), sub(v1, v3)];

    let triangle = TriangleData {
        v1,
        v2,
        v3,
        e1: triangle_edges[0],
        e2: triangle_edges[1],
        e3: triangle_edges[2],
        center: triangle_center,
        plane: triangle_plane,
        flags: triangle_flags,
    };

    let speculative = speculative_distance();
    cache.hit = 1;

    // Attempt to use the cache to speed up collision
    match cache.type_ {
        FACE_AXIS_A => {
            b3_assert!(cache.index_a == 0);

            let vertex_index = crate::hull::find_hull_support_vertex(hull_a, neg(triangle_plane.normal));
            let support = hull_a.points[vertex_index as usize];
            let separation = plane_separation(triangle_plane, support);
            if separation >= speculative {
                // Cache hit, shapes are separated
                return;
            }

            let face_query = FaceQuery {
                separation,
                face_index: cache.index_a as i32,
                vertex_index,
            };

            // Read cache but don't modify it
            let mut local_cache = *cache;
            let clipped_separation =
                collide_triangle_face(manifold, capacity, &triangle, hull_a, face_query, &mut local_cache);

            if manifold.point_count() > 0 && abs_float(cache.separation - clipped_separation) < slop {
                // Cache hit, contact points generated
                return;
            }

            // Invalidate cache and fall through
            manifold.points.clear();
            *cache = SATCache::default();
        }

        FACE_AXIS_B => {
            b3_assert!((cache.index_b as i32) < hull_a.face_count());

            let plane = hull_a.planes[cache.index_b as usize];

            // Get triangle support point
            let mut vertex_index: i32 = 0;
            let mut distance = -dot(v1, plane.normal);
            for i in 1..3 {
                let d = -dot(triangle_points[i as usize], plane.normal);
                if d > distance {
                    distance = d;
                    vertex_index = i;
                }
            }

            let support = triangle_points[vertex_index as usize];

            // Separation of triangle support point with hull plane
            let separation = plane_separation(plane, support);
            if separation >= speculative {
                // Cache hit, shapes are separated
                return;
            }

            // Deep overlap may lead to an invalid cache
            // todo confirm
            let is_deep = separation < -2.0 * slop;

            // Don't persist deep cache or allow separation to change too much
            if !is_deep {
                // Try to rebuild contact from last features
                let face_query = FaceQuery {
                    separation,
                    face_index: cache.index_b as i32,
                    vertex_index,
                };

                // Read cache but don't modify it
                let mut local_cache = *cache;
                let clipped_separation =
                    collide_hull_face(manifold, capacity, &triangle, hull_a, face_query, &mut local_cache);

                // Cache reuse is only successful if it creates contact points and the clipped
                // separation didn't change much.
                if manifold.point_count() > 0 && abs_float(cache.separation - clipped_separation) < slop {
                    // Cache hit, contact points generated
                    return;
                }
            }

            // Invalidate cache and fall through
            manifold.points.clear();
            *cache = SATCache::default();
        }

        EDGE_PAIR_AXIS => {
            b3_assert!(cache.index_a < 3);
            let index_a = cache.index_a as i32;

            let tri_point = triangle_points[index_a as usize];
            let tri_edge = triangle_edges[index_a as usize];

            b3_assert!((cache.index_b as i32) < hull_a.edge_count() - 1);
            let index_b = cache.index_b as i32;

            let edge2 = &hull_a.edges[index_b as usize];
            let twin2 = &hull_a.edges[(index_b + 1) as usize];
            b3_assert!(edge2.twin as i32 == index_b + 1 && twin2.twin as i32 == index_b);

            let hull_point = hull_a.points[edge2.origin as usize];
            let hull_edge = sub(hull_a.points[twin2.origin as usize], hull_point);
            let hull_normal1 = hull_a.planes[edge2.face as usize].normal;
            let hull_normal2 = hull_a.planes[twin2.face as usize].normal;

            // Confirm the edge pair is still a Minkowski face
            let is_minkowski =
                is_triangle_minkowski_face(triangle_plane.normal, tri_edge, hull_normal1, hull_normal2, hull_edge);
            if is_minkowski {
                // Transform reference center of the first hull into local space of the second hull
                let separation =
                    edge_edge_separation(tri_point, tri_edge, triangle_center, hull_point, hull_edge, hull_a.center);
                if separation > speculative {
                    // Cache hit, shapes are separated
                    return;
                }

                if abs_float(cache.separation - separation) < slop {
                    // Try to rebuild contact from last features
                    let edge_query = EdgeQuery { separation, index_a, index_b };

                    // Read cache but don't modify it
                    let mut local_cache = *cache;
                    collide_hull_and_triangle_edges(
                        manifold,
                        capacity,
                        tri_point,
                        tri_edge,
                        triangle_center,
                        hull_a,
                        edge_query,
                        &mut local_cache,
                    );

                    if manifold.point_count() > 0 {
                        // Cache hit, contact point generated
                        return;
                    }
                }
            }

            // Invalidate cache and fall through
            *cache = SATCache::default();
        }

        // This case is for testing
        MANUAL_FACE_AXIS_A => {
            let face_query_a = query_triangle_face(&triangle, hull_a);
            collide_triangle_face(manifold, capacity, &triangle, hull_a, face_query_a, cache);
            return;
        }

        // This case is for testing
        MANUAL_FACE_AXIS_B => {
            let face_query_b = query_hull_face(&triangle, hull_a);
            collide_hull_face(manifold, capacity, &triangle, hull_a, face_query_b, cache);
            return;
        }

        // This case is for testing
        MANUAL_EDGE_PAIR_AXIS => {
            let edge_query = test_edge_pairs(&triangle, hull_a);
            if edge_query.index_a != NULL_INDEX {
                let triangle_point = triangle_points[edge_query.index_a as usize];
                let triangle_edge = triangle_edges[edge_query.index_a as usize];
                collide_hull_and_triangle_edges(
                    manifold,
                    capacity,
                    triangle_point,
                    triangle_edge,
                    triangle_center,
                    hull_a,
                    edge_query,
                    cache,
                );
            }
            return;
        }

        _ => {
            b3_assert!(cache.type_ == INVALID_AXIS);
        }
    }

    // Cache miss
    cache.hit = 0;

    // Find axis of minimum penetration
    let face_query_a = query_triangle_face(&triangle, hull_a);
    if face_query_a.separation > speculative {
        // Separating axis found
        cache.separation = face_query_a.separation;
        cache.type_ = FACE_AXIS_A;
        cache.index_a = 0;
        cache.index_b = u8::MAX;
        return;
    }

    let face_query_b = query_hull_face(&triangle, hull_a);
    if face_query_b.separation > speculative {
        // Separating axis found
        cache.separation = face_query_b.separation;
        cache.type_ = FACE_AXIS_B;
        cache.index_a = u8::MAX;
        cache.index_b = face_query_b.face_index as u8;
        return;
    }

    let edge_query = test_edge_pairs(&triangle, hull_a);
    if edge_query.separation > speculative {
        // Separating axis found
        cache.separation = edge_query.separation;
        cache.type_ = EDGE_PAIR_AXIS;
        cache.index_a = edge_query.index_a as u8;
        cache.index_b = edge_query.index_b as u8;
        return;
    }

    let clipped_face_separation;

    // Don't allow a hull face opposed to the triangle face.
    let hull_normal = hull_a.planes[face_query_b.face_index as usize].normal;
    let pushing_up = dot(hull_normal, triangle_plane.normal) < 0.0;
    if face_query_b.separation > face_query_a.separation + slop && pushing_up {
        clipped_face_separation = collide_hull_face(manifold, capacity, &triangle, hull_a, face_query_b, cache);
    } else {
        clipped_face_separation = collide_triangle_face(manifold, capacity, &triangle, hull_a, face_query_a, cache);
    }

    // Does an edge axis exist?
    if edge_query.index_a != NULL_INDEX {
        // When axes are aligned the edge separation can be garbage.
        // If a face axis has positive separation there may be no points.
        let max_face_separation = max_float(face_query_a.separation, face_query_b.separation);

        if (manifold.point_count() == 0 && edge_query.separation > max_face_separation)
            || (manifold.point_count() == 1 && edge_query.separation > clipped_face_separation + slop)
        {
            b3_assert!(0 <= edge_query.index_a && edge_query.index_a < 3);
            let triangle_point = triangle_points[edge_query.index_a as usize];
            let triangle_edge = triangle_edges[edge_query.index_a as usize];
            manifold.points.clear();
            collide_hull_and_triangle_edges(
                manifold,
                capacity,
                triangle_point,
                triangle_edge,
                triangle_center,
                hull_a,
                edge_query,
                cache,
            );
        }
    }

    // Using the speculative distance means that sometimes there are no valid contact points from SAT.
    // In this fall back to GJK. This is important to prevent tunneling in rare cases.
    if manifold.point_count() == 0 {
        let triangle_b = [v1, v2, v3];
        let input = DistanceInput {
            proxy_a: ShapeProxy { points: &triangle_b, radius: 0.0 },
            proxy_b: ShapeProxy { points: &hull_a.points, radius: 0.0 },
            transform: Transform::IDENTITY,
            use_radii: false,
        };

        let mut simplex_cache = SimplexCache::default();
        let output = shape_distance(&input, &mut simplex_cache, None);

        if output.distance > 0.0 {
            b3_assert!(0 < simplex_cache.count && simplex_cache.count <= 3);

            manifold.feature = get_triangle_feature(&simplex_cache);
            manifold.normal = output.normal;

            // This feature pair not accurate but maybe it doesn't matter
            manifold.points.push(LocalManifoldPoint {
                point: output.point_b,
                separation: output.distance,
                pair: FEATURE_PAIR_SINGLE,
                triangle_index: 0,
            });
        }
    }
}
