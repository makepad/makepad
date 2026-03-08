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
