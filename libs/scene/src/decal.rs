//! Resolved surface marks ready to draw, including their surface offset.
use makepad_math::{Vec3f, Vec4f};

/// The complete resident bullet-mark budget. Once full, one shot overwrites
/// the oldest slot; the backing allocation never grows again.
pub const BULLET_DECAL_CAPACITY: usize = 64;
/// Full diameter in world metres (8 cm).
pub const BULLET_DECAL_SIZE: f32 = 0.08;
/// Full diameter of an explosion scorch (50 cm).
pub const SCORCH_DECAL_SIZE: f32 = 0.5;
/// Full diameter of a projectile-energy burn (16 cm).
pub const ENERGY_DECAL_SIZE: f32 = 0.16;
/// Lift from the hit plane in world metres, just enough to win the depth tie.
pub const BULLET_DECAL_OFFSET: f32 = 0.002;

/// Procedural surface-mark family. Pellets use `Bullet` once per ray: the
/// spray is real impact geometry rather than a shotgun-shaped texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BulletDecalKind {
    #[default]
    Bullet,
    Scorch,
    Energy,
}

impl BulletDecalKind {
    pub fn size(self) -> f32 {
        match self {
            Self::Bullet => BULLET_DECAL_SIZE,
            Self::Scorch => SCORCH_DECAL_SIZE,
            Self::Energy => ENERGY_DECAL_SIZE,
        }
    }

    /// Numeric style lane consumed by the single procedural decal shader.
    pub fn shader_id(self) -> f32 {
        match self {
            Self::Bullet => 0.0,
            Self::Scorch => 1.0,
            Self::Energy => 2.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decal {
    pub pos: Vec3f,
    pub normal: Vec3f,
    pub kind: BulletDecalKind,
    pub color: Vec4f,
    pub size: f32,
    pub serial: u64,
}
