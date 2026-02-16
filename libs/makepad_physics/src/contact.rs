use makepad_math::*;

/// Maximum contact points per manifold (face-face can produce up to 4,
/// ground corners can produce up to 8).
pub const MAX_CONTACTS: usize = 8;

#[derive(Clone, Copy, Debug, Default)]
pub struct ContactPoint {
    /// World-space contact position.
    pub world_point: Vec3f,
    /// Offset from body A center to contact point (world space).
    pub offset_a: Vec3f,
    /// Offset from body B center to contact point (world space).
    pub offset_b: Vec3f,
    /// Contact normal pointing from A toward B (world space, unit length).
    pub normal: Vec3f,
    /// Penetration depth (positive = overlapping).
    pub penetration: f32,
    /// Accumulated normal impulse (for warmstarting, future use).
    pub normal_impulse: f32,
    /// Accumulated tangent impulses [tangent1, tangent2] (for warmstarting, future use).
    pub tangent_impulse: [f32; 2],
}

#[derive(Clone, Debug)]
pub struct ContactManifold {
    /// Index of body A in the world's body list.
    pub body_a: usize,
    /// Index of body B in the world's body list (usize::MAX = ground).
    pub body_b: usize,
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
