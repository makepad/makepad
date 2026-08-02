//! Entity/decoration/HUD data types — moved verbatim from gamemaker's
//! game_view.rs (M0 stage A extraction; see game.md). Semantics and float
//! expression order are preserved exactly for tape parity.

use crate::CallbackSlot;
use makepad_math::*;

#[derive(Clone, Copy, PartialEq)]
pub enum BodyKind {
    /// Doesn't move on its own; the world for everything else to stand on.
    Static,
    /// Script-driven velocity, no gravity, no collision response on itself.
    /// Moving platforms; things standing on it are carried.
    Kinematic,
    /// Gravity + collides with static/kinematic. Players and NPCs.
    Mover,
}

/// Visual shape of an entity or part. Physics stays the entity's AABB — the
/// same approximation the Godot corpus made (collision boxes under any model).
/// Each shape is a shared unit geometry; rendering batches per shape, so a
/// mixed scene still costs one draw call per shape per pass.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
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

#[derive(Clone)]
pub struct Entity {
    pub id: u64,
    pub kind: BodyKind,
    pub pos: Vec3f,
    pub vel: Vec3f,
    pub half: Vec3f,
    pub color: Vec4f,
    pub tag: String,
    pub sensor: bool,
    /// `collide: false` = opaque decoration: renders like a solid, but no
    /// physics, no touch reports (rotated road slabs, arches, scenery).
    /// Distinct from `sensor`, which is translucent AND reports touches.
    pub collide: bool,
    pub gravity_scale: f32,
    pub on_floor: bool,
    /// Entity id this mover rests on (for kinematic carry), 0 = none.
    pub floor_id: u64,
    /// Riding another entity (vehicle seats): physics skips this mover and
    /// pins it to the owner at the given offset. 0 = free.
    pub attached_to: u64,
    pub attach_offset: Vec3f,
    /// Ride mode ("ride" vs the default "seat"): a latched rider (headcrab).
    /// Seat riders face where their owner faces; ride riders spin their model
    /// at `attach_spin` rad/s (the scrabbling).
    pub attach_ride: bool,
    pub attach_spin: f32,
    /// Engine-side scale on game.walk velocities (headcrab debuff): the
    /// player script never needs to know something slowed it down.
    pub speed_mult: f32,
    /// Seconds until auto-removal; 0 = forever. Projectiles.
    pub life: f32,
    /// Report contacts with every other solid entity through on_touch
    /// (movers pass through each other spatially, but a `hits` entity still
    /// sees the overlap; wall stops from the sweep are reported too).
    pub hits: bool,
    /// Transient: solid id a `hits` entity swept into this tick.
    pub hit_wall: u64,
    /// Visual model yaw (radians). Physics stays an unrotated AABB — Godot's
    /// CharacterBody does exactly the same: only the Model child rotates.
    pub yaw: f32,
    /// Movers turn to face their walk direction unless the script took over
    /// with game.face().
    pub auto_face: bool,
    /// Radians/second toward the facing target (Godot actors used 5.5–10).
    pub turn_rate: f32,
    /// Visual model scale (physics half untouched); lerped toward the target
    /// like Godot's `_model.scale.lerp(target, delta*6)` curls.
    pub scale: Vec3f,
    pub scale_target: Vec3f,
    /// Emission energy: 0 = matte, ~3 = glowing eyes, ramps at runtime.
    pub glow: f32,
    /// Visual-only shape; collision stays the AABB.
    pub shape: Shape,
}

/// A purely visual box welded to an entity — eyes, arms, hats. No collision,
/// no physics. Offsets/rotation are OWNER-LOCAL (front at -z) and rotate/scale
/// with the owner's model; gone when the owner goes. Each field pairs with a
/// target the engine lerps toward (game.move_part), which is how arms reach.
#[derive(Clone)]
pub struct Part {
    pub id: u64,
    pub owner: u64,
    pub offset: Vec3f,
    pub rot: Vec3f,
    pub half: Vec3f,
    pub target_offset: Vec3f,
    pub target_rot: Vec3f,
    pub target_half: Vec3f,
    /// Lerp rate/second (Godot's arm reach used ~9).
    pub rate: f32,
    pub color: Vec4f,
    pub glow: f32,
    pub shape: Shape,
    /// True while easing toward targets (game.move_part re-arms it). Settled
    /// parts skip the easing math AND stay eligible for the static slab.
    pub anim_active: bool,
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

/// Where a HUD slot pins to the pane. Slots sharing an anchor stack downward
/// in insertion order.
#[derive(Clone, Copy, PartialEq, Default)]
pub enum HudAnchor {
    TopLeft,
    Top,
    TopRight,
    #[default]
    Center,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl HudAnchor {
    pub fn parse(name: &str) -> HudAnchor {
        match name {
            "top_left" => HudAnchor::TopLeft,
            "top" => HudAnchor::Top,
            "top_right" => HudAnchor::TopRight,
            "bottom_left" => HudAnchor::BottomLeft,
            "bottom" => HudAnchor::Bottom,
            "bottom_right" => HudAnchor::BottomRight,
            _ => HudAnchor::Center,
        }
    }
}

/// One line of screen text. `size`/`color.w` of 0 mean "use the slot default".
#[derive(Clone, Default)]
pub struct HudSlot {
    pub text: String,
    pub color: Vec4f,
    pub size: f32,
    pub anchor: HudAnchor,
}

/// A HUD gauge (speedometer, boost). Fraction 0..1 fills left to right.
#[derive(Clone)]
pub struct HudBar {
    pub name: String,
    pub fraction: f32,
    pub color: Vec4f,
    pub anchor: HudAnchor,
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

/// Sky + atmosphere, set from script with game.sky({...}). Off by default so
/// existing indoor/abstract games keep their dark backdrop.
#[derive(Clone, Copy)]
pub struct SkyConfig {
    pub top: Vec4f,
    pub horizon: Vec4f,
    pub ground: Vec4f,
    pub ground_bottom: Vec4f,
    /// Exponential distance-fog density toward the horizon color.
    pub fog: f32,
}

impl Default for SkyConfig {
    fn default() -> Self {
        // The Godot game's ProceduralSkyMaterial numbers.
        Self {
            top: vec4(0.32, 0.58, 0.9, 1.0),
            horizon: vec4(0.75, 0.87, 0.96, 1.0),
            ground: vec4(0.68, 0.75, 0.66, 1.0),
            ground_bottom: vec4(0.3, 0.4, 0.3, 1.0),
            fog: 0.004,
        }
    }
}

/// Script-registered timer. The callback is an opaque host slot — the sim
/// never touches the script VM (game.md: no ScriptObjectRef in sim state).
#[derive(Clone)]
pub struct GameTimer {
    pub id: u64,
    pub at_tick: u64,
    /// 0 = one-shot (game.after); N = re-arm every N ticks (game.every).
    pub interval_ticks: u64,
    pub func: CallbackSlot,
}

/// One persisted save value (game.save/game.load). Numbers and strings only —
/// enough for best laps, high scores, unlocked things.
#[derive(Clone)]
pub enum SaveVal {
    Num(f64),
    Str(String),
}

#[derive(Default, Clone, Copy)]
pub struct PadState {
    pub axis_x: f64,
    pub axis_z: f64,
    pub jump: bool,
    pub jump_pressed: bool,
    pub shoot: bool,
    pub shoot_pressed: bool,
    pub grab: bool,
    pub grab_pressed: bool,
    /// Gamepad Y — the "reset my car" action (keyboard R).
    pub reset: bool,
    pub reset_pressed: bool,
}
