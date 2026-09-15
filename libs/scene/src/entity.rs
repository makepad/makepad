//! Displayed entity poses and appearance, without simulation state.
use makepad_math::*;
use crate::light::EntityLight;

#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum BodyKind {
    /// Doesn't move on its own; the world for everything else to stand on.
    #[default]
    Static,
    /// Script-driven velocity, no gravity, no collision response on itself.
    /// Moving platforms; things standing on it are carried.
    Kinematic,
    /// Gravity + collides with static/kinematic. Players and NPCs.
    Mover,
    /// Full box3d rigid-body dynamics (M1a): stacks, tumbles, takes impulses.
    /// Collides with statics/kinematics/other rigids — NOT with movers (v1
    /// contract; movers keep the kinematic sweep). Shared replication tier.
    Rigid,
}

/// Visual shape of an entity or part. Physics stays the entity's AABB — the
/// same approximation the Godot corpus made (collision boxes under any model).
/// Each shape is a shared unit geometry; rendering batches per shape, so a
/// mixed scene still costs one draw call per shape per pass.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Shape {
    #[default]
    Box = 0,
    Sphere = 1,
    Cylinder = 2,
    Cone = 3,
    Wedge = 4,
}

impl Shape {
    pub const ALL: [Shape; 5] = [
        Shape::Box,
        Shape::Sphere,
        Shape::Cylinder,
        Shape::Cone,
        Shape::Wedge,
    ];

    pub fn index(self) -> usize {
        match self {
            Shape::Box => 0,
            Shape::Sphere => 1,
            Shape::Cylinder => 2,
            Shape::Cone => 3,
            Shape::Wedge => 4,
        }
    }

    pub fn parse(name: &str) -> Shape {
        match name {
            "sphere" | "ball" => Shape::Sphere,
            "cylinder" => Shape::Cylinder,
            "cone" => Shape::Cone,
            "wedge" | "ramp" => Shape::Wedge,
            _ => Shape::Box,
        }
    }
}

/// Per-instance albedo adjustment shared by meshes, sprites and primitives.
/// `hue` is authored in degrees; saturation/value are neutral at 1. The
/// renderer converts this small CPU-facing value to its vec4 instance lane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorAdjust {
    pub hue: f32,
    pub saturation: f32,
    pub value: f32,
}

impl Default for ColorAdjust {
    fn default() -> Self {
        Self { hue: 0.0, saturation: 1.0, value: 1.0 }
    }
}

impl ColorAdjust {
    pub fn instance(self) -> Vec4f {
        vec4f(self.hue, self.saturation, self.value, 0.0)
    }
}

/// A displayed body. IDs are supplied by the scene's owner.
#[derive(Clone)]
pub struct Entity {
    pub id: u64,
    pub kind: BodyKind,
    pub pos: Vec3f,
    pub half: Vec3f,
    pub scale: Vec3f,
    pub yaw: f32,
    pub orient: Quat,
    pub forward_axis: Option<Vec3f>,
    pub color: Vec4f,
    pub color_adjust: ColorAdjust,
    pub glow: f32,
    pub shape: Shape,
    pub hidden: bool,
    pub alpha_primitive: bool,
    pub suppress_primitive: bool,
    pub bake_skip: bool,
    pub parent: u64,
    pub lights: Vec<EntityLight>,
}
impl Default for Entity {
    fn default() -> Self {
        Self {
            id: 0, kind: BodyKind::default(), pos: Vec3f::default(), half: Vec3f::default(),
            scale: vec3f(1.0, 1.0, 1.0), yaw: 0.0, orient: Quat::default(), forward_axis: None,
            color: Vec4f::default(), color_adjust: ColorAdjust::default(), glow: 0.0,
            shape: Shape::default(), hidden: false, alpha_primitive: false,
            suppress_primitive: false, bake_skip: false, parent: 0, lights: Vec::new(),
        }
    }
}

impl Entity {
    /// Current horizontal facing, regardless of which motion tier owns it.
    ///
    /// Movers and statics author `yaw` directly. A rigid body, however,
    /// receives its full quaternion from box3d every tick and its historical
    /// `yaw` field remains the spawn heading. Cameras, AI and attachments that
    /// follow a turning car must therefore derive heading from `orient` rather
    /// than silently tracking the stale spawn value.
    pub fn visual_heading(&self) -> f32 {
        if self.orient == Quat::default() {
            return self.yaw;
        }
        let q = self.orient;
        let v = vec3f(0.0, 0.0, -1.0);
        let u = vec3f(q.x, q.y, q.z);
        let forward = u * (2.0 * u.dot(v))
            + v * (q.w * q.w - u.dot(u))
            + Vec3f::cross(u, v) * (2.0 * q.w);
        crate::forward_to_heading(forward)
    }
}

#[cfg(test)]
mod facing_tests {
    use super::*;

    #[test]
    fn rigid_visual_heading_comes_from_orientation_not_stale_spawn_yaw() {
        let yaw = 0.73f32;
        let (s, c) = makepad_math::deterministic::sincos(yaw * 0.5);
        let entity = Entity {
            yaw: -1.2,
            orient: Quat {
                x: 0.0,
                y: s,
                z: 0.0,
                w: c,
            },
            ..Default::default()
        };
        assert!((crate::heading_delta(entity.visual_heading(), yaw)).abs() < 1.0e-5);

        let authored = Entity {
            yaw,
            ..Default::default()
        };
        assert_eq!(authored.visual_heading(), yaw);
    }
}

/// A displayed owner-local part, with procedural rotation already resolved.
#[derive(Clone, Default)]
pub struct Part {
    pub id: u64,
    pub owner: u64,
    pub offset: Vec3f,
    pub rot: Vec3f,
    pub half: Vec3f,
    pub color: Vec4f,
    pub glow: f32,
    pub shape: Shape,
    pub animated: bool,
}

/// Immediate-mode stretched box between two points (grapple cables, lasers,
/// tow ropes). Scripts re-issue it every tick from on_tick; anything not
/// re-issued is gone next tick — no lifecycle to leak.
#[derive(Clone, Copy)]
pub struct Beam {
    pub from: Vec3f,
    pub to: Vec3f,
    /// Full thickness of the cable box.
    pub size: f32,
    pub color: Vec4f,
    pub glow: f32,
}

/// A billboard nametag. Each entity has at most one DEFAULT label (the plain
/// `game.label(id, text)` form) plus any number of extra ones ("HELP!").
#[derive(Clone)]
pub struct LabelDef {
    pub lid: u64,
    pub owner: u64,
    pub text: String,
    /// Height above the entity center; NAN = auto (half.y + 0.7).
    pub height: f32,
    /// w = 0 → style default color.
    pub color: Vec4f,
    /// 0 → style default size.
    pub size: f32,
    pub default: bool,
}

