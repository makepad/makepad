// Port of box3d/src/manifold.h + manifold.c
// Dirk Gregorius contributed portions of the original C code.

use crate::b3_assert;
use crate::b3_validate;
use crate::math_functions::{
    abs_float, cross, dot, length, length_squared, mul_add, mul_sv, neg, normalize, sub, Plane, Vec3,
};
use crate::math_internal::plane_separation;
use crate::types::{FeaturePair, HullData};

pub const MAX_CLIP_POINTS: usize = 64;

#[derive(Clone, Copy, Debug, Default)]
pub struct FaceQuery {
    pub separation: f32,
    pub face_index: i32,
    pub vertex_index: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EdgeQuery {
    pub separation: f32,
    pub index_a: i32,
    pub index_b: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClipVertex {
    pub position: Vec3,
    pub separation: f32,
    pub pair: FeaturePair,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FeatureOwner {
    ShapeA = 0,
    ShapeB = 1,
}

/// For single point contact, such as sphere-sphere, sphere-capsule, sphere-triangle.
pub const FEATURE_PAIR_SINGLE: FeaturePair = FeaturePair { owner1: 0, index1: 0, owner2: 0, index2: 0 };

#[inline]
pub fn make_feature_pair(owner1: FeatureOwner, index1: i32, owner2: FeatureOwner, index2: i32) -> FeaturePair {
    b3_assert!(0 <= index1 && index1 <= u8::MAX as i32);
    b3_assert!(0 <= index2 && index2 <= u8::MAX as i32);

    FeaturePair {
        owner1: owner1 as u8,
        index1: index1 as u8,
        owner2: owner2 as u8,
        index2: index2 as u8,
    }
}

#[inline]
pub fn make_feature_id(pair: FeaturePair) -> u32 {
    ((pair.owner1 as u32) << 24) | ((pair.index1 as u32) << 16) | ((pair.owner2 as u32) << 8) | (pair.index2 as u32)
}

// p1 : origin on edge 1
// e1 : edge 1
// c1 : shape 1 centroid
// p2 : origin on edge 2
// e2 : edge 2
// c2 : shape 2 centroid
pub fn edge_edge_separation(p1: Vec3, e1: Vec3, c1: Vec3, p2: Vec3, e2: Vec3, c2: Vec3) -> f32 {
    // Build search direction
    let u = cross(e1, e2);
    let len = length(u);

    // Skip near parallel edges: |e1 x e1| = sin(alpha) * |e1| * |e2|
    const K_TOLERANCE: f32 = 0.005;
    if len < K_TOLERANCE * (length_squared(e1) * length_squared(e2)).sqrt() {
        return -f32::MAX;
    }

    if len * len < 1000.0 * f32::MIN_POSITIVE {
        return -f32::MAX;
    }

    let mut n = mul_sv(1.0 / len, u);

    // Make sure normal points away from the first shape
    // For a triangle, it is possible that N is aligned with the triangle normal and the sign
    // value can be close to zero and flicker between small negative and positive values, leading to
    // an incorrect separation value. So we assume the other hull has some volume and pick the most
    // significant sign value to orient N.
    let sign1 = dot(n, sub(p1, c1));
    let sign2 = dot(n, sub(p2, c2));
    if abs_float(sign1) > abs_float(sign2) {
        if sign1 < 0.0 {
            n = neg(n);
        }
    } else if sign2 > 0.0 {
        n = neg(n);
    }

    // s = Dot(n, p2) - d = Dot(n, p2) - Dot(n, p1) = Dot(n, p2 - p1)
    dot(n, sub(p2, p1))
}

// This was extended to make the wedge shape get the correct incident face.
// Instead of looking directly for the most anti-parallel face, we first find the closest
// vertex (passed in). Then we look for all edges coming out of that vertex and look for
// the edge that is most perpendicular to the reference normal.
// Then from that edge, we select the adjacent face that is most anti-parallel to the
// reference normal.
pub fn find_incident_face(hull: &HullData, ref_normal: Vec3, vertex_index: i32) -> i32 {
    let vertices = &hull.vertices;
    let edges = &hull.edges;
    let planes = &hull.planes;
    let points = &hull.points;

    let mut min_edge_index = -1;
    let mut min_edge_projection = f32::MAX;

    let vertex = vertices[vertex_index as usize];

    let mut edge_index = vertex.edge as i32;
    let mut edge = edges[edge_index as usize];
    let edge_origin = points[edge.origin as usize];
    b3_assert!(edge.origin as i32 == vertex_index);

    loop {
        let twin = edges[edge.twin as usize];
        let twin_origin = points[twin.origin as usize];

        let axis = normalize(sub(twin_origin, edge_origin));
        let edge_projection = abs_float(dot(axis, ref_normal));
        if edge_projection < min_edge_projection {
            min_edge_index = edge_index;
            min_edge_projection = edge_projection;
        }

        edge_index = twin.next as i32;
        edge = edges[edge_index as usize];
        b3_assert!(edge.origin as i32 == vertex_index);

        if edge_index == vertex.edge as i32 {
            break;
        }
    }
    b3_assert!(min_edge_index >= 0);

    let min_edge = edges[min_edge_index as usize];
    let min_face_index1 = min_edge.face as i32;
    let min_plane1 = planes[min_face_index1 as usize];

    let min_twin = edges[min_edge.twin as usize];
    let min_face_index2 = min_twin.face as i32;
    let min_plane2 = planes[min_face_index2 as usize];

    if dot(min_plane1.normal, ref_normal) < dot(min_plane2.normal, ref_normal) {
        min_face_index1
    } else {
        min_face_index2
    }
}

// This logic seems wrong but it is designed so that choosing
// face A or B as the reference face does not change the resulting
// feature pair. This way the contact impulses are persisted even
// if there is reference face flip-flop.
pub fn flip_pair(pair: FeaturePair) -> FeaturePair {
    b3_assert!(pair.owner1 == 0 || pair.owner1 == 1);
    b3_assert!(pair.owner2 == 0 || pair.owner2 == 1);
    let mut pair = pair;
    std::mem::swap(&mut pair.owner1, &mut pair.owner2);
    pair.owner1 = 1 - pair.owner1;
    pair.owner2 = 1 - pair.owner2;
    std::mem::swap(&mut pair.index1, &mut pair.index2);
    pair
}

// C: only compiled with B3_ENABLE_VALIDATION; used through b3_validate! so the
// call compiles away in release builds.
pub fn validate_polygon(polygon: &[ClipVertex], count: i32) -> bool {
    // Empty polygons are valid (we can clip away all points when re-constructing
    // manifolds from cache)
    if count == 0 {
        return true;
    }

    // Validate that incoming and outgoing edges match
    let mut vertex1 = polygon[count as usize - 1];
    for i in 0..count {
        let vertex2 = polygon[i as usize];

        if vertex1.pair.owner2 != vertex2.pair.owner1 {
            return false;
        }

        if vertex1.pair.index2 != vertex2.pair.index1 {
            return false;
        }

        vertex1 = vertex2;
    }

    true
}

pub fn clip_polygon(out: &mut [ClipVertex], polygon: &[ClipVertex], count: i32, clip_plane: Plane, edge: i32, ref_plane: Plane) -> i32 {
    b3_assert!(count >= 3);

    let mut vertex1 = polygon[count as usize - 1];
    let mut distance1 = plane_separation(clip_plane, vertex1.position);
    let mut out_count: i32 = 0;

    for index in 0..count {
        let vertex2 = polygon[index as usize];
        let distance2 = plane_separation(clip_plane, vertex2.position);

        // Clip edge against plane (Sutherland-Hodgman clipping)
        if distance1 <= 0.0 && distance2 <= 0.0 {
            // Both vertices are behind the plane - keep vertex2
            out[out_count as usize] = vertex2;
            out_count += 1;
        } else if distance1 <= 0.0 && distance2 > 0.0 {
            // Vertex1 is behind of the plane, vertex2 is in front -> intersection point
            let fraction = distance1 / (distance1 - distance2);
            let position = mul_add(vertex1.position, fraction, sub(vertex2.position, vertex1.position));

            // Keep intersection point and adjust outgoing edge
            let mut vertex = ClipVertex::default();
            vertex.position = position;
            vertex.separation = plane_separation(ref_plane, position);
            vertex.pair = vertex2.pair;
            vertex.pair.owner2 = FeatureOwner::ShapeA as u8;
            vertex.pair.index2 = edge as u8;
            out[out_count as usize] = vertex;
            out_count += 1;
        } else if distance2 <= 0.0 && distance1 > 0.0 {
            // Vertex1 is in front, vertex2 is behind of the plane, -> intersection point
            let fraction = distance1 / (distance1 - distance2);
            let position = mul_add(vertex1.position, fraction, sub(vertex2.position, vertex1.position));

            // Keep intersection point and adjust incoming edge
            let mut vertex = ClipVertex::default();
            vertex.position = position;
            vertex.separation = plane_separation(ref_plane, position);
            vertex.pair = vertex1.pair;
            vertex.pair.owner1 = FeatureOwner::ShapeA as u8;
            vertex.pair.index1 = edge as u8;
            out[out_count as usize] = vertex;
            out_count += 1;

            // And also keep vertex2
            out[out_count as usize] = vertex2;
            out_count += 1;
        }

        // Keep vertex2 as starting vertex for next edge
        vertex1 = vertex2;
        distance1 = distance2;
    }

    b3_validate!(validate_polygon(out, out_count));

    out_count
}
