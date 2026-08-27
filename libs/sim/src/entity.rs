//! Entity/decoration/HUD data types — moved verbatim from gamemaker's
//! game_view.rs (M0 stage A extraction; see game.md). Semantics and float
//! expression order are preserved exactly for tape parity.

use crate::CallbackSlot;
use makepad_math::*;

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

/// Optional presentation-matched exterior in the entity's local frame.
///
/// Physics is intentionally free to keep a cheaper body (for example a low
/// vehicle chassis). Hosts that bind a taller or wider model can publish its
/// measured bounds here so weapon rays hit everything the player sees. For a
/// rigid, capsule walkers also treat this box as a query-only obstacle: the
/// visible roof and bodywork block a character without adding any solver mass,
/// contact response, or inertia to the deliberately smaller rigid chassis.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct HurtBox {
    pub center: Vec3f,
    pub half: Vec3f,
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

#[derive(Clone, Default)]
pub struct Entity {
    pub id: u64,
    pub kind: BodyKind,
    pub pos: Vec3f,
    pub vel: Vec3f,
    pub half: Vec3f,
    pub hurt_box: Option<HurtBox>,
    pub color: Vec4f,
    pub color_adjust: ColorAdjust,
    pub tag: String,
    pub sensor: bool,
    /// `collide: false` = opaque decoration: renders like a solid, but no
    /// physics, no touch reports (rotated road slabs, arches, scenery).
    /// Distinct from `sensor`, which is translucent AND reports touches.
    pub collide: bool,
    /// `hidden: true` = the exact inverse of `collide: false` — solid to
    /// everything, drawn by nothing. It exists for props whose *appearance*
    /// comes from a mesh: a stock house is drawn as a model, so its collider
    /// must not also draw a box around it. Skipped by the geometry batches
    /// and by shadow casting, since an invisible box casting a shadow is
    /// exactly the artifact this flag prevents.
    ///
    /// Stated as `hidden` rather than `visible` on purpose: `Entity` derives
    /// `Default`, so the field that defaults to `false` must be the unusual
    /// case. A `visible` flag would make every default-constructed entity
    /// invisible — the same class of trap as the rng that defaulted to a
    /// zero seed and the bodies that defaulted to zero gravity.
    pub hidden: bool,
    /// Invisible containment: physics treats this exactly like an ordinary
    /// solid, camera placement rays pass through it, and presentation draws
    /// NOTHING — an interior is an open stage bounded by walls you feel but
    /// never see (user, 2026-08-27: "make these indoor spaces without
    /// walls"). Distinct from `hidden`, which is a presentation lie trap:
    /// shell is an authored, replicated design choice for room envelopes.
    /// Default false keeps every existing body an ordinary drawn solid.
    pub shell: bool,
    /// Retained visual identity with no gameplay presence. The entity still
    /// replicates and renders (for example, a corpse finishing its skinned
    /// death pose), but queries, collision separation and touch collection
    /// must ignore it. Distinct from `hidden`: presentation is the reason it
    /// remains alive. Default false keeps ordinary entities interactive.
    pub non_interactive: bool,
    /// AIM-LOCKED locomotion (a first-person walker): the body's yaw is the
    /// player's AIM, written by their rig every tick, and facing must never
    /// be derived from travel — walking backwards means backpedaling, not
    /// turning around. Skins read this to face the aim and to reverse the
    /// walk cycle on negative longitudinal speed; replication reads it to
    /// treat yaw as authored state rather than Derived tier.
    pub aim_locked: bool,
    /// Excluded from the light bake's occluder set. Mesh-voxel physics
    /// colliders set this: dozens of stepped boxes per model smear baked
    /// shadows, and the clean occluder boxes come from the mask/kit path.
    /// Physics and light want different simplifications of the same mesh.
    pub bake_skip: bool,
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
    /// Visual-only shape; collision stays the AABB. Exception: a Rigid with
    /// shape:"sphere" gets a box3d sphere collider (radius = half.x) so it
    /// rolls — the one place visual and collision shape agree.
    pub shape: Shape,
    /// Full orientation, read back from box3d each tick — Rigid only
    /// (identity for everything else; movers/statics rotate via `yaw`).
    pub orient: Quat,
    /// Rigid material params, applied when the box3d body is created.
    pub density: f32,
    pub friction: f32,
    pub restitution: f32,
    /// How hard this mover resists being shoved by another mover. Heavier
    /// moves less: two equals each give half, a player at 4.0 against an NPC
    /// at 1.0 gives up a fifth of the overlap and shoulders through.
    ///
    /// `0.0` — the `Default` — READS AS 1.0, not as weightless. A zero here
    /// would make a default-constructed mover infinitely shovable and, worse,
    /// divide by zero when two of them met. Same discipline as `hidden` over
    /// `visible`: the value a `Default` lands on must be the sane one.
    pub push_mass: f32,
    /// Walkable top surface for a STATIC prop collider (a house's roof).
    /// Movers resolve their floor against this grid instead of the entity's
    /// box, which drops out of their sweeps but stays for rigid dynamics and
    /// raycasts — see [`crate::surface`]. Not replicated: only the simulating
    /// host steps movers against props. `Arc`'d so world snapshots stay cheap.
    pub surface: Option<std::sync::Arc<crate::surface::SurfaceGrid>>,
    /// Ticks left flat on the ground after a rigid body hit this mover (D4:
    /// car-hits-walker). Set by the contact pipeline
    /// ([`crate::dynamics::apply_mover_contacts`]), decremented once per tick
    /// by step_world, read by controllers/brains to suppress walking and play
    /// the ragdoll-ish pose. Shared replication tier — it rides in the
    /// entity-state flags' upper byte, which is why its cap
    /// ([`crate::dynamics::KNOCKDOWN_MAX_TICKS`]) stays below 256.
    pub knocked_down: u16,
    /// Queryable surface type index (F10): becomes the box3d shape's
    /// `user_material_id`, so `game.raycast` and contact events can tell
    /// tarmac from grass from ice per entity. 0 = default surface. Purely an
    /// id — friction/restitution stay their own fields.
    pub material: u8,
    /// Unit normal of the surface this mover stands on (P2: general slopes).
    /// Written by the sweep whenever `on_floor` is set — up for flat boxes
    /// and prop roofs, the true slope normal for wedges and terrain — and
    /// zeroed when airborne, so `floor_normal != 0 ⇔ on_floor` on the mover
    /// path. Derived tier: recomputed every tick from geometry, never
    /// replicated. Read through [`floor_normal_of`], which maps the zero
    /// default to straight up (same discipline as `push_mass`).
    pub floor_normal: Vec3f,
    /// Unit normal of the wall the sweep clamped against this tick (points
    /// AWAY from the wall, back at the mover). Zero when nothing was hit —
    /// transient like `hit_wall`, and set alongside it. Wall jumps and wall
    /// slides read this; the AABB sweep reports axis-aligned normals, the
    /// capsule path (P3) true plane normals.
    pub wall_normal: Vec3f,
    /// Opt-in capsule collider (P3, D2's "later, optional upgrade"): this
    /// mover's motion routes through box3d `world_collide_mover` +
    /// `solve_planes` — real wall sliding, smooth corners, true contact
    /// normals — instead of the axis-separated AABB sweep. Default false:
    /// existing worlds keep the sweep byte-for-byte.
    pub capsule_collider: bool,
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

/// Effective shove resistance, applying the zero-reads-as-one rule.
pub fn push_mass_of(e: &Entity) -> f32 {
    if e.push_mass > 0.0 {
        e.push_mass
    } else {
        1.0
    }
}

#[cfg(test)]
mod facing_tests {
    use super::*;

    #[test]
    fn rigid_visual_heading_comes_from_orientation_not_stale_spawn_yaw() {
        let yaw = 0.73f32;
        let (s, c) = crate::math::sincos(yaw * 0.5);
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

/// Effective floor normal, applying the zero-reads-as-up rule: a grounded
/// mover whose floor never wrote a normal (default-constructed state, worlds
/// stepped by an older engine) stands on flat ground as far as any slope
/// logic is concerned.
pub fn floor_normal_of(e: &Entity) -> Vec3f {
    let n = e.floor_normal;
    if n.x * n.x + n.y * n.y + n.z * n.z > 1.0e-6 {
        n
    } else {
        vec3f(0.0, 1.0, 0.0)
    }
}

/// A purely visual box welded to an entity — eyes, arms, hats. No collision,
/// no physics. Offsets/rotation are OWNER-LOCAL (front at -z) and rotate/scale
/// with the owner's model; gone when the owner goes. Each field pairs with a
/// target the engine lerps toward (game.move_part), which is how arms reach.
#[derive(Clone, Default)]
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
    /// Engine-owned procedural rotation layered over `rot`. The part kit uses
    /// this for declarative gait/tail swings so `move_part` can keep owning
    /// its ordinary authored pose without a script callback fighting it.
    pub procedural_rot: Vec3f,
    /// A procedural animation stays out of the renderer's static slab even at
    /// the oscillator's zero crossing. Purely presentational: parts remain
    /// absent from collision, physics and navigation.
    pub procedural_anim: bool,
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
#[derive(Clone, Copy, PartialEq, Debug, Default)]
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
#[derive(Clone, Debug, Default)]
pub struct HudSlot {
    pub text: String,
    pub color: Vec4f,
    pub size: f32,
    pub anchor: HudAnchor,
}

/// A HUD gauge (speedometer, boost). Fraction 0..1 fills left to right.
#[derive(Clone, Debug)]
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
    /// Inputs for the shared analytic daylight/twilight model.
    pub turbidity: f32,
    pub sky_strength: f32,
    pub sun_strength: f32,
    /// User compensation on top of mean-luminance auto exposure.
    pub exposure_ev: f32,
}

impl Default for SkyConfig {
    fn default() -> Self {
        // The sky every app gets. The warm, slightly deeper horizon and the
        // thin haze were tuned on the sandbox's village demo and looked
        // right there — a sunset that reads as air rather than a grey veil —
        // so they belong to the engine, not to one scene: the viewer and
        // every game now open on the same sky.
        Self {
            top: vec4(0.32, 0.58, 0.9, 1.0),
            horizon: vec4(0.66, 0.76, 0.80, 1.0),
            ground: vec4(0.68, 0.75, 0.66, 1.0),
            ground_bottom: vec4(0.3, 0.4, 0.3, 1.0),
            fog: 0.0015,
            turbidity: 2.5,
            sky_strength: 1.0,
            sun_strength: 4.0,
            exposure_ev: 0.0,
        }
    }
}

/// What `game.sun({...})` asked for. The sim stores only the request — it
/// cannot depend on `makepad_draw`, so the renderer resolves this against
/// the shared `SceneSun` model (see game_render's `resolve_sun`). Lighting
/// is presentation: nothing here is ever read by the step.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SunConfig {
    /// Local solar hour 0..24. `None` keeps the default rig.
    pub time_of_day: Option<f32>,
    /// Latitude for the solar model, degrees.
    pub latitude: f32,
    /// Explicit direction toward the sun (y-up), overriding `time_of_day`.
    pub dir: Option<Vec3f>,
    /// Direct-term multiplier.
    pub color: Option<Vec3f>,
    /// Flat ambient, applied to both hemisphere terms.
    pub ambient: Option<Vec3f>,
    /// How much brighter the DISC is than the DOME at full daylight, as a
    /// ratio of luminances. `None` keeps the stock split (roughly 2.6:1),
    /// which is a soft, forgiving key for a stylised world. A viewer that
    /// wants a CLEAR sky asks for around 9: measured clear daylight puts
    /// only about a tenth of the light in the dome, and that is the
    /// difference between shadows that read and shadows that fill in.
    ///
    /// Applied to the DAYLIGHT rig only — the twilight and night ramps run
    /// on top of it untouched, so an evening keeps its own floor.
    pub daylight_balance: Option<f32>,
    /// How dark cast shadows draw, 0..1.
    pub shadow_alpha: Option<f32>,
}

/// What `game.tune({...})` asked for at the WORLD level: scalars every block
/// of a kind reads each tick, so one call retunes a whole fleet without
/// touching a single spawn line. Unlike [`SunConfig`] this IS read by the
/// step — it is gameplay, not presentation.
///
/// A struct rather than loose `GameWorld` fields for one concrete reason:
/// the derived `GameWorld::default()` then gets the NEUTRAL values, where a
/// bare `f32` field would default to 0.0 and silently freeze every car in
/// any world not built through `GameWorld::new()` (the trap `gravity`
/// documents two constructors up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldTuning {
    /// Multiplies every car's authored `top_speed` (and its acceleration, so
    /// the 0-to-top time is unchanged) each tick. 1.0 = exactly as authored.
    /// Stored raw; read through [`WorldTuning::car_speed_scale`].
    pub car_speed: f32,
}

impl Default for WorldTuning {
    fn default() -> Self {
        Self { car_speed: 1.0 }
    }
}

impl WorldTuning {
    /// The band a car-speed scale may take. A car that cannot crawl and a car
    /// that cannot be caught are both broken games, so the setters clamp
    /// rather than refuse.
    pub const CAR_SPEED_MIN: f32 = 0.2;
    pub const CAR_SPEED_MAX: f32 = 5.0;

    /// Clamp an incoming scale into the band; a non-finite value means
    /// "as authored" rather than a frozen or infinite fleet.
    pub fn sanitize_car_speed(scale: f32) -> f32 {
        if scale.is_finite() {
            scale.clamp(Self::CAR_SPEED_MIN, Self::CAR_SPEED_MAX)
        } else {
            1.0
        }
    }

    /// The multiplier a car applies THIS tick — always finite and in band,
    /// even if a snapshot or a hand-built world left the field at zero.
    pub fn car_speed_scale(&self) -> f32 {
        Self::sanitize_car_speed(self.car_speed)
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

#[derive(Default, Clone, Copy, Debug)]
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
    /// The fighting actions (mix.md §3.3/K3). Wire bits existed since S4;
    /// these are the LOCAL device's lanes for them, so a pad punch reads
    /// through `action_held`/`PlayerInput::held` exactly like a pad jump.
    /// Throw deliberately reuses `grab` and needs no field of its own.
    pub punch: bool,
    pub punch_pressed: bool,
    pub kick: bool,
    pub kick_pressed: bool,
    pub guard: bool,
    pub guard_pressed: bool,
}
