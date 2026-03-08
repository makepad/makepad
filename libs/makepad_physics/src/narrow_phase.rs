use crate::contact::{ContactManifold, ContactPoint};
use crate::rigid_body::RigidBody;
use makepad_math::*;

/// Max vertices during polygon clipping (4 input + up to 4 clip intersections).
const MAX_CLIP_VERTS: usize = 16;

/// Prediction distance: generate contacts when bodies are within this distance
/// of touching, even if not yet penetrating. Matches rapier's default (0.002).
pub const PREDICTION_DISTANCE: f32 = 0.002;
const FEATURE_ID_UNKNOWN: u32 = 0;
const FEATURE_ID_HEADER_VERTEX: u32 = 0b01 << 30;
const FEATURE_ID_HEADER_EDGE: u32 = 0b10 << 30;
const FEATURE_ID_HEADER_FACE: u32 = 0b11 << 30;

fn midpoint(a: Vec3f, b: Vec3f) -> Vec3f {
    (a + b) * 0.5
}

fn world_to_local(body: Option<&RigidBody>, point: Vec3f) -> Vec3f {
    body.map(|body| body.pose.invert().transform_vec3(&point))
        .unwrap_or(point)
}

fn world_dir_to_local(body: Option<&RigidBody>, dir: Vec3f) -> Vec3f {
    body.map(|body| body.pose.orientation.invert().rotate_vec3(&dir))
        .unwrap_or(dir)
}

fn pack_vertex_feature(code: u32) -> u32 {
    FEATURE_ID_HEADER_VERTEX | code
}

fn pack_edge_feature(code: u32) -> u32 {
    FEATURE_ID_HEADER_EDGE | code
}

fn pack_face_feature(code: u32) -> u32 {
    FEATURE_ID_HEADER_FACE | code
}

fn match_contact_data(previous: &ContactManifold, current: &mut ContactManifold) {
    for point in current.points[..current.num_points].iter_mut() {
        for prev_point in previous.points[..previous.num_points].iter() {
            if (point.feature_id_a != FEATURE_ID_UNKNOWN || point.feature_id_b != FEATURE_ID_UNKNOWN)
                && point.feature_id_a == prev_point.feature_id_a
                && point.feature_id_b == prev_point.feature_id_b
            {
                point.normal_impulse = prev_point.normal_impulse;
                point.tangent_impulse = prev_point.tangent_impulse;
                point.warmstart_normal_impulse = prev_point.warmstart_normal_impulse;
                point.warmstart_tangent_impulse = prev_point.warmstart_tangent_impulse;
                point.warmstart_twist_impulse = prev_point.warmstart_twist_impulse;
                break;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct LocalFeature {
    vertices: [Vec3f; 4],
    vids: [u32; 4],
    eids: [u32; 4],
    fid: u32,
    num_vertices: usize,
}

/// Generate contact manifolds for all broad-phase pairs + ground contacts.
/// Clears and reuses the provided buffer.
pub fn narrow_phase(
    bodies: &[RigidBody],
    pairs: &[(usize, usize)],
    ground_y: f32,
    previous_manifolds: &[ContactManifold],
    manifolds: &mut Vec<ContactManifold>,
) {
    manifolds.clear();

    // Body-body contacts
    for &(i, j) in pairs {
        let previous = previous_manifolds
            .iter()
            .find(|prev| prev.body_a == i && prev.body_b == j);
        let mut m = previous.cloned().unwrap_or_default();

        if try_update_cuboid_cuboid_contacts(&bodies[i], &bodies[j], &mut m)
            || cuboid_cuboid_contacts(i, j, &bodies[i], &bodies[j], &mut m)
        {
            if let Some(previous) = previous {
                match_contact_data(previous, &mut m);
            }
            manifolds.push(m);
        }
    }

    // Body-ground contacts
    for (i, body) in bodies.iter().enumerate() {
        if body.is_dynamic() {
            let mut m = ContactManifold::default();
            if cuboid_ground_contacts(i, body, ground_y, &mut m) {
                if let Some(previous) = previous_manifolds
                    .iter()
                    .find(|prev| prev.body_a == usize::MAX && prev.body_b == i)
                {
                    match_contact_data(previous, &mut m);
                }
                manifolds.push(m);
            }
        }
    }
}

/// Cuboid vs infinite ground plane at y = ground_y, normal = (0, 1, 0).
/// Convention: body_a = ground (usize::MAX), body_b = dynamic body.
/// Normal points from A (ground) to B (body) = (0, 1, 0) upward.
fn cuboid_ground_contacts(
    body_idx: usize,
    body: &RigidBody,
    ground_y: f32,
    out: &mut ContactManifold,
) -> bool {
    let he = body.half_extents;
    let normal = vec3f(0.0, 1.0, 0.0);
    let local_normal_b = world_dir_to_local(Some(body), -normal);
    let feature = cuboid_support_face(&[he.x, he.y, he.z], local_normal_b);

    // Ground is body_a, dynamic body is body_b
    out.body_a = usize::MAX;
    out.body_b = body_idx;
    out.local_normal_a = normal;
    out.local_normal_b = local_normal_b;
    out.num_points = 0;

    for i in 0..feature.num_vertices {
        let local_point_b = feature.vertices[i];
        let world_point_b = body.pose.transform_vec3(&local_point_b);
        let dist_to_plane = world_point_b.y - ground_y;
        if dist_to_plane <= PREDICTION_DISTANCE {
            let world_point_a = world_point_b - normal * dist_to_plane;
            out.push_point(ContactPoint {
                world_point: midpoint(world_point_a, world_point_b),
                world_point_a,
                world_point_b,
                local_point_a: world_point_a,
                local_point_b,
                feature_id_a: pack_face_feature(0),
                feature_id_b: feature.vids[i],
                normal,
                penetration: -dist_to_plane,
                normal_impulse: 0.0,
                tangent_impulse: [0.0, 0.0],
                warmstart_normal_impulse: 0.0,
                warmstart_tangent_impulse: [0.0, 0.0],
                warmstart_twist_impulse: 0.0,
            });
        }
    }

    out.num_points > 0
}

/// Compute separation between two cuboids along a given axis (parry's approach).
/// Uses support points for robust separation computation.
/// Returns (separation, oriented_axis) where separation > 0 means separated.
fn compute_separation_wrt_axis(
    a: &RigidBody,
    b: &RigidBody,
    he_a: &[f32; 3],
    he_b: &[f32; 3],
    axes_a: &[Vec3f; 3],
    axes_b: &[Vec3f; 3],
    axis: Vec3f,
) -> (f32, Vec3f) {
    let t = b.pose.position - a.pose.position;
    // Orient axis to point from A toward B
    let signum = if t.dot(axis) >= 0.0 { 1.0 } else { -1.0 };
    let axis = axis * signum;

    // Support point on A in direction +axis (furthest point on A along axis)
    let local_pt_a = cuboid_support_point(he_a, axes_a, axis);
    // Support point on B in direction -axis (furthest point on B opposite to axis)
    let local_pt_b = cuboid_support_point(he_b, axes_b, -axis);

    let pt_a = a.pose.position + local_pt_a;
    let pt_b = b.pose.position + local_pt_b;
    let separation = (pt_b - pt_a).dot(axis);
    (separation, axis)
}

/// Cuboid support point: furthest point along direction, in local offset from center.
fn cuboid_support_point(he: &[f32; 3], axes: &[Vec3f; 3], dir: Vec3f) -> Vec3f {
    let mut result = Vec3f::default();
    for i in 0..3 {
        let sign = if axes[i].dot(dir) >= 0.0 { 1.0 } else { -1.0 };
        result = result + axes[i] * (sign * he[i]);
    }
    result
}

/// Find best face separation for one cuboid's normals (parry's oneway test).
/// Returns (best_separation, best_axis, face_index).
fn find_face_separating_normal_oneway(
    a: &RigidBody,
    b: &RigidBody,
    he_a: &[f32; 3],
    he_b: &[f32; 3],
    axes_a: &[Vec3f; 3],
    axes_b: &[Vec3f; 3],
) -> (f32, Vec3f, usize) {
    let mut best_sep = -f32::MAX;
    let mut best_axis = Vec3f::default();
    let mut best_idx = 0;

    for i in 0..3 {
        let (sep, axis) = compute_separation_wrt_axis(a, b, he_a, he_b, axes_a, axes_b, axes_a[i]);
        if sep > best_sep {
            best_sep = sep;
            best_axis = axis;
            best_idx = i;
        }
    }
    (best_sep, best_axis, best_idx)
}

/// Find best edge-edge separation (parry's twoway test).
/// Returns (best_separation, best_axis, edge_pair_index).
fn find_edge_separating_axis_twoway(
    a: &RigidBody,
    b: &RigidBody,
    he_a: &[f32; 3],
    he_b: &[f32; 3],
    axes_a: &[Vec3f; 3],
    axes_b: &[Vec3f; 3],
) -> (f32, Vec3f, usize) {
    let mut best_sep = -f32::MAX;
    let mut best_axis = Vec3f::default();
    let mut best_idx = 0;

    for i in 0..3 {
        for j in 0..3 {
            let cross = Vec3f::cross(axes_a[i], axes_b[j]);
            let norm = cross.length();
            // Skip degenerate axes (near-parallel edges) — matches parry's epsilon check
            if norm <= f32::EPSILON {
                continue;
            }
            let axis_n = cross * (1.0 / norm);
            let (sep, axis) = compute_separation_wrt_axis(a, b, he_a, he_b, axes_a, axes_b, axis_n);
            if sep > best_sep {
                best_sep = sep;
                best_axis = axis;
                best_idx = i * 3 + j;
            }
        }
    }
    (best_sep, best_axis, best_idx)
}

/// SAT-based cuboid vs cuboid contact generation.
/// Follows parry's three-pass approach: face normals A, face normals B, edge-edge.
fn cuboid_cuboid_contacts(
    idx_a: usize,
    idx_b: usize,
    a: &RigidBody,
    b: &RigidBody,
    out: &mut ContactManifold,
) -> bool {
    let rot_a = Mat3f::from_quat(a.pose.orientation);
    let rot_b = Mat3f::from_quat(b.pose.orientation);

    let axes_a = [rot_a.c0, rot_a.c1, rot_a.c2];
    let axes_b = [rot_b.c0, rot_b.c1, rot_b.c2];

    let he_a = [a.half_extents.x, a.half_extents.y, a.half_extents.z];
    let he_b = [b.half_extents.x, b.half_extents.y, b.half_extents.z];

    // Pass 1: face normals of A
    let (sep1, axis1, face_idx1) =
        find_face_separating_normal_oneway(a, b, &he_a, &he_b, &axes_a, &axes_b);
    if sep1 > PREDICTION_DISTANCE {
        return false;
    }

    // Pass 2: face normals of B
    let (sep2, axis2_local, face_idx2) =
        find_face_separating_normal_oneway(b, a, &he_b, &he_a, &axes_b, &axes_a);
    if sep2 > PREDICTION_DISTANCE {
        return false;
    }

    // Pass 3: edge-edge axes
    let (sep3, axis3, edge_idx) =
        find_edge_separating_axis_twoway(a, b, &he_a, &he_b, &axes_a, &axes_b);
    if sep3 > PREDICTION_DISTANCE {
        return false;
    }

    // Select best axis — matching parry's selection logic exactly:
    //   best = sep1 (cuboid A faces)
    //   if sep2 > sep1 && sep2 > sep3: best = sep2 (cuboid B faces)
    //   else if sep3 > sep1: best = sep3 (edge-edge)
    let mut best_axis;
    if sep2 > sep1 && sep2 > sep3 {
        // Cuboid B's face normal wins — transform back to A's frame convention
        // (parry: best_sep.1 = pos12.rotation * -sep2.1)
        best_axis = -axis2_local; // flip because axis2_local points from B, we want from A to B
    } else if sep3 > sep1 {
        // Edge-edge wins
        best_axis = axis3;
    } else {
        // Cuboid A's face normal wins (default)
        best_axis = axis1;
    }

    // Ensure normal points from A to B
    let t = b.pose.position - a.pose.position;
    if t.dot(best_axis) < 0.0 {
        best_axis = -best_axis;
    }

    out.body_a = idx_a;
    out.body_b = idx_b;
    out.local_normal_a = world_dir_to_local(Some(a), best_axis);
    out.local_normal_b = world_dir_to_local(Some(b), -best_axis);
    out.num_points = 0;

    // penetration = -separation (positive means overlapping)
    let local_normal_a = world_dir_to_local(Some(a), best_axis);
    let local_normal_b = world_dir_to_local(Some(b), -best_axis);
    generate_feature_contacts(a, b, &he_a, &he_b, local_normal_a, local_normal_b, out);

    out.num_points > 0
}

fn cuboid_support_face(he: &[f32; 3], local_dir: Vec3f) -> LocalFeature {
    let abs_dir = vec3f(local_dir.x.abs(), local_dir.y.abs(), local_dir.z.abs());
    let imax = if abs_dir.x >= abs_dir.y && abs_dir.x >= abs_dir.z {
        0
    } else if abs_dir.y >= abs_dir.z {
        1
    } else {
        2
    };
    let sign = if [local_dir.x, local_dir.y, local_dir.z][imax] >= 0.0 {
        1.0
    } else {
        -1.0
    };

    fn vid(i: u32) -> u32 {
        pack_vertex_feature(i * 2)
    }

    let sign_index = if sign > 0.0 { 1 } else { 0 };
    let vertices = match imax {
        0 => [
            vec3f(he[0] * sign, he[1], he[2]),
            vec3f(he[0] * sign, -he[1], he[2]),
            vec3f(he[0] * sign, -he[1], -he[2]),
            vec3f(he[0] * sign, he[1], -he[2]),
        ],
        1 => [
            vec3f(he[0], he[1] * sign, he[2]),
            vec3f(-he[0], he[1] * sign, he[2]),
            vec3f(-he[0], he[1] * sign, -he[2]),
            vec3f(he[0], he[1] * sign, -he[2]),
        ],
        2 => [
            vec3f(he[0], he[1], he[2] * sign),
            vec3f(he[0], -he[1], he[2] * sign),
            vec3f(-he[0], -he[1], he[2] * sign),
            vec3f(-he[0], he[1], he[2] * sign),
        ],
        _ => unreachable!(),
    };

    let vids = match imax {
        0 => [
            [vid(0b000), vid(0b010), vid(0b011), vid(0b001)],
            [vid(0b100), vid(0b110), vid(0b111), vid(0b101)],
        ][sign_index],
        1 => [
            [vid(0b000), vid(0b100), vid(0b101), vid(0b001)],
            [vid(0b010), vid(0b110), vid(0b111), vid(0b011)],
        ][sign_index],
        2 => [
            [vid(0b000), vid(0b010), vid(0b110), vid(0b100)],
            [vid(0b001), vid(0b011), vid(0b111), vid(0b101)],
        ][sign_index],
        _ => unreachable!(),
    };

    let eids = match imax {
        0 => [
            [
                pack_edge_feature(0b11_010_000),
                pack_edge_feature(0b11_011_010),
                pack_edge_feature(0b11_011_001),
                pack_edge_feature(0b11_001_000),
            ],
            [
                pack_edge_feature(0b11_110_100),
                pack_edge_feature(0b11_111_110),
                pack_edge_feature(0b11_111_101),
                pack_edge_feature(0b11_101_100),
            ],
        ][sign_index],
        1 => [
            [
                pack_edge_feature(0b11_100_000),
                pack_edge_feature(0b11_101_100),
                pack_edge_feature(0b11_101_001),
                pack_edge_feature(0b11_001_000),
            ],
            [
                pack_edge_feature(0b11_110_010),
                pack_edge_feature(0b11_111_110),
                pack_edge_feature(0b11_111_011),
                pack_edge_feature(0b11_011_010),
            ],
        ][sign_index],
        2 => [
            [
                pack_edge_feature(0b11_010_000),
                pack_edge_feature(0b11_110_010),
                pack_edge_feature(0b11_110_100),
                pack_edge_feature(0b11_100_000),
            ],
            [
                pack_edge_feature(0b11_011_001),
                pack_edge_feature(0b11_111_011),
                pack_edge_feature(0b11_111_101),
                pack_edge_feature(0b11_101_001),
            ],
        ][sign_index],
        _ => unreachable!(),
    };

    let fid = pack_face_feature((imax + sign_index * 3 + 10) as u32);

    LocalFeature {
        vertices,
        vids,
        eids,
        fid,
        num_vertices: 4,
    }
}

fn try_update_cuboid_cuboid_contacts(
    a: &RigidBody,
    b: &RigidBody,
    manifold: &mut ContactManifold,
) -> bool {
    if manifold.num_points == 0 {
        return false;
    }

    let local_n1 = manifold.local_normal_a;
    let local_n2_in_a = world_dir_to_local(
        Some(a),
        b.pose.orientation.rotate_vec3(&manifold.local_normal_b),
    );

    const ANGLE_DOT_THRESHOLD: f32 = 0.9998477;
    const DIST_SQ_THRESHOLD: f32 = 1.0e-6;

    if -local_n1.dot(local_n2_in_a) < ANGLE_DOT_THRESHOLD {
        return false;
    }

    let world_normal = a.pose.orientation.rotate_vec3(&local_n1);
    for point in manifold.points[..manifold.num_points].iter_mut() {
        let local_p2_in_a = b_local_to_a_local(a, b, point.local_point_b);
        let dpt = local_p2_in_a - point.local_point_a;
        let dist = dpt.dot(local_n1);

        if dist * point.penetration > 0.0 {
            return false;
        }

        let new_local_p1 = local_p2_in_a - local_n1 * dist;
        if (point.local_point_a - new_local_p1).length_squared() > DIST_SQ_THRESHOLD {
            return false;
        }

        point.local_point_a = new_local_p1;
        point.world_point_a = local_to_world(a, new_local_p1);
        point.world_point_b = local_to_world(b, point.local_point_b);
        point.world_point = midpoint(point.world_point_a, point.world_point_b);
        point.normal = world_normal;
        point.penetration = -dist;
    }

    true
}

fn local_to_world(body: &RigidBody, point: Vec3f) -> Vec3f {
    body.pose.transform_vec3(&point)
}

fn b_local_to_a_local(a: &RigidBody, b: &RigidBody, point_b: Vec3f) -> Vec3f {
    world_to_local(Some(a), local_to_world(b, point_b))
}

fn a_local_to_b_local(a: &RigidBody, b: &RigidBody, point_a: Vec3f) -> Vec3f {
    world_to_local(Some(b), local_to_world(a, point_a))
}

fn orthonormal_basis(normal: Vec3f) -> (Vec3f, Vec3f) {
    let tangent1 = if normal.x.abs() < 0.9 {
        Vec3f::cross(normal, vec3f(1.0, 0.0, 0.0)).normalize()
    } else {
        Vec3f::cross(normal, vec3f(0.0, 1.0, 0.0)).normalize()
    };
    let tangent2 = Vec3f::cross(normal, tangent1);
    (tangent1, tangent2)
}

fn project_to_basis(point: Vec3f, basis1: Vec3f, basis2: Vec3f) -> Vec2f {
    vec2(point.dot(basis1), point.dot(basis2))
}

fn perp_dot(a: Vec2f, b: Vec2f) -> f32 {
    a.x * b.y - a.y * b.x
}

fn point_in_convex_polygon(point: Vec2f, polygon: &[Vec2f]) -> bool {
    if polygon.is_empty() {
        return false;
    }

    let last = polygon.len() - 1;
    let mut sign = perp_dot(polygon[0] - polygon[last], point - polygon[last]);
    for i in 0..last {
        let new_sign = perp_dot(polygon[i + 1] - polygon[i], point - polygon[i]);
        if sign == 0.0 {
            sign = new_sign;
        } else if sign * new_sign < 0.0 {
            return false;
        }
    }

    true
}

fn closest_points_line2d(edge1: [Vec2f; 2], edge2: [Vec2f; 2]) -> Option<(f32, f32)> {
    let dir1 = edge1[1] - edge1[0];
    let dir2 = edge2[1] - edge2[0];
    let r = edge1[0] - edge2[0];

    let a = dir1.lengthsquared();
    let e = dir2.lengthsquared();
    let f = dir2.x * r.x + dir2.y * r.y;
    let eps = f32::EPSILON;

    if a <= eps && e <= eps {
        Some((0.0, 0.0))
    } else if a <= eps {
        Some((0.0, f / e))
    } else {
        let c = dir1.x * r.x + dir1.y * r.y;
        if e <= eps {
            Some((-c / a, 0.0))
        } else {
            let b = dir1.x * dir2.x + dir1.y * dir2.y;
            let ae = a * e;
            let bb = b * b;
            let denom = ae - bb;
            if denom <= eps {
                None
            } else {
                let s = (b * f - c * e) / denom;
                Some((s, (b * s + f) / e))
            }
        }
    }
}

fn push_local_contact(
    a: &RigidBody,
    b: &RigidBody,
    local_point_a: Vec3f,
    local_point_b_in_a: Vec3f,
    local_normal_a: Vec3f,
    feature_id_a: u32,
    feature_id_b: u32,
    out: &mut ContactManifold,
) {
    let local_point_b = a_local_to_b_local(a, b, local_point_b_in_a);
    let world_point_a = local_to_world(a, local_point_a);
    let world_point_b = local_to_world(b, local_point_b);
    let world_normal = a.pose.orientation.rotate_vec3(&local_normal_a);
    let dist = (local_point_b_in_a - local_point_a).dot(local_normal_a);
    out.push_point(ContactPoint {
        world_point: midpoint(world_point_a, world_point_b),
        world_point_a,
        world_point_b,
        local_point_a,
        local_point_b,
        feature_id_a,
        feature_id_b,
        normal: world_normal,
        penetration: -dist,
        normal_impulse: 0.0,
        tangent_impulse: [0.0, 0.0],
        warmstart_normal_impulse: 0.0,
        warmstart_tangent_impulse: [0.0, 0.0],
        warmstart_twist_impulse: 0.0,
    });
}

fn generate_feature_contacts(
    a: &RigidBody,
    b: &RigidBody,
    he_a: &[f32; 3],
    he_b: &[f32; 3],
    local_normal_a: Vec3f,
    local_normal_b: Vec3f,
    out: &mut ContactManifold,
) {
    let feature1 = cuboid_support_face(he_a, local_normal_a);
    let feature2 = cuboid_support_face(he_b, local_normal_b);
    let (basis1, basis2) = orthonormal_basis(local_normal_a);

    let mut projected_face1 = [Vec2f::default(); 4];
    let mut vertices2_1 = [Vec3f::default(); 4];
    let mut projected_face2 = [Vec2f::default(); 4];
    for i in 0..feature1.num_vertices {
        projected_face1[i] = project_to_basis(feature1.vertices[i], basis1, basis2);
    }
    for i in 0..feature2.num_vertices {
        vertices2_1[i] = b_local_to_a_local(a, b, feature2.vertices[i]);
        projected_face2[i] = project_to_basis(vertices2_1[i], basis1, basis2);
    }

    let normal2_1 =
        Vec3f::cross(vertices2_1[2] - vertices2_1[1], vertices2_1[0] - vertices2_1[1]);
    let denom2 = normal2_1.dot(local_normal_a);
    if denom2.abs() > f32::EPSILON {
        for i in 0..feature1.num_vertices {
            let p1 = projected_face1[i];
            if point_in_convex_polygon(p1, &projected_face2[..feature2.num_vertices]) {
                let dist = (vertices2_1[0] - feature1.vertices[i]).dot(normal2_1) / denom2;
                let local_p1 = feature1.vertices[i];
                let local_p2_1 = local_p1 + local_normal_a * dist;
                push_local_contact(
                    a,
                    b,
                    local_p1,
                    local_p2_1,
                    local_normal_a,
                    feature1.vids[i],
                    feature2.fid,
                    out,
                );
            }
        }
    }

    let normal1 = Vec3f::cross(
        feature1.vertices[2] - feature1.vertices[1],
        feature1.vertices[0] - feature1.vertices[1],
    );
    let denom1 = -normal1.dot(local_normal_a);
    if denom1.abs() > f32::EPSILON {
        for i in 0..feature2.num_vertices {
            let p2 = projected_face2[i];
            if point_in_convex_polygon(p2, &projected_face1[..feature1.num_vertices]) {
                let local_p2_1 = vertices2_1[i];
                let dist = (feature1.vertices[0] - local_p2_1).dot(normal1) / denom1;
                let local_p1 = local_p2_1 - local_normal_a * dist;
                push_local_contact(
                    a,
                    b,
                    local_p1,
                    local_p2_1,
                    local_normal_a,
                    feature1.fid,
                    feature2.vids[i],
                    out,
                );
            }
        }
    }

    for j in 0..feature2.num_vertices {
        let projected_edge2 = [
            projected_face2[j],
            projected_face2[(j + 1) % feature2.num_vertices],
        ];
        for i in 0..feature1.num_vertices {
            let projected_edge1 = [
                projected_face1[i],
                projected_face1[(i + 1) % feature1.num_vertices],
            ];
            if let Some((bcoord1, bcoord2)) = closest_points_line2d(projected_edge1, projected_edge2)
            {
                if bcoord1 > 0.0 && bcoord1 < 1.0 && bcoord2 > 0.0 && bcoord2 < 1.0 {
                    let edge1_a = feature1.vertices[i];
                    let edge1_b = feature1.vertices[(i + 1) % feature1.num_vertices];
                    let edge2_a = vertices2_1[j];
                    let edge2_b = vertices2_1[(j + 1) % feature2.num_vertices];
                    let local_p1 = edge1_a * (1.0 - bcoord1) + edge1_b * bcoord1;
                    let local_p2_1 = edge2_a * (1.0 - bcoord2) + edge2_b * bcoord2;
                    push_local_contact(
                        a,
                        b,
                        local_p1,
                        local_p2_1,
                        local_normal_a,
                        feature1.eids[i],
                        feature2.eids[j],
                        out,
                    );
                }
            }
        }
    }
}

/// Select up to `max_keep` best-distributed contacts from a manifold, matching Rapier's
/// narrow-phase reduction stage without mutating the manifold itself.
pub fn select_contact_indices(
    manifold: &ContactManifold,
    max_keep: usize,
    selected: &mut [usize; 4],
    num_selected: &mut usize,
) {
    if manifold.num_points <= max_keep {
        *num_selected = manifold.num_points;
        for (i, slot) in selected.iter_mut().take(*num_selected).enumerate() {
            *slot = i;
        }
        return;
    }

    *selected = [usize::MAX; 4];

    // 1. Keep the deepest contact.
    let mut deepest_penetration = -f32::MAX;
    for i in 0..manifold.num_points {
        if manifold.points[i].penetration > deepest_penetration {
            deepest_penetration = manifold.points[i].penetration;
            (*selected)[0] = i;
        }
    }

    if (*selected)[0] == usize::MAX {
        *num_selected = 0;
        return;
    }

    // 2. Keep the point furthest away from the deepest contact on body A.
    let selected_a = manifold.points[(*selected)[0]].local_point_a;
    let mut furthest_dist = -f32::MAX;
    for i in 0..manifold.num_points {
        if i == (*selected)[0] || manifold.points[i].penetration < -PREDICTION_DISTANCE {
            continue;
        }
        let dist = (manifold.points[i].local_point_a - selected_a).length_squared();
        if dist > furthest_dist {
            furthest_dist = dist;
            (*selected)[1] = i;
        }
    }

    *num_selected = if (*selected)[1] == usize::MAX {
        1
    } else {
        let selected_b = manifold.points[(*selected)[1]].local_point_a;
        if selected_a == selected_b {
            1
        } else {
            let selected_ab = selected_b - selected_a;
            let tangent = Vec3f::cross(selected_ab, manifold.local_normal_a);

            let mut min_dot = f32::MAX;
            let mut max_dot = -f32::MAX;

            for i in 0..manifold.num_points {
                if i == (*selected)[0]
                    || i == (*selected)[1]
                    || manifold.points[i].penetration < -PREDICTION_DISTANCE
                {
                    continue;
                }

                let dot = (manifold.points[i].local_point_a - selected_a).dot(tangent);
                if dot < min_dot {
                    min_dot = dot;
                    (*selected)[2] = i;
                }
                if dot > max_dot {
                    max_dot = dot;
                    (*selected)[3] = i;
                }
            }

            if (*selected)[2] == usize::MAX {
                2
            } else if (*selected)[2] == (*selected)[3] {
                3
            } else {
                4
            }
        }
    };
}
