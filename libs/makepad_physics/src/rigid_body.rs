use makepad_math::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BodyType {
    /// Fully simulated, responds to forces and collisions.
    Dynamic,
    /// Never moves, infinite mass. Used for ground planes, walls, etc.
    Fixed,
}

#[derive(Clone, Debug)]
pub struct RigidBody {
    /// Position and orientation.
    pub pose: Pose,
    /// Linear velocity (world space, m/s).
    pub linear_velocity: Vec3f,
    /// Angular velocity (world space, rad/s).
    pub angular_velocity: Vec3f,
    /// Inverse mass (0.0 for fixed bodies).
    pub inv_mass: f32,
    /// Local-space inverse inertia tensor (diagonal for cuboids).
    pub local_inv_inertia: Mat3f,
    /// Cuboid half-extents.
    pub half_extents: Vec3f,
    /// Coefficient of friction.
    pub friction: f32,
    /// Coefficient of restitution (bounciness).
    pub restitution: f32,
    /// Body type.
    pub body_type: BodyType,
}

impl RigidBody {
    /// Create a dynamic cuboid rigid body.
    /// `half_extents`: half-size along each axis.
    /// `density`: mass per unit volume.
    pub fn new_dynamic(position: Vec3f, half_extents: Vec3f, density: f32) -> Self {
        let hx = half_extents.x;
        let hy = half_extents.y;
        let hz = half_extents.z;
        let volume = 8.0 * hx * hy * hz;
        let mass = density * volume;
        let inv_mass = if mass > 0.0 { 1.0 / mass } else { 0.0 };

        // Cuboid inertia tensor (standard formula):
        // I_xx = (mass/3) * (hy^2 + hz^2), etc.
        // We use full extents squared: (2*h)^2 = 4*h^2, and mass/12 * (ey^2 + ez^2)
        // which equals mass/3 * (hy^2 + hz^2)
        let k = mass / 3.0;
        let i_xx = k * (hy * hy + hz * hz);
        let i_yy = k * (hx * hx + hz * hz);
        let i_zz = k * (hx * hx + hy * hy);

        let inv_inertia = Vec3f {
            x: if i_xx > 0.0 { 1.0 / i_xx } else { 0.0 },
            y: if i_yy > 0.0 { 1.0 / i_yy } else { 0.0 },
            z: if i_zz > 0.0 { 1.0 / i_zz } else { 0.0 },
        };

        RigidBody {
            pose: Pose {
                position,
                orientation: Quat::default(),
            },
            linear_velocity: Vec3f::default(),
            angular_velocity: Vec3f::default(),
            inv_mass,
            local_inv_inertia: Mat3f::from_diagonal(inv_inertia),
            half_extents,
            friction: 0.5,
            restitution: 0.0,
            body_type: BodyType::Dynamic,
        }
    }

    /// Create a fixed (immovable) cuboid body.
    pub fn new_fixed(position: Vec3f, half_extents: Vec3f) -> Self {
        RigidBody {
            pose: Pose {
                position,
                orientation: Quat::default(),
            },
            linear_velocity: Vec3f::default(),
            angular_velocity: Vec3f::default(),
            inv_mass: 0.0,
            local_inv_inertia: Mat3f::zero(),
            half_extents,
            friction: 0.5,
            restitution: 0.0,
            body_type: BodyType::Fixed,
        }
    }

    /// Compute world-space inverse inertia tensor: R * I_local_inv * R^T
    pub fn world_inv_inertia(&self) -> Mat3f {
        let r = Mat3f::from_quat(self.pose.orientation);
        let rt = r.transpose();
        r.mul_mat3(&self.local_inv_inertia).mul_mat3(&rt)
    }

    /// Is this body dynamic (can move)?
    pub fn is_dynamic(&self) -> bool {
        self.body_type == BodyType::Dynamic
    }
}
