//! Engine-default bullet marks.
//!
//! A mark on streamed/static geometry stays in world space. A mark on a
//! moving body stores its pose in that body's local frame, so the renderer can
//! reconstruct it from the current transform without turning the mark into a
//! physics entity or asking game script to update it.

use crate::{BodyKind, Entity, GameWorld};
use makepad_math::{vec3f, vec4f, Mat4f, Vec3f};

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

/// One bullet mark. `owner == 0` means `pos`/`normal` are world-space;
/// otherwise they are owner-local and follow that entity's live transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BulletDecal {
    pub owner: u64,
    pub pos: Vec3f,
    pub normal: Vec3f,
    pub kind: BulletDecalKind,
    /// Tint used by energy burns; the other procedural families keep their
    /// authored charcoal/scorch palette.
    pub color: makepad_math::Vec4f,
    pub size: f32,
    /// Monotonic insertion identity, also available to the renderer as a
    /// stable per-mark rotation seed.
    pub serial: u64,
}

impl BulletDecal {
    pub fn from_impact(target: Option<&Entity>, pos: Vec3f, normal: Vec3f) -> Self {
        Self::from_styled_impact(
            target,
            pos,
            normal,
            BulletDecalKind::Bullet,
            vec4f(0.105, 0.075, 0.045, 1.0),
        )
    }

    pub fn from_styled_impact(
        target: Option<&Entity>,
        pos: Vec3f,
        normal: Vec3f,
        kind: BulletDecalKind,
        color: makepad_math::Vec4f,
    ) -> Self {
        let normal = unit_or(normal, vec3f(0.0, 1.0, 0.0));
        let Some(owner) = target.filter(|entity| entity.kind != BodyKind::Static) else {
            return Self {
                owner: 0,
                pos,
                normal,
                kind,
                color,
                size: kind.size(),
                serial: 0,
            };
        };
        let rotation = entity_rotation(owner);
        let scale = effective_scale(owner.scale);
        let local_pos = component_div(rotate_transpose(&rotation, pos - owner.pos), scale);
        // For p_world = R*S*p_local, normals transform by R*S^-1. Therefore
        // the inverse conversion at impact time is S*R^T*n_world.
        let local_normal = unit_or(
            component_mul(rotate_transpose(&rotation, normal), scale),
            vec3f(0.0, 1.0, 0.0),
        );
        Self {
            owner: owner.id,
            pos: local_pos,
            normal: local_normal,
            kind,
            color,
            size: kind.size(),
            serial: 0,
        }
    }

    /// Current world-space centre and normal. A vanished owner makes the mark
    /// vanish exactly like an ordinary [`crate::Part`].
    pub fn world_pose(&self, world: &GameWorld) -> Option<(Vec3f, Vec3f)> {
        if self.owner == 0 {
            return Some((self.pos + self.normal * BULLET_DECAL_OFFSET, self.normal));
        }
        let owner = world.entity(self.owner)?;
        let rotation = entity_rotation(owner);
        let scale = effective_scale(owner.scale);
        let pos = owner.pos + rotate(&rotation, component_mul(self.pos, scale));
        let normal = unit_or(
            rotate(&rotation, component_div(self.normal, scale)),
            vec3f(0.0, 1.0, 0.0),
        );
        Some((pos + normal * BULLET_DECAL_OFFSET, normal))
    }
}

/// Ring storage for all bullet marks in a world.
#[derive(Clone, Debug)]
pub struct BulletDecals {
    marks: Vec<BulletDecal>,
    next: usize,
    serial: u64,
}

impl Default for BulletDecals {
    fn default() -> Self {
        Self {
            marks: Vec::with_capacity(BULLET_DECAL_CAPACITY),
            next: 0,
            serial: 0,
        }
    }
}

impl BulletDecals {
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    pub fn as_slice(&self) -> &[BulletDecal] {
        &self.marks
    }

    pub fn clear(&mut self) {
        self.marks.clear();
        self.next = 0;
        self.serial = 0;
    }

    /// Add a world impact. Dynamic entities become owner-local; static
    /// entities and streamed/terrain hits pass `None` and remain world-space.
    pub fn mark(&mut self, target: Option<&Entity>, pos: Vec3f, normal: Vec3f) {
        self.push(BulletDecal::from_impact(target, pos, normal));
    }

    /// Insert a pre-localised mark. This split lets a `GameWorld` caller end
    /// its immutable entity borrow before mutating the pool, without cloning
    /// the entity (and its tag string) on every shot.
    pub fn push(&mut self, mut mark: BulletDecal) {
        self.serial = self.serial.wrapping_add(1);
        mark.serial = self.serial;
        if self.marks.len() < BULLET_DECAL_CAPACITY {
            self.marks.push(mark);
            return;
        }
        self.marks[self.next] = mark;
        self.next = (self.next + 1) % BULLET_DECAL_CAPACITY;
    }
}

fn effective_scale(scale: Vec3f) -> Vec3f {
    vec3f(
        if scale.x.abs() > 1.0e-6 { scale.x } else { 1.0 },
        if scale.y.abs() > 1.0e-6 { scale.y } else { 1.0 },
        if scale.z.abs() > 1.0e-6 { scale.z } else { 1.0 },
    )
}

fn component_mul(a: Vec3f, b: Vec3f) -> Vec3f {
    vec3f(a.x * b.x, a.y * b.y, a.z * b.z)
}

fn component_div(a: Vec3f, b: Vec3f) -> Vec3f {
    vec3f(a.x / b.x, a.y / b.y, a.z / b.z)
}

fn unit_or(v: Vec3f, fallback: Vec3f) -> Vec3f {
    let len_sq = v.length_squared();
    if len_sq > 1.0e-12 {
        v * (1.0 / len_sq.sqrt())
    } else {
        fallback
    }
}

fn rotate(m: &Mat4f, v: Vec3f) -> Vec3f {
    let p = m.transform_vec4(vec4f(v.x, v.y, v.z, 0.0));
    vec3f(p.x, p.y, p.z)
}

fn rotate_transpose(m: &Mat4f, v: Vec3f) -> Vec3f {
    vec3f(
        m.v[0] * v.x + m.v[1] * v.y + m.v[2] * v.z,
        m.v[4] * v.x + m.v[5] * v.y + m.v[6] * v.z,
        m.v[8] * v.x + m.v[9] * v.y + m.v[10] * v.z,
    )
}

/// Mirrors the renderer's entity frame: full rigid quaternion for cars and
/// ordinary visual yaw for movers/kinematics.
fn entity_rotation(entity: &Entity) -> Mat4f {
    if entity.kind != BodyKind::Rigid {
        return Mat4f::rotation(vec3f(0.0, entity.yaw, 0.0));
    }
    let (x, y, z, w) = (
        entity.orient.x,
        entity.orient.y,
        entity.orient.z,
        entity.orient.w,
    );
    let mut m = Mat4f::identity();
    m.v[0] = 1.0 - 2.0 * (y * y + z * z);
    m.v[1] = 2.0 * (x * y + w * z);
    m.v[2] = 2.0 * (x * z - w * y);
    m.v[4] = 2.0 * (x * y - w * z);
    m.v[5] = 1.0 - 2.0 * (x * x + z * z);
    m.v[6] = 2.0 * (y * z + w * x);
    m.v[8] = 2.0 * (x * z + w * y);
    m.v[9] = 2.0 * (y * z - w * x);
    m.v[10] = 1.0 - 2.0 * (x * x + y * y);
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BodyKind, Entity, GameWorld};

    fn near(a: Vec3f, b: Vec3f) {
        assert!((a - b).length() < 1.0e-5, "{a:?} != {b:?}");
    }

    #[test]
    fn entity_decal_follows_owner_translation_and_rotation() {
        let mut world = GameWorld::new();
        world.push_entity(Entity {
            id: 1,
            kind: BodyKind::Mover,
            pos: vec3f(1.0, 2.0, 3.0),
            scale: vec3f(1.0, 1.0, 1.0),
            ..Default::default()
        });
        let mark = BulletDecal::from_impact(
            world.entity(1),
            vec3f(1.0, 2.0, 2.0),
            vec3f(0.0, 0.0, -1.0),
        );
        world.bullet_decals.push(mark);

        let owner = world.entity_mut(1).unwrap();
        owner.pos = vec3f(5.0, 4.0, 6.0);
        owner.yaw = std::f32::consts::FRAC_PI_2;
        let (pos, normal) = world.bullet_decals.as_slice()[0].world_pose(&world).unwrap();
        near(pos, vec3f(4.0 - BULLET_DECAL_OFFSET, 4.0, 6.0));
        near(normal, vec3f(-1.0, 0.0, 0.0));
    }

    #[test]
    fn fixed_pool_recycles_the_oldest_slot_without_growing() {
        let mut decals = BulletDecals::default();
        for i in 0..BULLET_DECAL_CAPACITY + 3 {
            decals.mark(None, vec3f(i as f32, 0.0, 0.0), vec3f(0.0, 1.0, 0.0));
        }
        assert_eq!(decals.len(), BULLET_DECAL_CAPACITY);
        assert_eq!(decals.as_slice()[0].pos.x, BULLET_DECAL_CAPACITY as f32);
        assert_eq!(decals.as_slice()[1].pos.x, BULLET_DECAL_CAPACITY as f32 + 1.0);
        assert_eq!(decals.as_slice()[2].pos.x, BULLET_DECAL_CAPACITY as f32 + 2.0);
        assert_eq!(decals.as_slice()[3].pos.x, 3.0);
    }

    #[test]
    fn procedural_families_share_the_ring_but_keep_size_tint_and_kind() {
        let tint = vec4f(0.15, 1.0, 0.12, 0.95);
        let mut decals = BulletDecals::default();
        decals.push(BulletDecal::from_styled_impact(
            None,
            Vec3f::default(),
            vec3f(0.0, 1.0, 0.0),
            BulletDecalKind::Scorch,
            vec4f(0.095, 0.052, 0.022, 1.0),
        ));
        decals.push(BulletDecal::from_styled_impact(
            None,
            Vec3f::default(),
            vec3f(0.0, 1.0, 0.0),
            BulletDecalKind::Energy,
            tint,
        ));
        assert_eq!(decals.len(), 2);
        assert_eq!(decals.as_slice()[0].size, SCORCH_DECAL_SIZE);
        assert_eq!(decals.as_slice()[1].size, ENERGY_DECAL_SIZE);
        assert_eq!(decals.as_slice()[1].color, tint);
        assert_eq!(decals.marks.capacity(), BULLET_DECAL_CAPACITY);
    }
}
