use crate::rigid_body::RigidBody;
use makepad_math::*;

/// Maximum contact points per manifold (face-face can produce up to 4,
/// ground corners can produce up to 8).
pub const MAX_CONTACTS: usize = 8;

#[derive(Clone, Copy, Debug, Default)]
pub struct ContactPoint {
    /// World-space midpoint used by the solver.
    pub world_point: Vec3f,
    /// World-space contact point on body A.
    pub world_point_a: Vec3f,
    /// World-space contact point on body B.
    pub world_point_b: Vec3f,
    /// Body-A-local contact point. For ground, this stays in world space.
    pub local_point_a: Vec3f,
    /// Body-B-local contact point. For ground, this stays in world space.
    pub local_point_b: Vec3f,
    /// Geometric feature id on body A used for persistent matching.
    pub feature_id_a: u32,
    /// Geometric feature id on body B used for persistent matching.
    pub feature_id_b: u32,
    /// Contact normal pointing from A toward B (world space, unit length).
    pub normal: Vec3f,
    /// Penetration depth (positive = overlapping).
    pub penetration: f32,
    /// Total normal impulse applied during the previous frame.
    pub normal_impulse: f32,
    /// Total tangent impulses [tangent1, tangent2] applied during the previous frame.
    pub tangent_impulse: [f32; 2],
    /// Normal impulse retained for cross-frame warmstarting.
    pub warmstart_normal_impulse: f32,
    /// Tangent impulse retained for cross-frame warmstarting.
    pub warmstart_tangent_impulse: [f32; 2],
    /// Twist impulse retained for cross-frame warmstarting.
    pub warmstart_twist_impulse: f32,
}

#[derive(Clone, Debug)]
pub struct ContactManifold {
    /// Index of body A in the world's body list.
    pub body_a: usize,
    /// Index of body B in the world's body list (usize::MAX = ground).
    pub body_b: usize,
    /// Contact normal expressed in body A's local frame.
    pub local_normal_a: Vec3f,
    /// Contact normal expressed in body B's local frame.
    pub local_normal_b: Vec3f,
    /// Number of active contact points.
    pub num_points: usize,
    /// Contact points (fixed-size array, only first num_points are valid).
    pub points: [ContactPoint; MAX_CONTACTS],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CapsuleBoxContact {
    /// Contact normal pointing outward from the box toward the capsule.
    pub normal: Vec3f,
    /// Point on the box surface closest to (or penetrating into) the capsule.
    pub point_box: Vec3f,
    /// Point on the capsule surface closest to (or penetrating into) the box.
    pub point_capsule: Vec3f,
    /// Midpoint between the two contact points.
    pub point: Vec3f,
    /// Positive penetration depth.
    pub penetration: f32,
}

impl Default for ContactManifold {
    fn default() -> Self {
        ContactManifold {
            body_a: 0,
            body_b: 0,
            local_normal_a: Vec3f::default(),
            local_normal_b: Vec3f::default(),
            num_points: 0,
            points: [ContactPoint::default(); MAX_CONTACTS],
        }
    }
}

impl ContactManifold {
    pub fn active_points(&self) -> &[ContactPoint] {
        &self.points[..self.num_points]
    }

    pub fn push_point(&mut self, p: ContactPoint) {
        if self.num_points < MAX_CONTACTS {
            self.points[self.num_points] = p;
            self.num_points += 1;
        }
    }
}

fn clamp_point_to_box(point: Vec3f, half_extents: Vec3f) -> Vec3f {
    vec3f(
        point.x.clamp(-half_extents.x, half_extents.x),
        point.y.clamp(-half_extents.y, half_extents.y),
        point.z.clamp(-half_extents.z, half_extents.z),
    )
}

fn evaluate_segment_box_candidate(
    a: Vec3f,
    d: Vec3f,
    half_extents: Vec3f,
    t: f32,
) -> (f32, Vec3f, Vec3f) {
    let seg_point = a + d * t;
    let box_point = clamp_point_to_box(seg_point, half_extents);
    let delta = seg_point - box_point;
    (delta.dot(delta), seg_point, box_point)
}

fn closest_segment_point_to_box(
    a: Vec3f,
    b: Vec3f,
    half_extents: Vec3f,
) -> (Vec3f, Vec3f, f32) {
    let d = b - a;
    let mut breaks = vec![0.0f32, 1.0f32];
    for (a_axis, d_axis, h_axis) in [
        (a.x, d.x, half_extents.x),
        (a.y, d.y, half_extents.y),
        (a.z, d.z, half_extents.z),
    ] {
        if d_axis.abs() <= 1.0e-6 {
            continue;
        }
        for bound in [-h_axis, h_axis] {
            let t = (bound - a_axis) / d_axis;
            if t > 1.0e-6 && t < 1.0 - 1.0e-6 {
                breaks.push(t);
            }
        }
    }
    breaks.sort_by(|a, b| a.partial_cmp(b).unwrap());
    breaks.dedup_by(|a, b| (*a - *b).abs() < 1.0e-5);

    let mut best_t = 0.0;
    let mut best_seg = a;
    let mut best_box = clamp_point_to_box(a, half_extents);
    let mut best_dist_sq = (best_seg - best_box).dot(best_seg - best_box);

    let mut try_candidate = |t: f32| {
        let t = t.clamp(0.0, 1.0);
        let (dist_sq, seg_point, box_point) =
            evaluate_segment_box_candidate(a, d, half_extents, t);
        if dist_sq < best_dist_sq {
            best_t = t;
            best_seg = seg_point;
            best_box = box_point;
            best_dist_sq = dist_sq;
        }
    };

    for window in breaks.windows(2) {
        let t0 = window[0];
        let t1 = window[1];
        if t1 - t0 <= 1.0e-6 {
            continue;
        }
        try_candidate(t0);
        try_candidate(t1);

        let mid = 0.5 * (t0 + t1);
        let mut quad_a = 0.0;
        let mut quad_b = 0.0;
        let mut active_axes = 0usize;
        for (a_axis, d_axis, h_axis) in [
            (a.x, d.x, half_extents.x),
            (a.y, d.y, half_extents.y),
            (a.z, d.z, half_extents.z),
        ] {
            let p_mid = a_axis + d_axis * mid;
            let c = if p_mid < -h_axis {
                a_axis + h_axis
            } else if p_mid > h_axis {
                a_axis - h_axis
            } else {
                continue;
            };
            quad_a += d_axis * d_axis;
            quad_b += 2.0 * c * d_axis;
            active_axes += 1;
        }
        if active_axes == 0 {
            try_candidate(mid);
        } else if quad_a > 1.0e-8 {
            try_candidate((-quad_b / (2.0 * quad_a)).clamp(t0, t1));
        }
    }

    let _ = best_t;
    (best_seg, best_box, best_dist_sq)
}

pub fn capsule_box_contact(
    segment_a: Vec3f,
    segment_b: Vec3f,
    radius: f32,
    body: &RigidBody,
) -> Option<CapsuleBoxContact> {
    let inv_orientation = body.pose.orientation.invert();
    let a_local = inv_orientation.rotate_vec3(&(segment_a - body.pose.position));
    let b_local = inv_orientation.rotate_vec3(&(segment_b - body.pose.position));
    let (seg_local, box_local, dist_sq) =
        closest_segment_point_to_box(a_local, b_local, body.half_extents);

    if dist_sq > radius * radius {
        return None;
    }

    let delta = seg_local - box_local;
    let (normal_local, point_box_local, point_capsule_local, penetration) = if dist_sq > 1.0e-8 {
        let dist = dist_sq.sqrt();
        let normal = delta * (1.0 / dist);
        (
            normal,
            box_local,
            seg_local - normal * radius,
            radius - dist,
        )
    } else {
        let dx = body.half_extents.x - seg_local.x.abs();
        let dy = body.half_extents.y - seg_local.y.abs();
        let dz = body.half_extents.z - seg_local.z.abs();
        if dx <= dy && dx <= dz {
            let normal = vec3f(if seg_local.x >= 0.0 { 1.0 } else { -1.0 }, 0.0, 0.0);
            (
                normal,
                vec3f(
                    if seg_local.x >= 0.0 {
                        body.half_extents.x
                    } else {
                        -body.half_extents.x
                    },
                    seg_local.y.clamp(-body.half_extents.y, body.half_extents.y),
                    seg_local.z.clamp(-body.half_extents.z, body.half_extents.z),
                ),
                seg_local + normal * radius,
                radius + dx,
            )
        } else if dy <= dz {
            let normal = vec3f(0.0, if seg_local.y >= 0.0 { 1.0 } else { -1.0 }, 0.0);
            (
                normal,
                vec3f(
                    seg_local.x.clamp(-body.half_extents.x, body.half_extents.x),
                    if seg_local.y >= 0.0 {
                        body.half_extents.y
                    } else {
                        -body.half_extents.y
                    },
                    seg_local.z.clamp(-body.half_extents.z, body.half_extents.z),
                ),
                seg_local + normal * radius,
                radius + dy,
            )
        } else {
            let normal = vec3f(0.0, 0.0, if seg_local.z >= 0.0 { 1.0 } else { -1.0 });
            (
                normal,
                vec3f(
                    seg_local.x.clamp(-body.half_extents.x, body.half_extents.x),
                    seg_local.y.clamp(-body.half_extents.y, body.half_extents.y),
                    if seg_local.z >= 0.0 {
                        body.half_extents.z
                    } else {
                        -body.half_extents.z
                    },
                ),
                seg_local + normal * radius,
                radius + dz,
            )
        }
    };

    let normal = body.pose.orientation.rotate_vec3(&normal_local).normalize();
    let point_box = body.pose.orientation.rotate_vec3(&point_box_local) + body.pose.position;
    let point_capsule =
        body.pose.orientation.rotate_vec3(&point_capsule_local) + body.pose.position;
    Some(CapsuleBoxContact {
        normal,
        point_box,
        point_capsule,
        point: (point_box + point_capsule) * 0.5,
        penetration,
    })
}
