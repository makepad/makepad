use crate::contact::{ContactManifold, ContactPoint, MAX_CONTACTS};
use crate::rigid_body::RigidBody;
use makepad_math::*;

/// Max vertices during polygon clipping (4 input + up to 4 clip intersections).
const MAX_CLIP_VERTS: usize = 16;

/// Prediction distance: generate contacts when bodies are within this distance
/// of touching, even if not yet penetrating. Matches rapier's default (0.002).
pub const PREDICTION_DISTANCE: f32 = 0.002;

fn midpoint(a: Vec3f, b: Vec3f) -> Vec3f {
    (a + b) * 0.5
}

fn world_to_local(body: Option<&RigidBody>, point: Vec3f) -> Vec3f {
    body.map(|body| body.pose.invert().transform_vec3(&point))
        .unwrap_or(point)
}

/// Generate contact manifolds for all broad-phase pairs + ground contacts.
/// Clears and reuses the provided buffer.
pub fn narrow_phase(
    bodies: &[RigidBody],
    pairs: &[(usize, usize)],
    ground_y: f32,
    manifolds: &mut Vec<ContactManifold>,
) {
    manifolds.clear();

    // Body-body contacts
    for &(i, j) in pairs {
        let mut m = ContactManifold::default();
        if cuboid_cuboid_contacts(i, j, &bodies[i], &bodies[j], &mut m) {
            manifolds.push(m);
        }
    }

    // Body-ground contacts
    for (i, body) in bodies.iter().enumerate() {
        if body.is_dynamic() {
            let mut m = ContactManifold::default();
            if cuboid_ground_contacts(i, body, ground_y, &mut m) {
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

    // 8 corners in local space
    let signs: [[f32; 3]; 8] = [
        [-1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [-1.0, 1.0, 1.0],
        [1.0, 1.0, 1.0],
    ];

    // Ground is body_a, dynamic body is body_b
    out.body_a = usize::MAX;
    out.body_b = body_idx;
    out.num_points = 0;

    for s in &signs {
        let corner_local = vec3f(he.x * s[0], he.y * s[1], he.z * s[2]);
        let corner_world = body.pose.transform_vec3(&corner_local);
        let depth = ground_y - corner_world.y;
        if depth > -PREDICTION_DISTANCE {
            // depth > 0 means actual penetration, depth in (-PREDICTION_DISTANCE, 0] means
            // predicted contact (gap smaller than prediction distance)
            let world_point_a = vec3f(corner_world.x, ground_y, corner_world.z);
            let world_point_b = corner_world;
            out.push_point(ContactPoint {
                world_point: midpoint(world_point_a, world_point_b),
                world_point_a,
                world_point_b,
                local_point_a: world_point_a,
                local_point_b: world_to_local(Some(body), world_point_b),
                normal,
                penetration: depth, // negative = gap (predicted), positive = overlap
                normal_impulse: 0.0,
                tangent_impulse: [0.0, 0.0],
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
    let (best_sep, mut best_axis, best_axis_idx);
    if sep2 > sep1 && sep2 > sep3 {
        // Cuboid B's face normal wins — transform back to A's frame convention
        // (parry: best_sep.1 = pos12.rotation * -sep2.1)
        best_sep = sep2;
        best_axis = -axis2_local; // flip because axis2_local points from B, we want from A to B
        best_axis_idx = (3 + face_idx2) as i32;
    } else if sep3 > sep1 {
        // Edge-edge wins
        best_sep = sep3;
        best_axis = axis3;
        best_axis_idx = (6 + edge_idx) as i32;
    } else {
        // Cuboid A's face normal wins (default)
        best_sep = sep1;
        best_axis = axis1;
        best_axis_idx = face_idx1 as i32;
    }

    // Ensure normal points from A to B
    let t = b.pose.position - a.pose.position;
    if t.dot(best_axis) < 0.0 {
        best_axis = -best_axis;
    }

    out.body_a = idx_a;
    out.body_b = idx_b;
    out.num_points = 0;

    // penetration = -separation (positive means overlapping)
    let penetration = -best_sep;

    if best_axis_idx < 6 {
        generate_face_contacts(
            a,
            b,
            &axes_a,
            &axes_b,
            &he_a,
            &he_b,
            best_axis,
            best_axis_idx,
            out,
        );
    } else {
        generate_edge_contact(
            a,
            b,
            &axes_a,
            &axes_b,
            &he_a,
            &he_b,
            best_axis,
            penetration,
            best_axis_idx,
            out,
        );
    }

    out.num_points > 0
}

/// Face-face/face-edge contacts via Sutherland-Hodgman clipping.
fn generate_face_contacts(
    a: &RigidBody,
    b: &RigidBody,
    axes_a: &[Vec3f; 3],
    axes_b: &[Vec3f; 3],
    he_a: &[f32; 3],
    he_b: &[f32; 3],
    normal: Vec3f,
    axis_idx: i32,
    out: &mut ContactManifold,
) {
    let (ref_body, inc_body, ref_axes, inc_axes, ref_he, inc_he, ref_is_a) = if axis_idx < 3 {
        (a, b, axes_a, axes_b, he_a, he_b, true)
    } else {
        (b, a, axes_b, axes_a, he_b, he_a, false)
    };

    let ref_axis_local = if axis_idx < 3 {
        axis_idx as usize
    } else {
        (axis_idx - 3) as usize
    };
    let ref_normal = if ref_is_a { normal } else { -normal };

    // Find incident face: most anti-parallel to ref_normal
    let mut min_dot = f32::MAX;
    let mut inc_axis_local = 0usize;
    let mut inc_sign = 1.0f32;
    for k in 0..3 {
        let d = ref_normal.dot(inc_axes[k]);
        if d < min_dot {
            min_dot = d;
            inc_axis_local = k;
            inc_sign = 1.0;
        }
        if -d < min_dot {
            min_dot = -d;
            inc_axis_local = k;
            inc_sign = -1.0;
        }
    }

    // Incident face polygon (4 verts) — fixed-size array
    let inc_center =
        inc_body.pose.position + inc_axes[inc_axis_local] * (inc_sign * inc_he[inc_axis_local]);
    let iu = (inc_axis_local + 1) % 3;
    let iv = (inc_axis_local + 2) % 3;
    let u = inc_axes[iu] * inc_he[iu];
    let v = inc_axes[iv] * inc_he[iv];

    let mut poly = [Vec3f::default(); MAX_CLIP_VERTS];
    let mut poly_tmp = [Vec3f::default(); MAX_CLIP_VERTS];
    poly[0] = inc_center - u - v;
    poly[1] = inc_center + u - v;
    poly[2] = inc_center + u + v;
    poly[3] = inc_center - u + v;
    let mut count = 4usize;

    // Clip against 4 side planes of reference face
    let ru = (ref_axis_local + 1) % 3;
    let rv = (ref_axis_local + 2) % 3;

    let clip_normals = [ref_axes[ru], -ref_axes[ru], ref_axes[rv], -ref_axes[rv]];
    let clip_ds = [
        ref_body.pose.position.dot(ref_axes[ru]) + ref_he[ru],
        ref_body.pose.position.dot(-ref_axes[ru]) + ref_he[ru],
        ref_body.pose.position.dot(ref_axes[rv]) + ref_he[rv],
        ref_body.pose.position.dot(-ref_axes[rv]) + ref_he[rv],
    ];

    for plane_idx in 0..4 {
        count = clip_polygon(
            &poly[..count],
            clip_normals[plane_idx],
            clip_ds[plane_idx],
            &mut poly_tmp,
        );
        if count == 0 {
            return;
        }
        poly[..count].copy_from_slice(&poly_tmp[..count]);
    }

    let ref_plane_d = ref_body.pose.position.dot(ref_normal) + ref_he[ref_axis_local];

    for i in 0..count {
        let p = poly[i];
        let dist = p.dot(ref_normal) - ref_plane_d;
        // dist < 0 = actual penetration, dist in [0, PREDICTION_DISTANCE) = predicted contact
        if dist < PREDICTION_DISTANCE {
            let ref_world = p - ref_normal * dist;
            let (world_point_a, world_point_b) = if ref_is_a {
                (ref_world, p)
            } else {
                (p, ref_world)
            };
            out.push_point(ContactPoint {
                world_point: midpoint(world_point_a, world_point_b),
                world_point_a,
                world_point_b,
                local_point_a: world_to_local(Some(a), world_point_a),
                local_point_b: world_to_local(Some(b), world_point_b),
                normal,
                penetration: -dist, // positive = penetrating, negative = gap
                normal_impulse: 0.0,
                tangent_impulse: [0.0, 0.0],
            });
        }
    }

    // Reduce to 4 best if we got more
    if out.num_points > 4 {
        reduce_contacts(out, 4);
    }
}

/// Clip polygon against plane (keep side where dot(v, normal) <= d).
/// Writes into `out` array, returns new vertex count.
fn clip_polygon(
    verts: &[Vec3f],
    normal: Vec3f,
    d: f32,
    out: &mut [Vec3f; MAX_CLIP_VERTS],
) -> usize {
    let n = verts.len();
    let mut count = 0usize;
    for i in 0..n {
        let v0 = verts[i];
        let v1 = verts[(i + 1) % n];
        let d0 = v0.dot(normal) - d;
        let d1 = v1.dot(normal) - d;

        if d0 <= 0.0 {
            if count < MAX_CLIP_VERTS {
                out[count] = v0;
                count += 1;
            }
            if d1 > 0.0 && count < MAX_CLIP_VERTS {
                let t = d0 / (d0 - d1);
                out[count] = Vec3f::from_lerp(v0, v1, t);
                count += 1;
            }
        } else if d1 <= 0.0 && count < MAX_CLIP_VERTS {
            let t = d0 / (d0 - d1);
            out[count] = Vec3f::from_lerp(v0, v1, t);
            count += 1;
        }
    }
    count
}

/// Single contact point for edge-edge collision.
fn generate_edge_contact(
    a: &RigidBody,
    b: &RigidBody,
    axes_a: &[Vec3f; 3],
    axes_b: &[Vec3f; 3],
    he_a: &[f32; 3],
    he_b: &[f32; 3],
    normal: Vec3f,
    penetration: f32,
    axis_idx: i32,
    out: &mut ContactManifold,
) {
    let edge_idx = (axis_idx - 6) as usize;
    let i = edge_idx / 3;
    let j = edge_idx % 3;

    // Support point on edge of A
    let mut support_a = a.pose.position;
    for k in 0..3 {
        if k == i {
            continue;
        }
        let sign = if axes_a[k].dot(normal) > 0.0 {
            1.0
        } else {
            -1.0
        };
        support_a = support_a + axes_a[k] * (sign * he_a[k]);
    }

    // Support point on edge of B
    let mut support_b = b.pose.position;
    for k in 0..3 {
        if k == j {
            continue;
        }
        let sign = if axes_b[k].dot(-normal) > 0.0 {
            1.0
        } else {
            -1.0
        };
        support_b = support_b + axes_b[k] * (sign * he_b[k]);
    }

    // Closest points on two line segments
    let (pa, pb) =
        closest_points_on_segments(support_a, axes_a[i], he_a[i], support_b, axes_b[j], he_b[j]);

    out.push_point(ContactPoint {
        world_point: midpoint(pa, pb),
        world_point_a: pa,
        world_point_b: pb,
        local_point_a: world_to_local(Some(a), pa),
        local_point_b: world_to_local(Some(b), pb),
        normal,
        penetration,
        normal_impulse: 0.0,
        tangent_impulse: [0.0, 0.0],
    });
}

fn closest_points_on_segments(
    center_a: Vec3f,
    dir_a: Vec3f,
    half_len_a: f32,
    center_b: Vec3f,
    dir_b: Vec3f,
    half_len_b: f32,
) -> (Vec3f, Vec3f) {
    let r = center_a - center_b;
    let a = dir_a.dot(dir_a);
    let e = dir_b.dot(dir_b);
    let f = dir_b.dot(r);
    let cc = dir_a.dot(r);
    let b = dir_a.dot(dir_b);
    let denom = a * e - b * b;

    let (mut t, mut s) = if denom.abs() > 1e-6 {
        (
            (b * f - cc * e) / denom,
            (b * ((b * f - cc * e) / denom) + f) / e,
        )
    } else {
        (0.0, f / e)
    };

    t = t.clamp(-half_len_a, half_len_a);
    s = s.clamp(-half_len_b, half_len_b);

    (center_a + dir_a * t, center_b + dir_b * s)
}

/// Keep only the `max_keep` best-distributed contact points in the manifold.
fn reduce_contacts(manifold: &mut ContactManifold, max_keep: usize) {
    if manifold.num_points <= max_keep {
        return;
    }

    let mut selected = [usize::MAX; 4];

    // 1. Keep the deepest contact.
    let mut deepest_penetration = -f32::MAX;
    for i in 0..manifold.num_points {
        if manifold.points[i].penetration > deepest_penetration {
            deepest_penetration = manifold.points[i].penetration;
            selected[0] = i;
        }
    }

    if selected[0] == usize::MAX {
        manifold.num_points = 0;
        return;
    }

    // 2. Keep the point furthest away from the deepest contact on body A.
    let selected_a = manifold.points[selected[0]].world_point_a;
    let mut furthest_dist = -f32::MAX;
    for i in 0..manifold.num_points {
        if i == selected[0] || manifold.points[i].penetration < -PREDICTION_DISTANCE {
            continue;
        }
        let dist = (manifold.points[i].world_point_a - selected_a).length_squared();
        if dist > furthest_dist {
            furthest_dist = dist;
            selected[1] = i;
        }
    }

    let num_selected = if selected[1] == usize::MAX {
        1
    } else {
        let selected_b = manifold.points[selected[1]].world_point_a;
        if selected_a == selected_b {
            1
        } else {
            let selected_ab = selected_b - selected_a;
            let tangent = Vec3f::cross(selected_ab, manifold.points[selected[0]].normal);

            let mut min_dot = f32::MAX;
            let mut max_dot = -f32::MAX;

            for i in 0..manifold.num_points {
                if i == selected[0]
                    || i == selected[1]
                    || manifold.points[i].penetration < -PREDICTION_DISTANCE
                {
                    continue;
                }

                let dot = (manifold.points[i].world_point_a - selected_a).dot(tangent);
                if dot < min_dot {
                    min_dot = dot;
                    selected[2] = i;
                }
                if dot > max_dot {
                    max_dot = dot;
                    selected[3] = i;
                }
            }

            if selected[2] == usize::MAX {
                2
            } else if selected[2] == selected[3] {
                3
            } else {
                4
            }
        }
    };

    let mut kept = [false; MAX_CONTACTS];
    for index in selected.iter().take(num_selected) {
        kept[*index] = true;
    }

    // Compact: move kept points to front
    let mut write = 0;
    for read in 0..manifold.num_points {
        if kept[read] {
            if write != read {
                manifold.points[write] = manifold.points[read];
            }
            write += 1;
        }
    }
    manifold.num_points = write;
}
