//! GameView — the in-process game engine pane.
//!
//! Hosts the kid's game: evaluates `game.splash` in a dedicated splash isolate
//! (aichat-style incremental re-eval, so the world rebuilds live while the AI
//! streams edits), simulates a small fixed-step AABB world, renders it to an
//! offscreen 3D pass composited into the pane, and answers the agent harness
//! (`tools/ag`) through `.agent/` file RPC: peek captures, scripted input-tape
//! test runs, and an error/log round-trip so the AI can see what went wrong.
//!
//! The physics is deliberately tiny (gravity + axis-separated AABB sweeps —
//! the same vocabulary Godot's CharacterBody gave the AI). It is engine-sized
//! for kid platformers, not a solver.
// TODO(aigame): swap the mini-physics for libs/box3d once the xr Rapier->box3d
// port lands; the script API below is the stable surface.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

#[cfg(not(headless))]
use makepad_widgets::makepad_platform::event::GamepadState;
use makepad_widgets::makepad_platform::makepad_micro_serde::*;
use makepad_widgets::makepad_script::numeric::NumericValue;
use makepad_widgets::widget_async::{CxSplashVmExt, SplashVmId, MAIN_SPLASH_VM_ID};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    mod.draw.DrawGameTexture = mod.std.set_type_default() do #(DrawGameTexture::script_shader(vm)){
        ..mod.draw.DrawQuad
        scene_texture: texture_2d(float)

        pixel: fn() {
            let color = self.scene_texture.sample_as_bgra(self.pos)
            return Pal.premul(color)
        }
    }

    // The game cube: DrawCube + per-instance emission and distance fog.
    mod.draw.DrawGameCube = mod.std.set_type_default() do #(DrawGameCube::script_shader(vm)){
        ..mod.draw.DrawCube
        v_fog: varying(float)

        vertex: fn() {
            let pos = self.get_size() * self.geom.geom_pos + self.get_pos()
            let model_view = self.draw_list.view_transform * self.transform
            let normal4 = model_view * vec4(
                self.geom.geom_normal.x,
                self.geom.geom_normal.y,
                self.geom.geom_normal.z,
                0.0
            )
            let normal = normalize(normal4.xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(normal, normalize(self.light_dir)), 0.0)
            self.lit_color = self.get_color(dp)
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        get_color: fn(dp: float) {
            let ambient = self.color.xyz * 0.28
            let lit = ambient + self.color.xyz * dp * 0.72
            // Emission: glowing eyes, beacons, bolts (energy ramps at runtime).
            let glowing = lit + self.color.xyz * self.glow * 0.6
            return vec4(glowing, self.color.w)
        }

        pixel: fn() {
            let fogged = mix(self.lit_color.xyz, self.fog_color, self.v_fog)
            return vec4(fogged, self.lit_color.w)
        }
    }

    // Same shading, alpha-blended: water, sensor ghosts, blob shadows.
    mod.draw.DrawGameAlpha = mod.std.set_type_default() do #(DrawGameAlpha::script_shader(vm)){
        ..mod.draw.DrawGameCube
        alpha_blend: true
        backface_culling: false
    }

    // Sky dome: a big cube around the camera, gradient by view direction
    // (the Godot ProceduralSkyMaterial look).
    mod.draw.DrawGameSky = mod.std.set_type_default() do #(DrawGameSky::script_shader(vm)){
        ..mod.draw.DrawCube
        backface_culling: false
        v_dir: varying(vec3f)

        vertex: fn() {
            let pos = self.get_size() * self.geom.geom_pos + self.get_pos()
            let model_view = self.draw_list.view_transform * self.transform
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            self.v_dir = self.geom.geom_pos
            let view_pos = self.draw_pass.camera_view * self.world
            let clip = self.draw_pass.camera_projection * view_pos
            // Pin the sky to the far plane (z ~= w) — the skybox trick Godot's
            // background pass amounts to: the dome never clips against the far
            // plane no matter its world size, and everything else wins depth.
            self.vertex_pos = vec4(clip.x, clip.y, clip.w * 0.99995, clip.w)
        }

        pixel: fn() {
            let y = normalize(self.v_dir).y
            let up = clamp(y * 2.2, 0.0, 1.0)
            let down = clamp((0.0 - y) * 2.2, 0.0, 1.0)
            let sky = mix(self.sky_horizon, self.sky_top, up)
            let ground = mix(self.sky_ground, self.sky_bottom, down)
            let color = mix(ground, sky, step(0.0, y))
            return vec4(color, 1.0)
        }
    }

    // The smooth terrain mesh: per-vertex colored triangles, flat normals.
    mod.draw.DrawGameTerrain = mod.std.set_type_default() do #(DrawGameTerrain::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)
        lit_color: varying(vec4f)
        world: varying(vec4f)
        v_fog: varying(float)

        vertex: fn() {
            let pos = vec3(self.geom.pos_nx.x, self.geom.pos_nx.y, self.geom.pos_nx.z)
            let normal_in = vec3(self.geom.pos_nx.w, self.geom.ny_nz_uv.x, self.geom.ny_nz_uv.y)
            let model_view = self.draw_list.view_transform * self.transform
            let world_normal = normalize((model_view * vec4(normal_in.x, normal_in.y, normal_in.z, 0.0)).xyz)
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0)
            let view_pos = self.draw_pass.camera_view * self.world
            let dp = max(dot(world_normal, normalize(self.light_dir)), 0.0)
            let ambient = self.geom.color.xyz * 0.34
            self.lit_color = vec4(ambient + self.geom.color.xyz * dp * 0.66, self.geom.color.w)
            self.v_fog = 1.0 - exp(0.0 - length(view_pos.xyz) * self.fog_density)
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            return vec4(mix(self.lit_color.xyz, self.fog_color, self.v_fog), self.lit_color.w)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip)
        }
    }

    mod.widgets.GameViewBase = #(GameView::register_widget(vm))
    mod.widgets.GameView = set_type_default() do mod.widgets.GameViewBase{
        width: Fill
        height: Fill
        draw_hud +: {
            text_style: theme.font_bold{font_size: 22}
            color: #xffffffee
        }
        draw_label +: {
            text_style: theme.font_bold{font_size: 11}
            color: #xffffffdd
        }
        draw_dot +: {
            color: #xffffffb8
        }
        draw_cube +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_alpha +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_terrain +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawGameTexture {
    #[deref]
    draw_super: DrawQuad,
}

/// DrawCube + per-instance emission (`glow`) and per-instance fog params.
/// Instance-field rule: only #[live] instance fields after the deref chain.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameCube {
    #[deref]
    pub cube: DrawCube,
    #[live(0.0)]
    pub glow: f32,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
}

/// Alpha-blended variant: water, sensor ghosts, blob shadows.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameAlpha {
    #[deref]
    pub cube: DrawGameCube,
}

/// Sky dome gradient (colors are instances so Rust sets them per frame).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameSky {
    #[deref]
    pub cube: DrawCube,
    #[live(vec3(0.32, 0.58, 0.9))]
    pub sky_top: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub sky_horizon: Vec3f,
    #[live(vec3(0.68, 0.75, 0.66))]
    pub sky_ground: Vec3f,
    #[live(vec3(0.3, 0.4, 0.3))]
    pub sky_bottom: Vec3f,
}

/// The smooth terrain mesh (PbrVertex layout: per-vertex color).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGameTerrain {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live(vec3(0.75, 0.87, 0.96))]
    pub fog_color: Vec3f,
    #[live(0.0)]
    pub fog_density: f32,
}

const TICK_DT: f32 = 1.0 / 60.0;
const EVAL_INSTRUCTION_LIMIT: usize = 2_000_000;
const TICK_INSTRUCTION_LIMIT: usize = 500_000;
const AGENT_POLL_TICKS: u64 = 15;
const PEEK_SNAPS: usize = 4;
const PEEK_SNAP_GAP_TICKS: u64 = 18;

const GAME_PREFIX: &str = "use mod.prelude.widgets.*\n";

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

/// Smooth heightfield: vertex heights on an N×N grid (row-major z*cells+x),
/// rendered as one triangulated mesh (flat per-tri normals, Godot-style) and
/// collided by height lookup instead of per-column AABBs.
#[derive(Clone)]
pub struct Terrain {
    /// Vertices per side (the API's `cells` value).
    pub cells: usize,
    pub cell_size: f32,
    /// World x/z of vertex (0,0); the grid is square and centered.
    pub origin: f32,
    pub heights: Vec<f32>,
    pub colors: Vec<Vec4f>,
    /// Bumped on every rebuild so the GPU mesh regenerates lazily.
    pub revision: u64,
}

/// Reported as the hit id when a sweep stops against the terrain.
pub const TERRAIN_ID: u64 = u64::MAX;

impl Terrain {
    /// Piecewise-planar ground height at (x, z): the two triangles per cell,
    /// same split the mesh uses, so collision and pixels agree. None outside.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        let fx = (x - self.origin) / self.cell_size;
        let fz = (z - self.origin) / self.cell_size;
        if fx < 0.0 || fz < 0.0 {
            return None;
        }
        let max = (self.cells - 1) as f32;
        if fx >= max || fz >= max {
            return None;
        }
        let ix = fx.floor() as usize;
        let iz = fz.floor() as usize;
        let u = fx - ix as f32;
        let v = fz - iz as f32;
        let h = |gx: usize, gz: usize| self.heights[gz * self.cells + gx];
        let (h00, h10, h01, h11) = (h(ix, iz), h(ix + 1, iz), h(ix, iz + 1), h(ix + 1, iz + 1));
        Some(if u + v < 1.0 {
            h00 + (h10 - h00) * u + (h01 - h00) * v
        } else {
            h11 + (h01 - h11) * (1.0 - u) + (h10 - h11) * (1.0 - v)
        })
    }

    /// Max ground height under an AABB footprint (corners + center) — what a
    /// box standing here rests on.
    pub fn floor_under(&self, pos: Vec3f, half: Vec3f) -> Option<f32> {
        let probes = [
            (pos.x, pos.z),
            (pos.x - half.x, pos.z - half.z),
            (pos.x + half.x, pos.z - half.z),
            (pos.x - half.x, pos.z + half.z),
            (pos.x + half.x, pos.z + half.z),
        ];
        let mut best: Option<f32> = None;
        for (x, z) in probes {
            if let Some(h) = self.height_at(x, z) {
                best = Some(best.map_or(h, |b: f32| b.max(h)));
            }
        }
        best
    }
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

/// One line of screen text. `size`/`color.w` of 0 mean "use the slot default".
#[derive(Clone, Default)]
pub struct HudSlot {
    pub text: String,
    pub color: Vec4f,
    pub size: f32,
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

#[derive(Clone)]
struct GameTimer {
    at_tick: u64,
    func: ScriptObjectRef,
}

/// One frame-indexed scripted input event (same shape as the Godot tapes).
#[derive(SerJson, DeJson, Clone, Default)]
pub struct TapeEvent {
    pub f: u64,
    pub press: Option<String>,
    pub release: Option<String>,
}

#[derive(SerJson, DeJson, Clone, Default)]
pub struct Tape {
    pub events: Vec<TapeEvent>,
    pub probe: Vec<String>,
}

#[derive(SerJson, DeJson, Clone, Default)]
struct TestRequest {
    frames: u64,
    tape: String,
    every: u64,
}

struct TestRun {
    frame: u64,
    frames: u64,
    capture_every: u64,
    tape: Tape,
    probe_lines: Vec<String>,
    captures: usize,
}

struct PeekRun {
    snaps_left: usize,
    next_at_tick: u64,
}

/// Everything the script API reads/writes. Shared (Rc<RefCell>) between the
/// widget and the native `game` handle registered into the isolate, so script
/// calls mutate it synchronously — no async widget trampoline, deterministic
/// ordering, and world-building during eval completes before eval returns.
#[derive(Default)]
pub struct GameWorld {
    pub entities: Vec<Entity>,
    next_id: u64,
    pub gravity: f32,
    on_tick: Option<ScriptObjectRef>,
    on_touch: Option<ScriptObjectRef>,
    timers: Vec<GameTimer>,
    /// HUD: center banner (game.text), top line, and the small hint line.
    pub hud_center: HudSlot,
    pub hud_top: HudSlot,
    pub hud_hint: HudSlot,
    pub crosshair: bool,
    /// Camera requests from script.
    pub cam_target: Vec3f,
    pub cam_distance: f32,
    pub cam_follow: u64,
    pub cam_side: bool,
    /// Third-person rig: pivot entity (0 = off), pivot height, boom length.
    pub cam_third: u64,
    pub cam_height: f32,
    pub cam_boom: f32,
    /// One-shot pitch set from script, consumed by the widget next tick.
    pub cam_pitch_request: Option<f32>,
    /// Immediate-mode cables, cleared at the top of every tick.
    pub beams: Vec<Beam>,
    /// Input state, written by the ActionMap / tape, read by script.
    held: HashSet<LiveId>,
    pressed: HashSet<LiveId>,
    /// Gamepad state, merged with the keyboard at read time (never into
    /// `held`, so a pad release can't cancel a held key). Stick is analog.
    pad: PadState,
    /// Decoration: visual-only child boxes and billboard nametags.
    pub parts: Vec<Part>,
    pub labels: Vec<LabelDef>,
    /// Smooth heightfield ground (game.terrain smooth mode).
    pub terrain: Option<Terrain>,
    /// Sky/fog, enabled by game.sky().
    pub sky: Option<SkyConfig>,
    /// Orbit-camera yaw, mirrored from the widget each tick so scripts can do
    /// camera-relative movement ("run where the camera looks").
    pub cam_yaw: f32,
    /// Seeded per eval, so wander AI is repeatable under input tapes — an
    /// improvement over the Godot corpus, which called randomize().
    rng: u64,
    pub tick: u64,
    pub time: f64,
    log_pending: Vec<String>,
    /// PERF: bumped whenever anything a STATIC entity contributes to the
    /// screen changes (spawn/remove/restyle/sky). The renderer caches packed
    /// instance slabs for static content keyed by this — bump it or your
    /// static edit won't show.
    pub render_rev: u64,
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
}

impl GameWorld {
    /// Keyboard OR gamepad. The pad's stick maps onto the four directions at
    /// half deflection so `held("left")` works the same on both.
    fn action_held(&self, action: LiveId) -> bool {
        if self.held.contains(&action) {
            return true;
        }
        match action {
            x if x == live_id!(jump) => self.pad.jump,
            x if x == live_id!(shoot) => self.pad.shoot,
            x if x == live_id!(grab) => self.pad.grab,
            x if x == live_id!(left) => self.pad.axis_x < -0.5,
            x if x == live_id!(right) => self.pad.axis_x > 0.5,
            x if x == live_id!(up) => self.pad.axis_z < -0.5,
            x if x == live_id!(down) => self.pad.axis_z > 0.5,
            _ => false,
        }
    }

    fn action_pressed(&self, action: LiveId) -> bool {
        if self.pressed.contains(&action) {
            return true;
        }
        match action {
            x if x == live_id!(jump) => self.pad.jump_pressed,
            x if x == live_id!(shoot) => self.pad.shoot_pressed,
            x if x == live_id!(grab) => self.pad.grab_pressed,
            _ => false,
        }
    }

    fn reset_content(&mut self) {
        self.entities.clear();
        self.parts.clear();
        self.labels.clear();
        self.terrain = None;
        self.sky = None;
        self.next_id = 0;
        self.gravity = 30.0;
        self.on_tick = None;
        self.on_touch = None;
        self.timers.clear();
        self.hud_center = HudSlot::default();
        self.hud_top = HudSlot::default();
        self.hud_hint = HudSlot::default();
        self.crosshair = false;
        self.beams.clear();
        self.cam_target = vec3f(0.0, 2.0, 0.0);
        self.cam_distance = 18.0;
        self.cam_follow = 0;
        self.cam_side = false;
        self.cam_third = 0;
        self.cam_height = 1.6;
        self.cam_boom = 10.0;
        self.cam_pitch_request = None;
        self.rng = 0x9E37_79B9_7F4A_7C15;
        // A rebuilt world must never alias a previous world's slab revision.
        self.mark_render_dirty();
    }

    fn rand(&mut self) -> f64 {
        // xorshift64* — cheap, deterministic, plenty for wander timers.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn entity(&self, id: u64) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id == id)
    }

    fn entity_mut(&mut self, id: u64) -> Option<&mut Entity> {
        self.entities.iter_mut().find(|e| e.id == id)
    }

    fn log(&mut self, line: String) {
        self.log_pending.push(line);
    }

    /// See `render_rev`. Call after mutating anything static-visible.
    fn mark_render_dirty(&mut self) {
        self.render_rev = self.render_rev.wrapping_add(1);
    }

    /// Does a mutation of this entity id invalidate the static slab?
    fn is_static_visual(&self, id: u64) -> bool {
        self.entity(id).map_or(false, |e| e.kind == BodyKind::Static)
    }
}

/// How far the third-person boom may extend before hitting geometry: march
/// from the pivot toward the camera and stop at terrain or any solid box.
/// Entities tagged "scenery" are ignored (Godot keeps trees on a layer the
/// camera ray never sees, so foliage doesn't yank the view in).
fn camera_boom_limit(world: &GameWorld, pivot: Vec3f, dir: Vec3f, boom: f32) -> f32 {
    const STEPS: i32 = 32;
    for i in 1..=STEPS {
        let t = boom * i as f32 / STEPS as f32;
        let p = pivot + dir * t;
        if let Some(terrain) = &world.terrain {
            if let Some(h) = terrain.height_at(p.x, p.z) {
                if p.y < h + 0.2 {
                    return (t - 0.5).max(1.0);
                }
            }
        }
        for e in &world.entities {
            if e.sensor || e.tag == "scenery" {
                continue;
            }
            if !matches!(e.kind, BodyKind::Static | BodyKind::Kinematic) {
                continue;
            }
            if (p.x - e.pos.x).abs() < e.half.x
                && (p.y - e.pos.y).abs() < e.half.y
                && (p.z - e.pos.z).abs() < e.half.z
            {
                return (t - 0.5).max(1.0);
            }
        }
    }
    boom
}

fn axis_get(v: Vec3f, axis: usize) -> f32 {
    match axis {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

fn axis_set(v: &mut Vec3f, axis: usize, value: f32) {
    match axis {
        0 => v.x = value,
        1 => v.y = value,
        _ => v.z = value,
    }
}

fn overlaps(a_pos: Vec3f, a_half: Vec3f, b_pos: Vec3f, b_half: Vec3f) -> bool {
    (a_pos.x - b_pos.x).abs() < a_half.x + b_half.x
        && (a_pos.y - b_pos.y).abs() < a_half.y + b_half.y
        && (a_pos.z - b_pos.z).abs() < a_half.z + b_half.z
}

/// Move one axis and clamp against every solid; returns (clamped, hit_dir, hit_id).
fn sweep_axis(
    entities: &[Entity],
    self_id: u64,
    pos: Vec3f,
    half: Vec3f,
    axis: usize,
    delta: f32,
) -> (f32, f32, u64) {
    let mut new_axis = axis_get(pos, axis) + delta;
    let mut hit = 0.0f32;
    let mut hit_id = 0u64;
    for other in entities {
        if other.id == self_id || other.sensor {
            continue;
        }
        if !matches!(other.kind, BodyKind::Static | BodyKind::Kinematic) {
            continue;
        }
        let mut probe = pos;
        axis_set(&mut probe, axis, new_axis);
        if overlaps(probe, half, other.pos, other.half) {
            let gap = axis_get(half, axis) + axis_get(other.half, axis);
            if delta > 0.0 {
                new_axis = new_axis.min(axis_get(other.pos, axis) - gap);
                hit = 1.0;
                hit_id = other.id;
            } else if delta < 0.0 {
                new_axis = new_axis.max(axis_get(other.pos, axis) + gap);
                hit = -1.0;
                hit_id = other.id;
            }
        }
    }
    (new_axis, hit, hit_id)
}

/// Snapshot taken before a re-eval so a broken script never replaces a
/// working world ("last good" semantics).
struct WorldSnapshot {
    entities: Vec<Entity>,
    parts: Vec<Part>,
    labels: Vec<LabelDef>,
    terrain: Option<Terrain>,
    sky: Option<SkyConfig>,
    gravity: f32,
    on_tick: Option<ScriptObjectRef>,
    on_touch: Option<ScriptObjectRef>,
    timers: Vec<GameTimer>,
    hud_center: HudSlot,
    hud_top: HudSlot,
    hud_hint: HudSlot,
    crosshair: bool,
    cam_target: Vec3f,
    cam_distance: f32,
    cam_follow: u64,
    cam_side: bool,
    cam_third: u64,
    cam_height: f32,
    cam_boom: f32,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct GameView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawGameTexture,
    #[live]
    draw_cube: DrawGameCube,
    #[live]
    draw_alpha: DrawGameAlpha,
    #[live]
    draw_sky: DrawGameSky,
    #[live]
    draw_terrain: DrawGameTerrain,
    #[live]
    draw_hud: DrawText,
    #[live]
    draw_label: DrawText,
    /// The crosshair dot (and any future flat overlay quads).
    #[live]
    draw_dot: DrawColor,
    #[live(vec4(0.03, 0.045, 0.075, 1.0))]
    clear_color: Vec4f,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    area: Area,
    #[rust(false)]
    initialized: bool,
    // PERF: counters behind AIGAME_PERF=1 (see draw_scene).
    #[rust(std::env::var_os("AIGAME_PERF").is_some())]
    perf_enabled: bool,
    #[rust]
    perf_accum_us: u64,
    #[rust]
    perf_frames: u64,
    #[rust]
    perf_static_count: u64,
    #[rust]
    perf_dyn_count: u64,
    // PERF: unit shape geometries, built once (index = Shape::index()).
    #[rust]
    shape_geometries: [Option<Geometry>; 5],
    // PERF: packed static instance data per shape (opaque / alpha passes),
    // valid while slab_rev == world.render_rev.
    #[rust]
    static_slab: [Vec<f32>; 5],
    #[rust]
    static_slab_alpha: [Vec<f32>; 5],
    #[rust]
    slab_rev: Option<u64>,
    #[rust]
    slab_instance_count: u64,
    #[rust]
    world: Rc<RefCell<GameWorld>>,
    #[rust]
    vm_id: SplashVmId,
    #[rust]
    body: String,
    #[rust]
    eval_generation: u64,
    #[rust]
    last_eval_ok: bool,
    /// Error text of the last failed eval (all classes the isolate can
    /// produce — parse, runtime, pod, and shader-compiler errors all flow
    /// through the same captured-error sink). None while the eval is clean.
    #[rust]
    last_eval_error: Option<String>,
    /// Where the current game lives; `.agent/` goes under it.
    #[rust]
    project_dir: Option<PathBuf>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time_accum: f64,
    #[rust]
    last_time: Option<f64>,
    // Orbit camera (script sets target/distance; mouse orbits).
    #[rust(0.6f32)]
    orbit_yaw: f32,
    #[rust(-0.35f32)]
    orbit_pitch: f32,
    #[rust]
    orbit_last_abs: Option<DVec2>,
    /// Pane rect from the last draw, for mouse hit checks (raw mouse events,
    /// not the finger-hit system — same pattern as XrCamera's desktop orbit).
    #[rust]
    view_rect: Rect,
    #[rust]
    test_run: Option<TestRun>,
    #[rust]
    peek_run: Option<PeekRun>,
    /// Previous-tick gamepad button state, for press-edge detection.
    /// (The headless backend has no game input; the poll fn is stubbed there.)
    #[cfg(not(headless))]
    #[rust]
    pad_jump_prev: bool,
    #[cfg(not(headless))]
    #[rust]
    pad_shoot_prev: bool,
    #[cfg(not(headless))]
    #[rust]
    pad_grab_prev: bool,
    /// GPU mesh for the smooth terrain, rebuilt when the revision changes.
    #[rust]
    terrain_geometry: Option<Geometry>,
    #[rust]
    terrain_revision: u64,
}

impl GameView {
    // ── setup ───────────────────────────────────────────────────────────

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
        // NOTE: no world reset here — the startup eval may already have built
        // the world before the first draw. eval_body owns resets.
        self.next_frame = cx.new_next_frame();
    }

    pub fn set_project_dir(&mut self, dir: PathBuf) {
        self.project_dir = Some(dir);
    }

    fn agent_dir(&self) -> Option<PathBuf> {
        Some(self.project_dir.as_ref()?.join(".agent"))
    }

    // ── script isolate ──────────────────────────────────────────────────

    fn self_id(&self) -> usize {
        self as *const Self as usize
    }

    /// Feed (possibly streaming) game source. Evaluates incrementally like the
    /// Splash widget; a failed eval rolls the world back to the last good one
    /// and reports errors to `.agent/` for the AI.
    pub fn set_source(&mut self, cx: &mut Cx, source: &str) {
        if self.body == source {
            return;
        }
        self.body = source.to_string();
        self.eval_body(cx);
        self.redraw(cx);
    }

    #[allow(dead_code)]
    pub fn last_eval_ok(&self) -> bool {
        self.last_eval_ok
    }

    /// The last failed eval's error text, for the app to push back into the
    /// agent conversation. None when the current eval is clean.
    pub fn last_eval_error(&self) -> Option<&str> {
        self.last_eval_error.as_deref()
    }

    fn eval_body(&mut self, cx: &mut Cx) {
        if self.body.is_empty() {
            return;
        }
        if self.vm_id == MAIN_SPLASH_VM_ID {
            self.vm_id = cx.alloc_splash_vm_with_network(false);
            self.register_game_handle(cx);
        }
        self.eval_generation += 1;

        // Last-good: keep a copy of the world; the eval rebuilds from scratch.
        let snapshot = {
            let mut world = self.world.borrow_mut();
            let snapshot = WorldSnapshot {
                entities: std::mem::take(&mut world.entities),
                parts: std::mem::take(&mut world.parts),
                labels: std::mem::take(&mut world.labels),
                terrain: world.terrain.take(),
                sky: world.sky.take(),
                gravity: world.gravity,
                on_tick: world.on_tick.clone(),
                on_touch: world.on_touch.clone(),
                timers: std::mem::take(&mut world.timers),
                hud_center: std::mem::take(&mut world.hud_center),
                hud_top: std::mem::take(&mut world.hud_top),
                hud_hint: std::mem::take(&mut world.hud_hint),
                crosshair: world.crosshair,
                cam_target: world.cam_target,
                cam_distance: world.cam_distance,
                cam_follow: world.cam_follow,
                cam_side: world.cam_side,
                cam_third: world.cam_third,
                cam_height: world.cam_height,
                cam_boom: world.cam_boom,
            };
            world.reset_content();
            snapshot
        };

        let self_id = self.self_id();
        // The trailing "\n;" finalizes the stream: eval_with_append_source is a
        // STREAMING parser, so a last statement with no terminator is held back
        // as "possibly incomplete" and silently never runs — and game logic
        // (`on_tick`/`on_touch`) idiomatically sits last in the file. aichat
        // never hits this because its wrapper auto-closes with `}`. The empty
        // statement is harmless after any complete file. (bugs.md, my-game-5.)
        let code = format!("{}{}\n;", GAME_PREFIX, self.body);
        let script_mod = ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: String::new(),
            line: self_id,
            column: 0,
            code: String::new(),
            values: vec![],
        };

        let vm_id = self.vm_id;
        let errors = cx.with_script_vm_id(vm_id, |vm| {
            // Install the captured-error sink: run_core drains errors as they
            // occur (and streaming evals silence them); the sink is the only
            // reliable way to get them back for the AI.
            vm.bx.captured_errors = Some(Vec::new());
            let _ = vm.with_instruction_limit(EVAL_INSTRUCTION_LIMIT, |vm| {
                vm.eval_with_append_source(script_mod, &code, NIL.into())
            });
            vm.take_errors()
        });

        let generation = self.eval_generation;
        if errors.is_empty() {
            self.last_eval_ok = true;
            self.last_eval_error = None;
            let count = self.world.borrow().entities.len();
            self.append_log(&format!("eval #{generation}: ok, {count} entities"));
            self.write_agent_file("last_error.txt", "");
        } else {
            self.last_eval_ok = false;
            // Roll back so the kid keeps the world that worked.
            {
                let mut world = self.world.borrow_mut();
                world.entities = snapshot.entities;
                world.parts = snapshot.parts;
                world.labels = snapshot.labels;
                world.terrain = snapshot.terrain;
                world.sky = snapshot.sky;
                world.gravity = snapshot.gravity;
                world.on_tick = snapshot.on_tick;
                world.on_touch = snapshot.on_touch;
                world.timers = snapshot.timers;
                world.hud_center = snapshot.hud_center;
                world.hud_top = snapshot.hud_top;
                world.hud_hint = snapshot.hud_hint;
                world.crosshair = snapshot.crosshair;
                world.cam_target = snapshot.cam_target;
                world.cam_distance = snapshot.cam_distance;
                world.cam_follow = snapshot.cam_follow;
                world.cam_side = snapshot.cam_side;
                world.cam_third = snapshot.cam_third;
                world.cam_height = snapshot.cam_height;
                world.cam_boom = snapshot.cam_boom;
            }
            let joined = errors.join("\n");
            self.append_log(&format!("eval #{generation}: FAILED\n{joined}"));
            self.write_agent_file("last_error.txt", &format!("eval #{generation}\n{joined}\n"));
            self.last_eval_error = Some(joined);
        }
        if self.last_eval_ok {
            cx.widget_action(self.uid, GameViewAction::EvalOk { generation });
        } else {
            cx.widget_action(
                self.uid,
                GameViewAction::EvalFailed {
                    generation,
                    error: self.last_eval_error.clone().unwrap_or_default(),
                },
            );
        }
    }

    /// Register the synchronous `game` native handle into this view's isolate.
    fn register_game_handle(&mut self, cx: &mut Cx) {
        let world = self.world.clone();
        let vm_id = self.vm_id;
        cx.with_script_vm_id(vm_id, |vm| {
            let game_type = vm.new_handle_type(id_lut!(game));
            let dispatch_world = world.clone();
            vm.set_handle_call(game_type, move |vm, args, method| {
                game_dispatch(vm, &dispatch_world, args, method)
            });
            struct GameHandleGc;
            impl ScriptHandleGc for GameHandleGc {
                fn gc(&mut self) {}
            }
            let handle = vm.bx.heap.new_handle(game_type, Box::new(GameHandleGc));
            vm.set_injected_global(id!(game), handle.into());
        });
    }

    // ── logs / agent files ──────────────────────────────────────────────

    fn append_log(&self, line: &str) {
        let Some(dir) = self.agent_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        let stamped = format!("[t{}] {}\n", self.world.borrow().tick, line);
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("game.log"))
        {
            let _ = file.write_all(stamped.as_bytes());
        }
    }

    fn write_agent_file(&self, name: &str, contents: &str) {
        let Some(dir) = self.agent_dir() else { return };
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(name), contents);
    }

    fn state_report(&self) -> String {
        let world = self.world.borrow();
        let mut out = String::new();
        use std::fmt::Write;
        let _ = writeln!(out, "tick={} entities={}", world.tick, world.entities.len());
        for e in &world.entities {
            if e.kind == BodyKind::Static && e.tag.is_empty() {
                continue;
            }
            let _ = writeln!(
                out,
                "{} tag={} kind={} pos=({:.2},{:.2},{:.2}) vel=({:.2},{:.2},{:.2}) floor={}",
                e.id,
                if e.tag.is_empty() { "-" } else { &e.tag },
                match e.kind {
                    BodyKind::Static => "static",
                    BodyKind::Kinematic => "kinematic",
                    BodyKind::Mover => "mover",
                },
                e.pos.x, e.pos.y, e.pos.z,
                e.vel.x, e.vel.y, e.vel.z,
                e.on_floor,
            );
        }
        out
    }

    // ── agent RPC (peek / test) ─────────────────────────────────────────

    fn poll_agent_requests(&mut self, cx: &mut Cx) {
        let Some(dir) = self.agent_dir() else { return };

        let peek_request = dir.join("peek_request");
        if peek_request.exists() && self.peek_run.is_none() {
            let _ = std::fs::remove_file(&peek_request);
            let live = dir.join("live");
            let _ = std::fs::remove_dir_all(&live);
            let _ = std::fs::create_dir_all(&live);
            self.write_agent_file("live/state.txt", &self.state_report());
            let tick = self.world.borrow().tick;
            self.peek_run = Some(PeekRun {
                snaps_left: PEEK_SNAPS,
                next_at_tick: tick,
            });
        }

        let test_request = dir.join("test_request");
        if test_request.exists() && self.test_run.is_none() {
            let request = std::fs::read_to_string(&test_request)
                .ok()
                .and_then(|s| TestRequest::deserialize_json(&s).ok())
                .unwrap_or_default();
            let _ = std::fs::remove_file(&test_request);

            let tape = if request.tape.is_empty() {
                Tape::default()
            } else {
                let path = self
                    .project_dir
                    .as_ref()
                    .map(|p| p.join(&request.tape))
                    .unwrap_or_else(|| PathBuf::from(&request.tape));
                std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| Tape::deserialize_json(&s).ok())
                    .unwrap_or_default()
            };

            let cap = dir.join("cap");
            let _ = std::fs::remove_dir_all(&cap);
            let _ = std::fs::create_dir_all(&cap);

            // Restart the game so the run is repeatable from spawn state.
            self.reeval_for_test(cx);
            self.test_run = Some(TestRun {
                frame: 0,
                frames: request.frames.max(1),
                capture_every: request.every.max(1),
                tape,
                probe_lines: Vec::new(),
                captures: 0,
            });
        }
    }

    fn reeval_for_test(&mut self, cx: &mut Cx) {
        // Force a re-eval of the current body: rebuilds entities at spawn.
        let body = std::mem::take(&mut self.body);
        self.set_source(cx, &body);
    }

    fn tick_test_run(&mut self, cx: &mut Cx) -> bool {
        let Some(dir) = self.agent_dir() else {
            self.test_run = None;
            return false;
        };
        let Some(mut run) = self.test_run.take() else {
            return false;
        };

        // Scripted input for this frame.
        {
            let mut world = self.world.borrow_mut();
            for event in &run.tape.events {
                if event.f != run.frame {
                    continue;
                }
                if let Some(action) = &event.press {
                    let action = LiveId::from_str(action);
                    if world.held.insert(action) {
                        world.pressed.insert(action);
                    }
                }
                if let Some(action) = &event.release {
                    let action = LiveId::from_str(action);
                    world.held.remove(&action);
                }
            }
        }

        if run.frame % run.capture_every == 0 {
            cx.capture_next_frame_to_file(dir.join(format!("cap/f{:06}.png", run.frame)));
            run.captures += 1;
        }
        if run.frame % 15 == 0 {
            let world = self.world.borrow();
            for name in &run.tape.probe {
                let found = world.entities.iter().find(|e| &e.tag == name);
                if let Some(e) = found {
                    run.probe_lines.push(format!(
                        "[probe] f={} {} pos=({:.1},{:.1},{:.1}) vel=({:.1},{:.1},{:.1}) floor={}",
                        run.frame, name, e.pos.x, e.pos.y, e.pos.z,
                        e.vel.x, e.vel.y, e.vel.z, e.on_floor
                    ));
                } else {
                    run.probe_lines.push(format!("[probe] f={} {} MISSING", run.frame, name));
                }
            }
        }

        run.frame += 1;
        if run.frame >= run.frames {
            // Release everything the tape held down.
            {
                let mut world = self.world.borrow_mut();
                world.held.clear();
            }
            self.write_agent_file("probe.txt", &(run.probe_lines.join("\n") + "\n"));
            self.write_agent_file(
                "test_done",
                &format!("frames={} captures={}\n", run.frames, run.captures),
            );
            self.test_run = None;
        } else {
            self.test_run = Some(run);
        }
        true
    }

    fn tick_peek_run(&mut self, cx: &mut Cx) {
        let Some(dir) = self.agent_dir() else {
            self.peek_run = None;
            return;
        };
        let Some(mut run) = self.peek_run.take() else {
            return;
        };
        let tick = self.world.borrow().tick;
        if tick >= run.next_at_tick {
            let index = PEEK_SNAPS - run.snaps_left;
            cx.capture_next_frame_to_file(dir.join(format!("live/f{:04}.png", index)));
            run.snaps_left -= 1;
            run.next_at_tick = tick + PEEK_SNAP_GAP_TICKS;
        }
        if run.snaps_left > 0 {
            self.peek_run = Some(run);
        } else {
            // The last PNG lands a frame or two later; `ag` polls for the file.
            self.write_agent_file("live/done", "ok");
        }
    }

    // ── the fixed-step tick ─────────────────────────────────────────────

    /// Poll the most active gamepad into the world's PadState. Stick is
    /// analog (deadzone 0.22), dpad digital; A = jump, X = shoot — the same
    /// bindings AgentEye taught the Godot games. Merged with the keyboard at
    /// read time, never written into `held`.
    #[cfg(not(headless))]
    fn poll_gamepad(&mut self, cx: &mut Cx) {
        let mut best: Option<GamepadState> = None;
        let mut best_score = 0.0f32;
        for state in cx.game_input_states() {
            let GameInputState::Gamepad(pad) = state else {
                continue;
            };
            let score = pad.left_stick.x.abs() as f32
                + pad.left_stick.y.abs() as f32
                + pad.dpad_up
                + pad.dpad_down
                + pad.dpad_left
                + pad.dpad_right
                + pad.a
                + pad.x;
            if best.is_none() || score > best_score {
                best_score = score;
                best = Some(pad.clone());
            }
        }
        let (jump, shoot, grab, pad_state) = if let Some(pad) = best {
            const DEADZONE: f64 = 0.22;
            let stick_x = pad.left_stick.x as f64;
            // Stick up = forward = negative axis_z (axis_z is down-minus-up).
            let stick_z = -(pad.left_stick.y as f64);
            let mut axis_x = if stick_x.abs() > DEADZONE { stick_x } else { 0.0 };
            let mut axis_z = if stick_z.abs() > DEADZONE { stick_z } else { 0.0 };
            axis_x += (pad.dpad_right > 0.5) as i8 as f64 - (pad.dpad_left > 0.5) as i8 as f64;
            axis_z += (pad.dpad_down > 0.5) as i8 as f64 - (pad.dpad_up > 0.5) as i8 as f64;
            let jump = pad.a > 0.5;
            let shoot = pad.x > 0.5;
            let grab = pad.b > 0.5;
            (
                jump,
                shoot,
                grab,
                PadState {
                    axis_x: axis_x.clamp(-1.0, 1.0),
                    axis_z: axis_z.clamp(-1.0, 1.0),
                    jump,
                    jump_pressed: jump && !self.pad_jump_prev,
                    shoot,
                    shoot_pressed: shoot && !self.pad_shoot_prev,
                    grab,
                    grab_pressed: grab && !self.pad_grab_prev,
                },
            )
        } else {
            (false, false, false, PadState::default())
        };
        self.pad_jump_prev = jump;
        self.pad_shoot_prev = shoot;
        self.pad_grab_prev = grab;
        self.world.borrow_mut().pad = pad_state;
    }

    #[cfg(headless)]
    fn poll_gamepad(&mut self, _cx: &mut Cx) {}

    fn run_tick(&mut self, cx: &mut Cx) {
        let in_test = self.tick_test_run(cx);
        // Scripts steer relative to the camera ("run where the camera looks"),
        // so the EFFECTIVE yaw must be visible world state: the orbit yaw, or
        // 0 for the fixed side-on camera (where raw axes are already correct).
        // Tape runs pin it to 0 — repeatability must not depend on where the
        // kid happened to leave the camera.
        {
            let mut world = self.world.borrow_mut();
            // Script asked for a pitch (game.camera({pitch: ...})): apply it
            // to the live rig once, then the mouse owns it again.
            if let Some(pitch) = world.cam_pitch_request.take() {
                self.orbit_pitch = pitch;
            }
            // Beams are immediate-mode: whatever on_tick re-issues below
            // survives to render; everything else vanishes right here.
            world.beams.clear();
            world.cam_yaw = if world.cam_side || in_test {
                0.0
            } else {
                self.orbit_yaw
            };
        }
        if in_test {
            // Tapes own the input during a test; a bumped stick must not
            // contaminate a repeatable run.
            self.world.borrow_mut().pad = PadState::default();
        } else {
            self.poll_gamepad(cx);
        }

        // Call the script tick with (dt, input-snapshot) — input as a plain
        // object so the hot path costs no cross-boundary calls.
        let (on_tick, input_snapshot) = {
            let world = self.world.borrow();
            (world.on_tick.clone(), self.input_snapshot(&world))
        };
        if let Some(on_tick) = on_tick {
            self.call_script_fn2(cx, on_tick, ScriptValue::from_f64(TICK_DT as f64), input_snapshot);
        }

        // Timers.
        let due: Vec<GameTimer> = {
            let mut world = self.world.borrow_mut();
            let now = world.tick;
            let (due, rest): (Vec<_>, Vec<_>) =
                world.timers.drain(..).partition(|t| t.at_tick <= now);
            world.timers = rest;
            due
        };
        for timer in due {
            self.call_script_fn0(cx, timer.func);
        }

        // Physics + sensors.
        let touch_events = {
            let mut world = self.world.borrow_mut();
            step_world(&mut world);
            world.tick += 1;
            world.time += TICK_DT as f64;
            world.pressed.clear();
            collect_touches(&world)
        };
        let on_touch = self.world.borrow().on_touch.clone();
        if let Some(on_touch) = on_touch {
            for (a, b) in touch_events {
                let args = self.make_touch_args(cx, a, b);
                if let Some((av, bv)) = args {
                    self.call_script_fn2(cx, on_touch.clone(), av, bv);
                }
            }
        }

        // Agent RPC.
        if self.world.borrow().tick % AGENT_POLL_TICKS == 0 {
            self.poll_agent_requests(cx);
        }
        self.tick_peek_run(cx);
        self.flush_log();
        let _ = in_test;
    }

    fn input_snapshot(&self, world: &GameWorld) -> ScriptValue {
        // Built fresh per tick inside the isolate.
        let _ = world;
        NIL // replaced in call_script_fn2 via build_input_object
    }

    fn build_input_object(vm: &mut ScriptVm, world: &GameWorld) -> ScriptValue {
        let obj = vm.bx.heap.new_object();
        vm.bx.heap.set_object_storage_auto(obj);
        let trap = NoTrap;
        let key = |world: &GameWorld, name: LiveId| world.held.contains(&name);
        // Keyboard digital + gamepad analog, clamped — either device just works.
        let axis = ((key(world, live_id!(right)) as i8 - key(world, live_id!(left)) as i8) as f64
            + world.pad.axis_x)
            .clamp(-1.0, 1.0);
        let axis_z = ((key(world, live_id!(down)) as i8 - key(world, live_id!(up)) as i8) as f64
            + world.pad.axis_z)
            .clamp(-1.0, 1.0);
        let heap = &mut vm.bx.heap;
        heap.set_value(obj, id!(left).into(), ScriptValue::from_bool(world.action_held(live_id!(left))), trap);
        heap.set_value(obj, id!(right).into(), ScriptValue::from_bool(world.action_held(live_id!(right))), trap);
        heap.set_value(obj, id!(up).into(), ScriptValue::from_bool(world.action_held(live_id!(up))), trap);
        heap.set_value(obj, id!(down).into(), ScriptValue::from_bool(world.action_held(live_id!(down))), trap);
        heap.set_value(obj, id!(jump).into(), ScriptValue::from_bool(world.action_held(live_id!(jump))), trap);
        heap.set_value(obj, id!(jump_pressed).into(), ScriptValue::from_bool(world.action_pressed(live_id!(jump))), trap);
        heap.set_value(obj, id!(shoot).into(), ScriptValue::from_bool(world.action_held(live_id!(shoot))), trap);
        heap.set_value(obj, id!(shoot_pressed).into(), ScriptValue::from_bool(world.action_pressed(live_id!(shoot))), trap);
        heap.set_value(obj, id!(grab).into(), ScriptValue::from_bool(world.action_held(live_id!(grab))), trap);
        heap.set_value(obj, id!(grab_pressed).into(), ScriptValue::from_bool(world.action_pressed(live_id!(grab))), trap);
        heap.set_value(obj, id!(axis_x).into(), ScriptValue::from_f64(axis), trap);
        heap.set_value(obj, id!(axis_z).into(), ScriptValue::from_f64(axis_z), trap);
        // Camera-relative movement: what the kid MEANS by "left" is screen-left.
        // Camera basis on the ground plane: forward = (sin y, -cos y), right =
        // (cos y, sin y) — so rotate the raw axes by +yaw. Scripts should walk
        // with these; the raw axes stay for side-scrollers and custom schemes.
        let yaw = world.cam_yaw;
        let move_x = axis * yaw.cos() as f64 - axis_z * yaw.sin() as f64;
        let move_z = axis * yaw.sin() as f64 + axis_z * yaw.cos() as f64;
        heap.set_value(obj, id!(move_x).into(), ScriptValue::from_f64(move_x), trap);
        heap.set_value(obj, id!(move_z).into(), ScriptValue::from_f64(move_z), trap);
        obj.into()
    }

    fn make_touch_args(&self, _cx: &mut Cx, a: u64, b: u64) -> Option<(ScriptValue, ScriptValue)> {
        Some((ScriptValue::from_f64(a as f64), ScriptValue::from_f64(b as f64)))
    }

    fn call_script_fn0(&mut self, cx: &mut Cx, func: ScriptObjectRef) {
        self.call_script(cx, func, &[]);
    }

    fn call_script_fn2(
        &mut self,
        cx: &mut Cx,
        func: ScriptObjectRef,
        a: ScriptValue,
        b: ScriptValue,
    ) {
        self.call_script(cx, func, &[a, b]);
    }

    fn call_script(&mut self, cx: &mut Cx, func: ScriptObjectRef, args: &[ScriptValue]) {
        if self.vm_id == MAIN_SPLASH_VM_ID {
            return;
        }
        let world = self.world.clone();
        let vm_id = self.vm_id;
        let errors = cx.with_script_vm_id(vm_id, |vm| {
            let args_obj = vm.bx.heap.new_object();
            vm.bx.heap.set_object_storage_vec2(args_obj);
            vm.bx.heap.clear_object_deep(args_obj);
            for value in args {
                // NIL positional slots become the fresh input snapshot.
                let value = if value.is_nil() {
                    Self::build_input_object(vm, &world.borrow())
                } else {
                    *value
                };
                let trap = vm.bx.threads.cur().trap.pass();
                vm.bx.heap.vec_push(args_obj, NIL, value, trap);
            }
            vm.bx.captured_errors = Some(Vec::new());
            let _ = vm.with_instruction_limit(TICK_INSTRUCTION_LIMIT, |vm| {
                vm.call_with_args_object_with_me(func.as_object().into(), args_obj, NIL)
            });
            vm.take_errors()
        });
        if !errors.is_empty() {
            let joined = errors.join("\n");
            self.append_log(&format!("script error:\n{joined}"));
            self.write_agent_file("last_error.txt", &format!("runtime\n{joined}\n"));
            // Push to the app, which decides whether to wake the agent — a
            // runtime error the kid just hit is invisible to the AI otherwise.
            cx.widget_action(
                self.uid,
                GameViewAction::RuntimeError {
                    generation: self.eval_generation,
                    error: joined,
                },
            );
        }
    }

    fn flush_log(&mut self) {
        let pending: Vec<String> = std::mem::take(&mut self.world.borrow_mut().log_pending);
        for line in pending {
            self.append_log(&line);
        }
    }

    // ── camera / render ─────────────────────────────────────────────────

    fn scene_state(&self, rect: Rect, time: f64) -> Option<SceneState3D> {
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return None;
        }
        let world = self.world.borrow();
        // Tape runs pin the camera completely — captures must not depend on
        // where the kid happened to leave the mouse.
        let in_test = self.test_run.is_some();

        // Third-person rig: pivot above the entity, drag orbits around it,
        // boom slides in when geometry blocks the view (the Godot player cam).
        if world.cam_third != 0 {
            if let Some(e) = world.entity(world.cam_third) {
                let pivot = e.pos + vec3f(0.0, world.cam_height, 0.0);
                let (yaw, pitch) = if in_test {
                    (0.0f32, -0.35f32)
                } else {
                    (self.orbit_yaw, self.orbit_pitch.clamp(-1.2, 0.25))
                };
                let forward = vec3f(
                    yaw.sin() * pitch.cos(),
                    pitch.sin(),
                    -yaw.cos() * pitch.cos(),
                )
                .normalize();
                let boom = camera_boom_limit(&world, pivot, forward * -1.0, world.cam_boom);
                let camera_pos = pivot - forward * boom;
                let view = Mat4f::look_at(camera_pos, pivot, vec3f(0.0, 1.0, 0.0));
                let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
                // Near plane 1.0 (Godot's CAM_NEAR): a creature overlapping the
                // lens clips open instead of filling the screen with one giant
                // polygon.
                let projection = Mat4f::perspective(40.0, aspect, 1.0, 500.0);
                return Some(SceneState3D {
                    time,
                    camera_pos,
                    view,
                    projection,
                    viewport_rect: rect,
                });
            }
        }

        let mut target = world.cam_target;
        if world.cam_follow != 0 {
            if let Some(e) = world.entity(world.cam_follow) {
                target = e.pos;
            }
        }
        let distance = world.cam_distance.max(0.5);
        let (yaw, pitch) = if world.cam_side {
            // Side-on 2D style camera: look down -z.
            (0.0f32, -0.08f32)
        } else if in_test {
            // The widget's startup defaults: deterministic captures.
            (0.6f32, -0.35f32)
        } else {
            (self.orbit_yaw, self.orbit_pitch.clamp(-1.45, 1.45))
        };
        let forward = vec3f(
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            -yaw.cos() * pitch.cos(),
        )
        .normalize();
        // Camera sits behind the target looking along `forward` at it.
        let camera_pos = target - forward * distance;
        let view = Mat4f::look_at(camera_pos, target, vec3f(0.0, 1.0, 0.0));
        let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
        let projection = Mat4f::perspective(40.0, aspect, 1.0, 500.0);
        Some(SceneState3D {
            time,
            camera_pos,
            view,
            projection,
            viewport_rect: rect,
        })
    }

    fn set_pass_camera(&self, cx: &mut Cx, scene: &SceneState3D) {
        let camera_inv = scene.view.invert();
        let pass_uniforms = &mut cx.passes[self.pass.draw_pass_id()].pass_uniforms;
        pass_uniforms.camera_projection = scene.projection;
        pass_uniforms.camera_projection_r = scene.projection;
        pass_uniforms.camera_view = scene.view;
        pass_uniforms.camera_view_r = scene.view;
        pass_uniforms.depth_projection = scene.projection;
        pass_uniforms.depth_projection_r = scene.projection;
        pass_uniforms.depth_view = scene.view;
        pass_uniforms.depth_view_r = scene.view;
        pass_uniforms.camera_inv = camera_inv;
        pass_uniforms.camera_inv_r = camera_inv;
    }

    /// Rebuild the terrain GPU mesh when the world's terrain revision moved.
    /// Godot-style: two triangles per cell, verts duplicated per triangle so
    /// normals are flat, per-tri color = average of its corners.
    fn ensure_terrain_geometry(&mut self, cx: &mut Cx, terrain: &Terrain) -> GeometryId {
        if self.terrain_geometry.is_some() && self.terrain_revision == terrain.revision {
            return self.terrain_geometry.as_ref().unwrap().geometry_id();
        }
        let n = terrain.cells;
        let mut vertices: Vec<f32> = Vec::with_capacity((n - 1) * (n - 1) * 2 * 3 * 16);
        let mut indices: Vec<u32> = Vec::with_capacity((n - 1) * (n - 1) * 6);
        let world_pos = |gx: usize, gz: usize| -> Vec3f {
            vec3f(
                terrain.origin + gx as f32 * terrain.cell_size,
                terrain.heights[gz * n + gx],
                terrain.origin + gz as f32 * terrain.cell_size,
            )
        };
        let push_tri = |vertices: &mut Vec<f32>, indices: &mut Vec<u32>, a: Vec3f, b: Vec3f, c: Vec3f, color: Vec4f| {
            let normal = Vec3f::cross(b - a, c - a).normalize();
            for p in [a, b, c] {
                let base = vertices.len() as u32 / 16;
                let _ = base;
                // PbrVertex: pos_nx, ny_nz_uv, color, tangent — 16 floats.
                vertices.extend_from_slice(&[
                    p.x, p.y, p.z, normal.x, normal.y, normal.z, 0.0, 0.0, color.x, color.y,
                    color.z, color.w, 1.0, 0.0, 0.0, 1.0,
                ]);
                indices.push(vertices.len() as u32 / 16 - 1);
            }
        };
        for gz in 0..n - 1 {
            for gx in 0..n - 1 {
                let a = world_pos(gx, gz);
                let b = world_pos(gx + 1, gz);
                let c = world_pos(gx, gz + 1);
                let d = world_pos(gx + 1, gz + 1);
                let color_at = |gx: usize, gz: usize| terrain.colors[gz * n + gx];
                let c0 = color_at(gx, gz);
                let c1 = color_at(gx + 1, gz);
                let c2 = color_at(gx, gz + 1);
                let c3 = color_at(gx + 1, gz + 1);
                let avg3 = |x: Vec4f, y: Vec4f, z: Vec4f| {
                    vec4(
                        (x.x + y.x + z.x) / 3.0,
                        (x.y + y.y + z.y) / 3.0,
                        (x.z + y.z + z.z) / 3.0,
                        1.0,
                    )
                };
                // Same diagonal split as Terrain::height_at, CCW seen from +y.
                push_tri(&mut vertices, &mut indices, a, c, b, avg3(c0, c2, c1));
                push_tri(&mut vertices, &mut indices, b, c, d, avg3(c1, c2, c3));
            }
        }
        let geometry = Geometry::new(cx);
        geometry.update(cx, indices, vertices);
        let id = geometry.geometry_id();
        self.terrain_geometry = Some(geometry);
        self.terrain_revision = terrain.revision;
        id
    }

    /// Unit geometry for a shape, built once and shared by every instance
    /// (index = Shape::index()). All shapes span [-0.5, 0.5] so `cube_size`
    /// scales them exactly like the built-in cube.
    fn ensure_shape_geometry(&mut self, cx: &mut Cx, shape: Shape) -> GeometryId {
        let slot = &mut self.shape_geometries[shape.index()];
        if let Some(geometry) = slot {
            return geometry.geometry_id();
        }
        let (vertices, indices) = shape_geometry_data(shape);
        let geometry = Geometry::new(cx);
        geometry.update(cx, indices, vertices);
        let id = geometry.geometry_id();
        *slot = Some(geometry);
        id
    }

    /// PERF: pack one instance in the exact slice layout `DrawCube::draw`
    /// emits (DrawVars::as_slice covers the trailing glow/fog instance
    /// fields), so slab content and immediate draws are indistinguishable.
    fn pack_cube_instance(
        &mut self,
        alpha: bool,
        out_index: usize,
        transform: Mat4f,
        size: Vec3f,
        color: Vec4f,
        glow: f32,
    ) {
        if alpha {
            self.draw_alpha.cube.cube.transform = transform;
            self.draw_alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
            self.draw_alpha.cube.cube.cube_size = size;
            self.draw_alpha.cube.cube.color = color;
            self.draw_alpha.cube.cube.depth_clip = 1.0;
            self.draw_alpha.cube.glow = glow;
            let slice = self.draw_alpha.cube.cube.draw_vars.as_slice();
            self.static_slab_alpha[out_index].extend_from_slice(slice);
            self.slab_instance_count += 1;
        } else {
            self.draw_cube.cube.transform = transform;
            self.draw_cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
            self.draw_cube.cube.cube_size = size;
            self.draw_cube.cube.color = color;
            self.draw_cube.cube.depth_clip = 1.0;
            self.draw_cube.glow = glow;
            let slice = self.draw_cube.cube.draw_vars.as_slice();
            self.static_slab[out_index].extend_from_slice(slice);
            self.slab_instance_count += 1;
        }
    }

    /// PERF: rebuild the packed static instance slabs. Only runs when
    /// `world.render_rev` moved — the world bumps it on every mutation that
    /// changes what static content looks like (see mark_render_dirty).
    fn rebuild_static_slabs(&mut self, world: &GameWorld) {
        for slab in self.static_slab.iter_mut() {
            slab.clear();
        }
        for slab in self.static_slab_alpha.iter_mut() {
            slab.clear();
        }
        self.slab_instance_count = 0;
        // Static entities (opaque and sensor/alpha).
        for e in world.entities.iter().filter(|e| e.kind == BodyKind::Static) {
            let mut transform = Mat4f::rotation(vec3f(0.0, e.yaw, 0.0));
            transform.v[12] = e.pos.x;
            transform.v[13] = e.pos.y;
            transform.v[14] = e.pos.z;
            let size = vec3(
                e.half.x * 2.0 * e.scale.x,
                e.half.y * 2.0 * e.scale.y,
                e.half.z * 2.0 * e.scale.z,
            );
            let mut color = e.color;
            if e.sensor && color.w >= 0.99 {
                color.w = 0.35;
            }
            self.pack_cube_instance(e.sensor, e.shape.index(), transform, size, color, e.glow);
        }
        // Settled parts of static owners.
        for p in world.parts.iter().filter(|p| !p.anim_active) {
            // Entity ids are spawn-ordered, so the list stays sorted: binary
            // search instead of a linear scan (this runs per part).
            let Some(owner) = world
                .entities
                .binary_search_by_key(&p.owner, |e| e.id)
                .ok()
                .map(|i| &world.entities[i])
                .filter(|e| e.kind == BodyKind::Static)
            else {
                continue;
            };
            let mut owner_frame = Mat4f::rotation(vec3f(0.0, owner.yaw, 0.0));
            owner_frame.v[12] = owner.pos.x;
            owner_frame.v[13] = owner.pos.y;
            owner_frame.v[14] = owner.pos.z;
            let mut local = Mat4f::rotation(p.rot);
            local.v[12] = p.offset.x * owner.scale.x;
            local.v[13] = p.offset.y * owner.scale.y;
            local.v[14] = p.offset.z * owner.scale.z;
            let transform = Mat4f::mul(&owner_frame, &local);
            let size = vec3(
                p.half.x * 2.0 * owner.scale.x,
                p.half.y * 2.0 * owner.scale.y,
                p.half.z * 2.0 * owner.scale.z,
            );
            self.pack_cube_instance(false, p.shape.index(), transform, size, p.color, p.glow);
        }
    }

    fn draw_scene(&mut self, cx: &mut Cx3d, scene_state: SceneState3D) {
        // PERF instrumentation: AIGAME_PERF=1 logs avg draw_scene CPU time and
        // instance counts every 120 frames to stderr.
        let perf_t0 = self.perf_enabled.then(std::time::Instant::now);
        self.draw_scene_inner(cx, scene_state);
        if let Some(t0) = perf_t0 {
            self.perf_accum_us += t0.elapsed().as_micros() as u64;
            self.perf_frames += 1;
            if self.perf_frames >= 120 {
                eprintln!(
                    "[aigame-perf] draw_scene avg {}us over {} frames ({} static + {} dynamic instances)",
                    self.perf_accum_us / self.perf_frames,
                    self.perf_frames,
                    self.perf_static_count,
                    self.perf_dyn_count,
                );
                self.perf_accum_us = 0;
                self.perf_frames = 0;
            }
        }
    }

    fn draw_scene_inner(&mut self, cx: &mut Cx3d, scene_state: SceneState3D) {
        let camera_pos = scene_state.camera_pos;
        self.draw_list.begin_always(cx);
        cx.begin_scene_3d(scene_state);
        let previous_world = cx.set_scene_world_transform_3d(Mat4f::identity());

        let world = self.world.clone();
        let world = world.borrow();

        // Fog only exists once the script asked for a sky.
        let (fog_color, fog_density) = match &world.sky {
            Some(sky) => (vec3(sky.horizon.x, sky.horizon.y, sky.horizon.z), sky.fog),
            None => (vec3(0.75, 0.87, 0.96), 0.0),
        };

        // 1. Sky dome around the camera (depth-tested at radius, drawn first).
        if let Some(sky) = &world.sky {
            let mut transform = Mat4f::identity();
            transform.v[12] = camera_pos.x;
            transform.v[13] = camera_pos.y;
            transform.v[14] = camera_pos.z;
            self.draw_sky.cube.transform = transform;
            self.draw_sky.cube.cube_pos = vec3(0.0, 0.0, 0.0);
            self.draw_sky.cube.cube_size = vec3(800.0, 800.0, 800.0);
            self.draw_sky.cube.color = vec4(1.0, 1.0, 1.0, 1.0);
            self.draw_sky.cube.depth_clip = 1.0;
            self.draw_sky.sky_top = vec3(sky.top.x, sky.top.y, sky.top.z);
            self.draw_sky.sky_horizon = vec3(sky.horizon.x, sky.horizon.y, sky.horizon.z);
            self.draw_sky.sky_ground = vec3(sky.ground.x, sky.ground.y, sky.ground.z);
            self.draw_sky.sky_bottom =
                vec3(sky.ground_bottom.x, sky.ground_bottom.y, sky.ground_bottom.z);
            self.draw_sky.cube.draw(cx);
        }

        // 2. The smooth terrain mesh.
        if let Some(terrain) = world.terrain.clone() {
            let geometry_id = self.ensure_terrain_geometry(cx.cx, &terrain);
            self.draw_terrain.draw_vars.geometry_id = Some(geometry_id);
            self.draw_terrain.transform = Mat4f::identity();
            self.draw_terrain.depth_clip = 1.0;
            self.draw_terrain.fog_color = fog_color;
            self.draw_terrain.fog_density = fog_density;
            if self.draw_terrain.draw_vars.can_instance() {
                let new_area = cx.add_instance(&self.draw_terrain.draw_vars);
                self.draw_terrain.draw_vars.area =
                    cx.update_area_refs(self.draw_terrain.draw_vars.area, new_area);
            }
        }

        // PERF: sections 3+4 batch per shape through many_instances. Statics
        // come from packed slabs rebuilt only when world.render_rev moves
        // (bump it — mark_render_dirty — or your static edit won't show);
        // dynamics (movers, their parts, beams, blob shadows) re-pack every
        // frame. One draw call per shape per pass; empty batches are skipped.
        self.draw_cube.fog_color = fog_color;
        self.draw_cube.fog_density = fog_density;
        self.draw_alpha.cube.fog_color = fog_color;
        self.draw_alpha.cube.fog_density = fog_density;

        let vars_ready = self.draw_cube.cube.draw_vars.can_instance()
            && self.draw_alpha.cube.cube.draw_vars.can_instance();
        if vars_ready && self.slab_rev != Some(world.render_rev) {
            self.rebuild_static_slabs(&world);
            self.slab_rev = Some(world.render_rev);
        }
        if self.perf_enabled {
            self.perf_static_count = self.slab_instance_count;
            self.perf_dyn_count = 0;
        }

        // PERF: resolve dynamic parts and shape membership ONCE per frame —
        // the per-shape loops below must not re-scan entities per part.
        let mut dyn_parts: Vec<(usize, usize)> = Vec::new();
        for (part_index, part) in world.parts.iter().enumerate() {
            let Some(owner_index) = world
                .entities
                .binary_search_by_key(&part.owner, |e| e.id)
                .ok()
            else {
                continue;
            };
            if world.entities[owner_index].kind != BodyKind::Static || part.anim_active {
                dyn_parts.push((part_index, owner_index));
            }
        }
        let mut dyn_entity_shapes = [false; 5];
        let mut dyn_sensor_shapes = [false; 5];
        for e in world.entities.iter().filter(|e| e.kind != BodyKind::Static) {
            if e.sensor {
                dyn_sensor_shapes[e.shape.index()] = true;
            } else {
                dyn_entity_shapes[e.shape.index()] = true;
            }
        }
        let mut dyn_part_shapes = [false; 5];
        for (part_index, _) in &dyn_parts {
            dyn_part_shapes[world.parts[*part_index].shape.index()] = true;
        }

        // 3. Opaque pass, one batch per shape.
        for shape in Shape::ALL {
            let shape_index = shape.index();
            let has_static = !self.static_slab[shape_index].is_empty();
            let has_dynamic_entity = dyn_entity_shapes[shape_index];
            let has_dynamic_part = dyn_part_shapes[shape_index];
            let has_beams = shape == Shape::Box && !world.beams.is_empty();
            if !has_static && !has_dynamic_entity && !has_dynamic_part && !has_beams {
                continue;
            }
            let geometry_id = self.ensure_shape_geometry(cx.cx, shape);
            self.draw_cube.cube.draw_vars.geometry_id = Some(geometry_id);
            self.draw_cube.cube.many_instances =
                cx.begin_many_instances(&self.draw_cube.cube.draw_vars);
            if has_static {
                if let Some(mi) = &mut self.draw_cube.cube.many_instances {
                    mi.instances
                        .extend_from_slice(&self.static_slab[shape_index]);
                }
            }
            // Dynamic entities: movers/kinematics/projectiles of this shape.
            for e in world
                .entities
                .iter()
                .filter(|e| !e.sensor && e.kind != BodyKind::Static && e.shape == shape)
            {
                let mut transform = Mat4f::rotation(vec3f(0.0, e.yaw, 0.0));
                transform.v[12] = e.pos.x;
                transform.v[13] = e.pos.y;
                transform.v[14] = e.pos.z;
                self.draw_cube.cube.transform = transform;
                self.draw_cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                self.draw_cube.cube.cube_size = vec3(
                    e.half.x * 2.0 * e.scale.x,
                    e.half.y * 2.0 * e.scale.y,
                    e.half.z * 2.0 * e.scale.z,
                );
                self.draw_cube.cube.color = e.color;
                self.draw_cube.cube.depth_clip = 1.0;
                self.draw_cube.glow = e.glow;
                self.draw_cube.cube.draw(cx);
                self.perf_dyn_count += 1;
            }
            // Parts that are NOT in the slab: dynamic owner, or mid-animation.
            for (part_index, owner_index) in dyn_parts.iter().copied() {
                let part = &world.parts[part_index];
                if part.shape != shape {
                    continue;
                }
                let owner = &world.entities[owner_index];
                let mut owner_frame = Mat4f::rotation(vec3f(0.0, owner.yaw, 0.0));
                owner_frame.v[12] = owner.pos.x;
                owner_frame.v[13] = owner.pos.y;
                owner_frame.v[14] = owner.pos.z;
                let mut local = Mat4f::rotation(part.rot);
                local.v[12] = part.offset.x * owner.scale.x;
                local.v[13] = part.offset.y * owner.scale.y;
                local.v[14] = part.offset.z * owner.scale.z;
                self.draw_cube.cube.transform = Mat4f::mul(&owner_frame, &local);
                self.draw_cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                self.draw_cube.cube.cube_size = vec3(
                    part.half.x * 2.0 * owner.scale.x,
                    part.half.y * 2.0 * owner.scale.y,
                    part.half.z * 2.0 * owner.scale.z,
                );
                self.draw_cube.cube.color = part.color;
                self.draw_cube.cube.depth_clip = 1.0;
                self.draw_cube.glow = part.glow;
                self.draw_cube.cube.draw(cx);
                self.perf_dyn_count += 1;
            }
            // Immediate-mode beams (box batch): a box stretched between two
            // points (grapple cables, lasers). Cable axis on local z.
            if has_beams {
                for beam in &world.beams {
                    let d = beam.to - beam.from;
                    let len = d.length();
                    if len < 1.0e-4 {
                        continue;
                    }
                    let f = d * (1.0 / len);
                    let upv = if f.y.abs() > 0.99 {
                        vec3f(1.0, 0.0, 0.0)
                    } else {
                        vec3f(0.0, 1.0, 0.0)
                    };
                    let r = Vec3f::cross(upv, f).normalize();
                    let u = Vec3f::cross(f, r);
                    let mid = beam.from + d * 0.5;
                    let mut m = Mat4f::identity();
                    m.v[0] = r.x;
                    m.v[1] = r.y;
                    m.v[2] = r.z;
                    m.v[4] = u.x;
                    m.v[5] = u.y;
                    m.v[6] = u.z;
                    m.v[8] = f.x;
                    m.v[9] = f.y;
                    m.v[10] = f.z;
                    m.v[12] = mid.x;
                    m.v[13] = mid.y;
                    m.v[14] = mid.z;
                    self.draw_cube.cube.transform = m;
                    self.draw_cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    self.draw_cube.cube.cube_size = vec3(beam.size, beam.size, len);
                    self.draw_cube.cube.color = beam.color;
                    self.draw_cube.cube.depth_clip = 1.0;
                    self.draw_cube.glow = beam.glow;
                    self.draw_cube.cube.draw(cx);
                    self.perf_dyn_count += 1;
                }
            }
            if let Some(mi) = self.draw_cube.cube.many_instances.take() {
                cx.end_many_instances(mi);
            }
        }

        // 4. Alpha pass, one batch per shape: static sensors from the slab,
        // then blob shadows (box batch) and dynamic sensors — drawn after all
        // opaque geometry so blending sees depth.
        for shape in Shape::ALL {
            let shape_index = shape.index();
            let has_static = !self.static_slab_alpha[shape_index].is_empty();
            let has_dynamic_sensor = dyn_sensor_shapes[shape_index];
            let has_shadows = shape == Shape::Box
                && world
                    .entities
                    .iter()
                    .any(|e| e.kind == BodyKind::Mover && !e.sensor && e.attached_to == 0);
            if !has_static && !has_dynamic_sensor && !has_shadows {
                continue;
            }
            let geometry_id = self.ensure_shape_geometry(cx.cx, shape);
            self.draw_alpha.cube.cube.draw_vars.geometry_id = Some(geometry_id);
            self.draw_alpha.cube.cube.many_instances =
                cx.begin_many_instances(&self.draw_alpha.cube.cube.draw_vars);
            if has_static {
                if let Some(mi) = &mut self.draw_alpha.cube.cube.many_instances {
                    mi.instances
                        .extend_from_slice(&self.static_slab_alpha[shape_index]);
                }
            }
            if has_shadows {
                for e in world.entities.iter().filter(|e| {
                    e.kind == BodyKind::Mover && !e.sensor && e.attached_to == 0
                }) {
                    // Ground under the mover: terrain, or the tallest static
                    // box top.
                    let mut ground: Option<f32> = world
                        .terrain
                        .as_ref()
                        .and_then(|t| t.floor_under(e.pos, e.half));
                    let feet = e.pos.y - e.half.y;
                    for s in world.entities.iter() {
                        if s.sensor
                            || !matches!(s.kind, BodyKind::Static | BodyKind::Kinematic)
                        {
                            continue;
                        }
                        let top = s.pos.y + s.half.y;
                        if top <= feet + 0.01
                            && (e.pos.x - s.pos.x).abs() < s.half.x
                            && (e.pos.z - s.pos.z).abs() < s.half.z
                        {
                            ground = Some(ground.map_or(top, |g: f32| g.max(top)));
                        }
                    }
                    let Some(ground) = ground else { continue };
                    let drop = feet - ground;
                    if !(0.0..8.0).contains(&drop) {
                        continue;
                    }
                    let fade = (1.0 - drop / 8.0) * 0.35;
                    let mut transform = Mat4f::identity();
                    transform.v[12] = e.pos.x;
                    transform.v[13] = ground + 0.03;
                    transform.v[14] = e.pos.z;
                    self.draw_alpha.cube.cube.transform = transform;
                    self.draw_alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                    self.draw_alpha.cube.cube.cube_size = vec3(
                        e.half.x * 2.2 * e.scale.x,
                        0.02,
                        e.half.z * 2.2 * e.scale.z,
                    );
                    self.draw_alpha.cube.cube.color = vec4(0.02, 0.02, 0.05, fade);
                    self.draw_alpha.cube.cube.depth_clip = 1.0;
                    self.draw_alpha.cube.glow = 0.0;
                    self.draw_alpha.cube.cube.draw(cx);
                    self.perf_dyn_count += 1;
                }
            }
            for e in world
                .entities
                .iter()
                .filter(|e| e.sensor && e.kind != BodyKind::Static && e.shape == shape)
            {
                let mut transform = Mat4f::rotation(vec3f(0.0, e.yaw, 0.0));
                transform.v[12] = e.pos.x;
                transform.v[13] = e.pos.y;
                transform.v[14] = e.pos.z;
                self.draw_alpha.cube.cube.transform = transform;
                self.draw_alpha.cube.cube.cube_pos = vec3(0.0, 0.0, 0.0);
                self.draw_alpha.cube.cube.cube_size = vec3(
                    e.half.x * 2.0 * e.scale.x,
                    e.half.y * 2.0 * e.scale.y,
                    e.half.z * 2.0 * e.scale.z,
                );
                let mut color = e.color;
                if color.w >= 0.99 {
                    // Sensors are see-through by default; explicit alpha wins.
                    color.w = 0.35;
                }
                self.draw_alpha.cube.cube.color = color;
                self.draw_alpha.cube.cube.depth_clip = 1.0;
                self.draw_alpha.cube.glow = e.glow;
                self.draw_alpha.cube.cube.draw(cx);
                self.perf_dyn_count += 1;
            }
            if let Some(mi) = self.draw_alpha.cube.cube.many_instances.take() {
                cx.end_many_instances(mi);
            }
        }

        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }
        cx.end_scene_3d();
        self.draw_list.end(cx);
    }
}

// ── unit shape geometries ───────────────────────────────────────────────
//
// CubeVertex POD layout (12 floats): pos3, id, normal3, pad, uv2, pad2 — what
// the DrawCube shader family samples. All shapes span [-0.5, 0.5]. The opaque
// pass backface-culls, so triangles are wound with cross(b-a, c-a) pointing
// OUTWARD — the same convention the terrain mesh uses.

fn pod_vertex(vertices: &mut Vec<f32>, p: Vec3f, n: Vec3f) {
    vertices.extend_from_slice(&[
        p.x, p.y, p.z, 0.0, n.x, n.y, n.z, 0.0, 0.0, 0.0, 0.0, 0.0,
    ]);
}

/// Flat-shaded triangle, normal from winding.
fn pod_tri(vertices: &mut Vec<f32>, indices: &mut Vec<u32>, a: Vec3f, b: Vec3f, c: Vec3f) {
    let n = Vec3f::cross(b - a, c - a).normalize();
    for p in [a, b, c] {
        indices.push((vertices.len() / 12) as u32);
        pod_vertex(vertices, p, n);
    }
}

fn shape_geometry_data(shape: Shape) -> (Vec<f32>, Vec<u32>) {
    let mut vertices: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    match shape {
        Shape::Box => {
            // Hand-rolled unit cube, matching the outward-winding rule.
            let corner = |x: i32, y: i32, z: i32| {
                vec3f(x as f32 - 0.5, y as f32 - 0.5, z as f32 - 0.5)
            };
            // (axis-aligned quads: a, b, c, d counter-clockwise seen from outside)
            let faces = [
                // +x
                [corner(1, 0, 0), corner(1, 0, 1), corner(1, 1, 1), corner(1, 1, 0)],
                // -x
                [corner(0, 0, 1), corner(0, 0, 0), corner(0, 1, 0), corner(0, 1, 1)],
                // +y
                [corner(0, 1, 0), corner(1, 1, 0), corner(1, 1, 1), corner(0, 1, 1)],
                // -y
                [corner(0, 0, 1), corner(1, 0, 1), corner(1, 0, 0), corner(0, 0, 0)],
                // +z
                [corner(1, 0, 1), corner(0, 0, 1), corner(0, 1, 1), corner(1, 1, 1)],
                // -z
                [corner(0, 0, 0), corner(1, 0, 0), corner(1, 1, 0), corner(0, 1, 0)],
            ];
            for [a, b, c, d] in faces {
                pod_tri(&mut vertices, &mut indices, a, c, b);
                pod_tri(&mut vertices, &mut indices, a, d, c);
            }
        }
        Shape::Sphere => {
            // UV sphere, smooth normals (position direction).
            const RINGS: usize = 10;
            const SEGS: usize = 16;
            let point = |r: usize, s: usize| {
                let theta = std::f32::consts::PI * r as f32 / RINGS as f32;
                let phi = std::f32::consts::TAU * s as f32 / SEGS as f32;
                vec3f(
                    0.5 * theta.sin() * phi.cos(),
                    0.5 * theta.cos(),
                    0.5 * theta.sin() * phi.sin(),
                )
            };
            let push_smooth = |vertices: &mut Vec<f32>, indices: &mut Vec<u32>, p: Vec3f| {
                indices.push((vertices.len() / 12) as u32);
                pod_vertex(vertices, p, p.normalize());
            };
            for r in 0..RINGS {
                for s in 0..SEGS {
                    let (a, b) = (point(r, s), point(r + 1, s));
                    let (c, d) = (point(r + 1, s + 1), point(r, s + 1));
                    // Wound so cross points outward (verified by the test
                    // below); pole rows drop their degenerate half-quad.
                    if r + 1 < RINGS {
                        for p in [a, c, b] {
                            push_smooth(&mut vertices, &mut indices, p);
                        }
                    }
                    if r > 0 {
                        for p in [a, d, c] {
                            push_smooth(&mut vertices, &mut indices, p);
                        }
                    }
                }
            }
        }
        Shape::Cylinder => {
            const SEGS: usize = 20;
            let rim = |y: f32, s: usize| {
                let phi = std::f32::consts::TAU * s as f32 / SEGS as f32;
                vec3f(0.5 * phi.cos(), y, 0.5 * phi.sin())
            };
            for s in 0..SEGS {
                let (a, b) = (rim(-0.5, s), rim(-0.5, s + 1));
                let (c, d) = (rim(0.5, s + 1), rim(0.5, s));
                // Side (smooth radial normals).
                let side = |vertices: &mut Vec<f32>, indices: &mut Vec<u32>, p: Vec3f| {
                    indices.push((vertices.len() / 12) as u32);
                    pod_vertex(vertices, p, vec3f(p.x, 0.0, p.z).normalize());
                };
                for p in [a, c, b] {
                    side(&mut vertices, &mut indices, p);
                }
                for p in [a, d, c] {
                    side(&mut vertices, &mut indices, p);
                }
                // Caps (flat).
                pod_tri(&mut vertices, &mut indices, vec3f(0.0, 0.5, 0.0), rim(0.5, s + 1), rim(0.5, s));
                pod_tri(&mut vertices, &mut indices, vec3f(0.0, -0.5, 0.0), rim(-0.5, s), rim(-0.5, s + 1));
            }
        }
        Shape::Cone => {
            const SEGS: usize = 20;
            let rim = |s: usize| {
                let phi = std::f32::consts::TAU * s as f32 / SEGS as f32;
                vec3f(0.5 * phi.cos(), -0.5, 0.5 * phi.sin())
            };
            let apex = vec3f(0.0, 0.5, 0.0);
            for s in 0..SEGS {
                // Side (flat per-face) + base cap.
                pod_tri(&mut vertices, &mut indices, apex, rim(s + 1), rim(s));
                pod_tri(&mut vertices, &mut indices, vec3f(0.0, -0.5, 0.0), rim(s), rim(s + 1));
            }
        }
        Shape::Wedge => {
            // A ramp: full box footprint, sloping from the top back edge
            // (+z) down to the bottom front edge (-z). Front = -z like parts.
            let p = |x: f32, y: f32, z: f32| vec3f(x, y, z);
            let (l, r, bo, t, f, ba) = (-0.5, 0.5, -0.5, 0.5, -0.5, 0.5);
            // Bottom.
            pod_tri(&mut vertices, &mut indices, p(l, bo, f), p(r, bo, f), p(r, bo, ba));
            pod_tri(&mut vertices, &mut indices, p(l, bo, f), p(r, bo, ba), p(l, bo, ba));
            // Back (+z, full height).
            pod_tri(&mut vertices, &mut indices, p(r, bo, ba), p(r, t, ba), p(l, t, ba));
            pod_tri(&mut vertices, &mut indices, p(r, bo, ba), p(l, t, ba), p(l, bo, ba));
            // Slope (front bottom edge to back top edge).
            pod_tri(&mut vertices, &mut indices, p(l, bo, f), p(r, t, ba), p(r, bo, f));
            pod_tri(&mut vertices, &mut indices, p(l, bo, f), p(l, t, ba), p(r, t, ba));
            // Side triangles.
            pod_tri(&mut vertices, &mut indices, p(l, bo, f), p(l, bo, ba), p(l, t, ba));
            pod_tri(&mut vertices, &mut indices, p(r, bo, f), p(r, t, ba), p(r, bo, ba));
        }
    }
    (vertices, indices)
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    /// Every triangle of every shape must wind so its face normal points away
    /// from the shape's interior — the opaque pass backface-culls.
    #[test]
    fn shape_windings_face_outward() {
        for shape in Shape::ALL {
            let (vertices, indices) = shape_geometry_data(shape);
            assert_eq!(indices.len() % 3, 0);
            for tri in indices.chunks_exact(3) {
                let v = |i: u32| {
                    let base = i as usize * 12;
                    vec3f(vertices[base], vertices[base + 1], vertices[base + 2])
                };
                let (a, b, c) = (v(tri[0]), v(tri[1]), v(tri[2]));
                let n = Vec3f::cross(b - a, c - a);
                let centroid = (a + b + c) * (1.0 / 3.0);
                // A strictly-interior point: outward face normals of a convex
                // solid satisfy n · (face_point - interior) > 0. The wedge is
                // not origin-centred, so it gets its own interior point.
                let interior = match shape {
                    Shape::Wedge => vec3f(0.0, -0.25, 0.25),
                    _ => vec3f(0.0, 0.0, 0.0),
                };
                if n.length() < 1.0e-6 {
                    panic!("{:?} has a degenerate triangle", shape);
                }
                let d = n.dot(centroid - interior);
                assert!(
                    d > 1.0e-6,
                    "{:?} triangle winds inward (n·(centroid-interior) = {})",
                    shape,
                    d
                );
            }
        }
    }
}

// ── the script API dispatcher ───────────────────────────────────────────
//
// Every `game.<method>(...)` in a game script lands here, synchronously.
// The vocabulary is deliberately small and grows toward GDScript-like power
// method by method — add a match arm, document it in aigame-dsl.md, done.

fn arg(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> ScriptValue {
    let trap = vm.bx.threads.cur().trap.pass();
    vm.bx.heap.vec_value(args, index, trap)
}

/// Optional positional argument: absent → NIL, and — unlike `arg` — probing
/// past the end records no error (a live trap would fail the whole eval).
fn arg_opt(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> ScriptValue {
    let v = vm.bx.heap.vec_value(args, index, NoTrap);
    if v.is_err() {
        NIL
    } else {
        v
    }
}

fn arg_f32(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> f32 {
    let v = arg(vm, args, index);
    let ip = vm.bx.threads.cur_ref().trap.ip;
    vm.bx.heap.cast_to_f64(v, ip) as f32
}

fn arg_id(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> u64 {
    arg_f32(vm, args, index) as u64
}

fn arg_string(vm: &mut ScriptVm, args: ScriptObject, index: usize) -> String {
    let v = arg(vm, args, index);
    vm.bx.heap.temp_string_with(|heap, out| {
        heap.cast_to_string(v, out);
        out.to_string()
    })
}

fn value_vec3(vm: &mut ScriptVm, v: ScriptValue) -> Vec3f {
    let ip = vm.bx.threads.cur_ref().trap.ip;
    match NumericValue::from_script_value_heap(&vm.bx.heap, v, ip) {
        NumericValue::Vec3(v) => v,
        NumericValue::F64(f) => vec3f(f as f32, f as f32, f as f32),
        _ => vec3f(0.0, 0.0, 0.0),
    }
}

fn value_color(vm: &mut ScriptVm, v: ScriptValue) -> Vec4f {
    let ip = vm.bx.threads.cur_ref().trap.ip;
    match NumericValue::from_script_value_heap(&vm.bx.heap, v, ip) {
        NumericValue::Color(c) => c,
        NumericValue::Vec4(c) => c,
        _ => vec4(0.8, 0.8, 0.8, 1.0),
    }
}

fn vec3_value(vm: &mut ScriptVm, v: Vec3f) -> ScriptValue {
    NumericValue::Vec3(v).to_script_value_heap(&mut vm.bx.heap, &vm.bx.code)
}

/// Missing option keys come back as error values, not NIL — normalize both
/// to NIL so `is_nil()` means "not provided" (a raw error would NaN every
/// numeric cast downstream).
fn opts_value(vm: &mut ScriptVm, opts: ScriptObject, key: LiveId) -> ScriptValue {
    let v = vm.bx.heap.value(opts, key.into(), NoTrap);
    if v.is_err() {
        NIL
    } else {
        v
    }
}

fn fn_ref(vm: &mut ScriptVm, v: ScriptValue) -> Option<ScriptObjectRef> {
    let obj = v.as_object()?;
    Some(vm.bx.heap.new_object_ref(obj))
}

/// Script `[...]` literals are ScriptArrays; some paths hand us vec-objects.
/// Accept either — a heights list must never silently fall back to noise.
fn list_len(vm: &ScriptVm, v: ScriptValue) -> usize {
    if let Some(a) = v.as_array() {
        vm.bx.heap.array_len(a)
    } else if let Some(o) = v.as_object() {
        vm.bx.heap.vec_len(o)
    } else {
        0
    }
}

fn list_value(vm: &mut ScriptVm, v: ScriptValue, index: usize) -> ScriptValue {
    if let Some(a) = v.as_array() {
        vm.bx.heap.array_index(a, index, NoTrap)
    } else if let Some(o) = v.as_object() {
        vm.bx.heap.vec_value(o, index, NoTrap)
    } else {
        NIL
    }
}

fn spawn_entity(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    args: ScriptObject,
    kind: BodyKind,
) -> ScriptValue {
    let opts_val = arg(vm, args, 0);
    let Some(opts) = opts_val.as_object() else {
        return NIL;
    };
    let pos_v = opts_value(vm, opts, id!(pos));
    let size_v = opts_value(vm, opts, id!(size));
    let color_v = opts_value(vm, opts, id!(color));
    let tag_v = opts_value(vm, opts, id!(tag));
    let sensor_v = opts_value(vm, opts, id!(sensor));
    let body_v = opts_value(vm, opts, id!(body));
    let gravity_v = opts_value(vm, opts, id!(gravity));
    let vel_v = opts_value(vm, opts, id!(vel));
    let life_v = opts_value(vm, opts, id!(life));
    let hits_v = opts_value(vm, opts, id!(hits));
    let glow_v = opts_value(vm, opts, id!(glow));
    let face_v = opts_value(vm, opts, id!(face));
    let turn_v = opts_value(vm, opts, id!(turn_rate));

    let pos = if pos_v.is_nil() { vec3f(0.0, 0.0, 0.0) } else { value_vec3(vm, pos_v) };
    let size = if size_v.is_nil() { vec3f(1.0, 1.0, 1.0) } else { value_vec3(vm, size_v) };
    let color = if color_v.is_nil() { vec4(0.75, 0.75, 0.8, 1.0) } else { value_color(vm, color_v) };
    let tag = if tag_v.is_nil() {
        String::new()
    } else {
        vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(tag_v, out);
            out.to_string()
        })
    };
    let sensor = sensor_v.as_bool().unwrap_or(false);
    let gravity_scale = if gravity_v.is_nil() {
        1.0
    } else {
        let ip = vm.bx.threads.cur_ref().trap.ip;
        vm.bx.heap.cast_to_f64(gravity_v, ip) as f32
    };

    // `body: "kinematic"` upgrades a box to a script-driven platform.
    let kind = if body_v.is_nil() {
        kind
    } else {
        let body = vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(body_v, out);
            out.to_string()
        });
        match body.as_str() {
            "kinematic" => BodyKind::Kinematic,
            "mover" => BodyKind::Mover,
            _ => kind,
        }
    };

    let vel = if vel_v.is_nil() { vec3f(0.0, 0.0, 0.0) } else { value_vec3(vm, vel_v) };
    let life = if life_v.is_nil() {
        0.0
    } else {
        let ip = vm.bx.threads.cur_ref().trap.ip;
        (vm.bx.heap.cast_to_f64(life_v, ip) as f32).max(0.0)
    };
    let hits = hits_v.as_bool().unwrap_or(false);
    let ip = vm.bx.threads.cur_ref().trap.ip;
    let glow = if glow_v.is_nil() { 0.0 } else { vm.bx.heap.cast_to_f64(glow_v, ip) as f32 };
    let yaw = if face_v.is_nil() { 0.0 } else { vm.bx.heap.cast_to_f64(face_v, ip) as f32 };
    let turn_rate = if turn_v.is_nil() { 7.0 } else { vm.bx.heap.cast_to_f64(turn_v, ip) as f32 };

    let shape_v = opts_value(vm, opts, id!(shape));
    let shape = if shape_v.is_nil() {
        Shape::Box
    } else {
        let name = vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(shape_v, out);
            out.to_string()
        });
        Shape::parse(&name)
    };

    let mut world = world.borrow_mut();
    world.mark_render_dirty();
    world.next_id += 1;
    let id = world.next_id;
    world.entities.push(Entity {
        id,
        kind,
        shape,
        pos,
        vel,
        half: vec3f(
            (size.x * 0.5).max(0.01),
            (size.y * 0.5).max(0.01),
            (size.z * 0.5).max(0.01),
        ),
        color,
        tag,
        sensor,
        gravity_scale,
        on_floor: false,
        floor_id: 0,
        attached_to: 0,
        attach_offset: vec3f(0.0, 0.0, 0.0),
        attach_ride: false,
        attach_spin: 0.0,
        speed_mult: 1.0,
        life,
        hits,
        hit_wall: 0,
        yaw,
        // Movers face their walk direction like every Godot actor; boxes and
        // platforms hold whatever `face:` gave them.
        auto_face: kind == BodyKind::Mover,
        turn_rate,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        glow,
    });
    ScriptValue::from_f64(id as f64)
}

/// One call builds a whole heightfield of column boxes — the corpus built
/// ~960 of these by hand in script. Heights come from a flat row-major
/// script array (index z * cells + x, world-y column tops) or, absent that,
/// from built-in terraced value noise seeded by `seed`. Colors: parallel
/// `colors` array, or `color` auto-shaded darker (low) to lighter (high).
fn spawn_terrain(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    opts: ScriptObject,
) -> ScriptValue {
    let f32_opt = |vm: &mut ScriptVm, opts: ScriptObject, key: LiveId, default: f32| -> f32 {
        let v = opts_value(vm, opts, key);
        if v.is_nil() {
            default
        } else {
            let ip = vm.bx.threads.cur_ref().trap.ip;
            vm.bx.heap.cast_to_f64(v, ip) as f32
        }
    };
    let span = f32_opt(vm, opts, id!(size), 120.0).max(2.0);
    // 384^2 vertices ≈ 147k — engine-generated, so no script instruction cost;
    // the Godot corpus runs 257x257.
    let cells = f32_opt(vm, opts, id!(cells), 24.0).clamp(2.0, 384.0) as usize;
    let base = f32_opt(vm, opts, id!(base), -12.0);
    let amp = f32_opt(vm, opts, id!(amp), 6.0);
    let seed = f32_opt(vm, opts, id!(seed), 1.0) as u64;
    // Noise shaping (engine-side so big worlds don't burn the eval budget):
    // `freq` lattice frequency per cell, `offset` raises the whole field,
    // `step` terrace size (0 = smooth), `min`/`max` clamp, and `plaza`
    // flattens a disc at the origin with a blend ramp — the corpus layout.
    let noise_freq = f32_opt(vm, opts, id!(freq), 0.18).clamp(0.005, 2.0);
    let noise_offset = f32_opt(vm, opts, id!(offset), 0.0);
    let terrace = f32_opt(vm, opts, id!(step), 1.0).max(0.0);
    let clamp_min = f32_opt(vm, opts, id!(min), f32::MIN);
    let clamp_max = f32_opt(vm, opts, id!(max), f32::MAX);
    let plaza_v = opts_value(vm, opts, id!(plaza));
    let plaza = plaza_v.as_object().map(|p| {
        (
            f32_opt(vm, p, id!(r), 20.0),
            f32_opt(vm, p, id!(ramp), 12.0).max(0.01),
            f32_opt(vm, p, id!(h), 0.0),
        )
    });
    let color_v = opts_value(vm, opts, id!(color));
    let base_color = if color_v.is_nil() {
        vec4(0.36, 0.62, 0.32, 1.0)
    } else {
        value_color(vm, color_v)
    };
    let tag_v = opts_value(vm, opts, id!(tag));
    let tag = if tag_v.is_nil() {
        "terrain".to_string()
    } else {
        vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(tag_v, out);
            out.to_string()
        })
    };

    let heights_v = opts_value(vm, opts, id!(heights));
    let colors_v = opts_value(vm, opts, id!(colors));
    let count = cells * cells;

    // Column tops: script array, or terraced value noise.
    let mut tops = Vec::with_capacity(count);
    let heights_len = list_len(vm, heights_v);
    if heights_len > 0 {
        let len = heights_len.min(count);
        let ip = vm.bx.threads.cur_ref().trap.ip;
        for index in 0..count {
            let top = if index < len {
                let v = list_value(vm, heights_v, index);
                if v.is_err() || v.is_nil() { base } else { vm.bx.heap.cast_to_f64(v, ip) as f32 }
            } else {
                base
            };
            tops.push(top);
        }
    } else {
        // Deterministic terraced value noise (xorshift lattice + bilinear).
        let lattice = |x: i64, z: i64| -> f32 {
            let mut h = seed
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add((x as u64).wrapping_mul(0x2545_F491_4F6C_DD1D))
                .wrapping_add((z as u64).wrapping_mul(0x27D4_EB2F_1656_67C5));
            h ^= h >> 33;
            h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
            h ^= h >> 33;
            (h >> 11) as f32 / (1u64 << 53) as f32
        };
        let cell_size = span / (cells - 1).max(1) as f32;
        let origin = -span * 0.5;
        for iz in 0..cells {
            for ix in 0..cells {
                let fx = ix as f32 * noise_freq;
                let fz = iz as f32 * noise_freq;
                let (x0, z0) = (fx.floor() as i64, fz.floor() as i64);
                let (tx, tz) = (fx.fract(), fz.fract());
                let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
                let (sx, sz) = (smooth(tx), smooth(tz));
                let h00 = lattice(x0, z0);
                let h10 = lattice(x0 + 1, z0);
                let h01 = lattice(x0, z0 + 1);
                let h11 = lattice(x0 + 1, z0 + 1);
                let h = h00 + (h10 - h00) * sx + (h01 - h00) * sz
                    + (h00 - h10 - h01 + h11) * sx * sz;
                let mut top = noise_offset + h * amp;
                if let Some((r, ramp, flat_h)) = plaza {
                    let wx = origin + ix as f32 * cell_size;
                    let wz = origin + iz as f32 * cell_size;
                    let d = (wx * wx + wz * wz).sqrt();
                    if d < r {
                        top = flat_h;
                    } else if d < r + ramp {
                        top = flat_h + (top - flat_h) * ((d - r) / ramp);
                    }
                }
                // Terraces: steps a mover can jump up, like the corpus.
                if terrace > 0.0 {
                    top = (top / terrace).floor() * terrace;
                }
                tops.push(top.clamp(clamp_min, clamp_max));
            }
        }
    }

    let (min_top, max_top) = tops.iter().fold((f32::MAX, f32::MIN), |(lo, hi), t| {
        (lo.min(*t), hi.max(*t))
    });
    let colors_len = list_len(vm, colors_v);

    // Height bands: `bands: [{h: 3.6, color: SAND}, ..., {h: 999, color: SNOW}]`
    // — a handful of thresholds instead of a 257x257 colors array. This is how
    // the corpus paints sand/grass/dirt/stone/snowy-mountain terrain.
    let bands_v = opts_value(vm, opts, id!(bands));
    let bands_len = list_len(vm, bands_v);
    let mut bands: Vec<(f32, Vec4f)> = Vec::with_capacity(bands_len);
    for index in 0..bands_len {
        let entry = list_value(vm, bands_v, index);
        if let Some(entry) = entry.as_object() {
            let h_v = opts_value(vm, entry, id!(h));
            let c_v = opts_value(vm, entry, id!(color));
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let h = if h_v.is_nil() { f32::MAX } else { vm.bx.heap.cast_to_f64(h_v, ip) as f32 };
            let c = if c_v.is_nil() { base_color } else { value_color(vm, c_v) };
            bands.push((h, c));
        }
    }
    bands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // Per-index colors (bands, script array, or auto-shade valleys darker).
    let mut vertex_colors = Vec::with_capacity(count);
    for index in 0..count {
        let color = if !bands.is_empty() {
            bands
                .iter()
                .find(|(h, _)| tops[index] <= *h)
                .map(|(_, c)| *c)
                .unwrap_or_else(|| bands.last().map(|(_, c)| *c).unwrap_or(base_color))
        } else if colors_len > 0 {
            if index < colors_len {
                let v = list_value(vm, colors_v, index);
                if v.is_err() || v.is_nil() { base_color } else { value_color(vm, v) }
            } else {
                base_color
            }
        } else {
            let t = if max_top > min_top { (tops[index] - min_top) / (max_top - min_top) } else { 0.5 };
            let shade = 0.75 + t * 0.4;
            vec4(
                (base_color.x * shade).min(1.0),
                (base_color.y * shade).min(1.0),
                (base_color.z * shade).min(1.0),
                base_color.w,
            )
        };
        vertex_colors.push(color);
    }

    let smooth = opts_value(vm, opts, id!(smooth)).as_bool().unwrap_or(false);
    let water_v = opts_value(vm, opts, id!(water));

    if smooth {
        // Smooth mode: the same heights array becomes VERTEX heights of one
        // triangulated ground mesh (cells = vertices per side) with height
        // lookups for collision — no column entities at all.
        let cell_size = span / (cells - 1).max(1) as f32;
        let mut world = world.borrow_mut();
        let revision = world.terrain.as_ref().map_or(1, |t| t.revision + 1);
        world.terrain = Some(Terrain {
            cells,
            cell_size,
            origin: -span * 0.5,
            heights: tops,
            colors: vertex_colors,
            revision,
        });
        if !water_v.is_nil() {
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let level = vm.bx.heap.cast_to_f64(water_v, ip) as f32;
            world.next_id += 1;
            let id = world.next_id;
            // One translucent sensor slab: gameplay touch + the water look.
            world.entities.push(Entity {
                id,
                kind: BodyKind::Static,
                pos: vec3f(0.0, level - 0.05, 0.0),
                vel: vec3f(0.0, 0.0, 0.0),
                half: vec3f(span * 0.5, 0.05, span * 0.5),
                color: vec4(0.25, 0.55, 0.85, 0.6),
                tag: "water".to_string(),
                sensor: true,
                gravity_scale: 1.0,
                on_floor: false,
                floor_id: 0,
                attached_to: 0,
                attach_offset: vec3f(0.0, 0.0, 0.0),
                attach_ride: false,
                attach_spin: 0.0,
                speed_mult: 1.0,
                life: 0.0,
                hits: false,
                hit_wall: 0,
                yaw: 0.0,
                auto_face: false,
                turn_rate: 7.0,
                scale: vec3f(1.0, 1.0, 1.0),
                scale_target: vec3f(1.0, 1.0, 1.0),
                glow: 0.0,
                shape: Shape::Box,
            });
        }
        return ScriptValue::from_f64(count as f64);
    }

    let cell_size = span / cells as f32;
    let mut spawned = 0usize;
    for iz in 0..cells {
        for ix in 0..cells {
            let index = iz * cells + ix;
            let top = tops[index].max(base + 0.05);
            let color = vertex_colors[index];
            let x = (ix as f32 + 0.5) * cell_size - span * 0.5;
            let z = (iz as f32 + 0.5) * cell_size - span * 0.5;
            let mut world = world.borrow_mut();
            world.next_id += 1;
            let id = world.next_id;
            world.entities.push(Entity {
                id,
                kind: BodyKind::Static,
                pos: vec3f(x, (base + top) * 0.5, z),
                vel: vec3f(0.0, 0.0, 0.0),
                half: vec3f(cell_size * 0.5, ((top - base) * 0.5).max(0.05), cell_size * 0.5),
                color,
                tag: tag.clone(),
                sensor: false,
                gravity_scale: 1.0,
                on_floor: false,
                floor_id: 0,
                attached_to: 0,
                attach_offset: vec3f(0.0, 0.0, 0.0),
                attach_ride: false,
                attach_spin: 0.0,
                speed_mult: 1.0,
                life: 0.0,
                hits: false,
                hit_wall: 0,
                yaw: 0.0,
                auto_face: false,
                turn_rate: 7.0,
                scale: vec3f(1.0, 1.0, 1.0),
                scale_target: vec3f(1.0, 1.0, 1.0),
                glow: 0.0,
                shape: Shape::Box,
            });
            spawned += 1;
        }
    }
    ScriptValue::from_f64(spawned as f64)
}

fn game_dispatch(
    vm: &mut ScriptVm,
    world: &Rc<RefCell<GameWorld>>,
    args: ScriptObject,
    method: LiveId,
) -> ScriptValue {
    match method {
        x if x == LiveId::from_str("box") || x == live_id!(block) => {
            spawn_entity(vm, world, args, BodyKind::Static)
        }
        x if x == live_id!(mover) => spawn_entity(vm, world, args, BodyKind::Mover),
        // A projectile-flavored mover: same options plus vel/life/hits are
        // typically set. game.spawn({pos, vel, life: 1.5, hits: true, ...}).
        x if x == live_id!(spawn) => spawn_entity(vm, world, args, BodyKind::Mover),
        x if x == live_id!(part) => {
            let owner = arg_id(vm, args, 0);
            let Some(opts) = arg_opt(vm, args, 1).as_object() else {
                return NIL;
            };
            let pos_v = opts_value(vm, opts, id!(pos));
            let size_v = opts_value(vm, opts, id!(size));
            let color_v = opts_value(vm, opts, id!(color));
            let glow_v = opts_value(vm, opts, id!(glow));
            let rx_v = opts_value(vm, opts, id!(rot_x));
            let ry_v = opts_value(vm, opts, id!(rot_y));
            let rz_v = opts_value(vm, opts, id!(rot_z));
            let offset = if pos_v.is_nil() { vec3f(0.0, 0.0, 0.0) } else { value_vec3(vm, pos_v) };
            let size = if size_v.is_nil() { vec3f(0.2, 0.2, 0.2) } else { value_vec3(vm, size_v) };
            let color = if color_v.is_nil() { vec4(0.1, 0.1, 0.12, 1.0) } else { value_color(vm, color_v) };
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let glow = if glow_v.is_nil() { 0.0 } else { vm.bx.heap.cast_to_f64(glow_v, ip) as f32 };
            let rot = vec3f(
                if rx_v.is_nil() { 0.0 } else { vm.bx.heap.cast_to_f64(rx_v, ip) as f32 },
                if ry_v.is_nil() { 0.0 } else { vm.bx.heap.cast_to_f64(ry_v, ip) as f32 },
                if rz_v.is_nil() { 0.0 } else { vm.bx.heap.cast_to_f64(rz_v, ip) as f32 },
            );
            let half = vec3f(
                (size.x * 0.5).max(0.005),
                (size.y * 0.5).max(0.005),
                (size.z * 0.5).max(0.005),
            );
            let shape_v = opts_value(vm, opts, id!(shape));
            let shape = if shape_v.is_nil() {
                Shape::Box
            } else {
                let name = vm.bx.heap.temp_string_with(|heap, out| {
                    heap.cast_to_string(shape_v, out);
                    out.to_string()
                });
                Shape::parse(&name)
            };
            let mut world = world.borrow_mut();
            if world.entity(owner).is_none() {
                return NIL;
            }
            world.mark_render_dirty();
            world.next_id += 1;
            let id = world.next_id;
            world.parts.push(Part {
                id,
                owner,
                offset,
                rot,
                half,
                target_offset: offset,
                target_rot: rot,
                target_half: half,
                rate: 9.0,
                color,
                glow,
                shape,
                anim_active: false,
            });
            ScriptValue::from_f64(id as f64)
        }
        // Animate a part: set lerp targets; the engine eases toward them at
        // `rate`/second (Godot's arm-reach used delta*9). Only given keys move.
        x if x == live_id!(move_part) => {
            let pid = arg_id(vm, args, 0);
            let Some(opts) = arg_opt(vm, args, 1).as_object() else {
                return NIL;
            };
            let pos_v = opts_value(vm, opts, id!(pos));
            let size_v = opts_value(vm, opts, id!(size));
            let rx_v = opts_value(vm, opts, id!(rot_x));
            let ry_v = opts_value(vm, opts, id!(rot_y));
            let rz_v = opts_value(vm, opts, id!(rot_z));
            let rate_v = opts_value(vm, opts, id!(rate));
            let pos = if pos_v.is_nil() { None } else { Some(value_vec3(vm, pos_v)) };
            let size = if size_v.is_nil() { None } else { Some(value_vec3(vm, size_v)) };
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let rx = if rx_v.is_nil() { None } else { Some(vm.bx.heap.cast_to_f64(rx_v, ip) as f32) };
            let ry = if ry_v.is_nil() { None } else { Some(vm.bx.heap.cast_to_f64(ry_v, ip) as f32) };
            let rz = if rz_v.is_nil() { None } else { Some(vm.bx.heap.cast_to_f64(rz_v, ip) as f32) };
            let rate = if rate_v.is_nil() { None } else { Some(vm.bx.heap.cast_to_f64(rate_v, ip) as f32) };
            let mut world = world.borrow_mut();
            if let Some(part) = world.parts.iter_mut().find(|p| p.id == pid) {
                if let Some(pos) = pos {
                    part.target_offset = pos;
                }
                if let Some(size) = size {
                    part.target_half = vec3f(
                        (size.x * 0.5).max(0.005),
                        (size.y * 0.5).max(0.005),
                        (size.z * 0.5).max(0.005),
                    );
                }
                if let Some(rx) = rx {
                    part.target_rot.x = rx;
                }
                if let Some(ry) = ry {
                    part.target_rot.y = ry;
                }
                if let Some(rz) = rz {
                    part.target_rot.z = rz;
                }
                if let Some(rate) = rate {
                    part.rate = rate.max(0.1);
                }
                part.anim_active = true;
            }
            // The part leaves the static slab while it animates.
            let owner = world.parts.iter().find(|p| p.id == pid).map(|p| p.owner);
            if let Some(owner) = owner {
                if world.is_static_visual(owner) {
                    world.mark_render_dirty();
                }
            }
            NIL
        }
        // Manual facing: sets the model yaw and takes over from auto-face —
        // vehicles pointing where they drive, the headcrab's riding spin.
        x if x == live_id!(face) => {
            let id = arg_id(vm, args, 0);
            let yaw = arg_f32(vm, args, 1);
            let mut world = world.borrow_mut();
            if world.is_static_visual(id) {
                world.mark_render_dirty();
            }
            if let Some(e) = world.entity_mut(id) {
                e.yaw = yaw;
                e.auto_face = false;
            }
            NIL
        }
        x if x == live_id!(yaw) => {
            let id = arg_id(vm, args, 0);
            let yaw = world.borrow().entity(id).map(|e| e.yaw).unwrap_or(0.0);
            ScriptValue::from_f64(yaw as f64)
        }
        // Visual model scale (physics box unchanged), eased like Godot's
        // `_model.scale.lerp(target, delta*6)` — CatNap's curl, giant bosses.
        x if x == live_id!(scale) => {
            let id = arg_id(vm, args, 0);
            let v = arg(vm, args, 1);
            let s = value_vec3(vm, v);
            let mut world = world.borrow_mut();
            if world.is_static_visual(id) {
                // Animating statics render through the dynamic path until the
                // ease settles (see step_world), so drop them from the slab.
                world.mark_render_dirty();
            }
            if let Some(e) = world.entity_mut(id) {
                e.scale_target = vec3f(s.x.max(0.01), s.y.max(0.01), s.z.max(0.01));
            }
            NIL
        }
        // Emission energy on an entity body or a part (glowing eyes ramp
        // 1.5→5 with AI state in the corpus).
        x if x == live_id!(glow) => {
            let id = arg_id(vm, args, 0);
            let energy = arg_f32(vm, args, 1).max(0.0);
            let mut world = world.borrow_mut();
            // Static entity, or a part on a static owner → slab content changed.
            let part_owner = world.parts.iter().find(|p| p.id == id).map(|p| p.owner);
            if world.is_static_visual(id)
                || part_owner.is_some_and(|o| world.is_static_visual(o))
            {
                world.mark_render_dirty();
            }
            if let Some(e) = world.entity_mut(id) {
                e.glow = energy;
            } else if let Some(p) = world.parts.iter_mut().find(|p| p.id == id) {
                p.glow = energy;
            }
            NIL
        }
        // Sky + fog. game.sky({}) = the Godot game's daylight defaults.
        x if x == live_id!(sky) => {
            let mut config = SkyConfig::default();
            if let Some(opts) = arg_opt(vm, args, 0).as_object() {
                let top_v = opts_value(vm, opts, id!(top));
                let horizon_v = opts_value(vm, opts, id!(horizon));
                let ground_v = opts_value(vm, opts, id!(ground));
                let fog_v = opts_value(vm, opts, id!(fog));
                if !top_v.is_nil() {
                    config.top = value_color(vm, top_v);
                }
                if !horizon_v.is_nil() {
                    config.horizon = value_color(vm, horizon_v);
                }
                if !ground_v.is_nil() {
                    config.ground = value_color(vm, ground_v);
                    config.ground_bottom = vec4(
                        config.ground.x * 0.45,
                        config.ground.y * 0.55,
                        config.ground.z * 0.45,
                        1.0,
                    );
                }
                if !fog_v.is_nil() {
                    let ip = vm.bx.threads.cur_ref().trap.ip;
                    config.fog = (vm.bx.heap.cast_to_f64(fog_v, ip) as f32).clamp(0.0, 0.2);
                }
            }
            {
                let mut world = world.borrow_mut();
                world.sky = Some(config);
                // Fog parameters are baked into the static instance slabs.
                world.mark_render_dirty();
            }
            NIL
        }
        x if x == live_id!(label) => {
            // game.label(id, text)          → the entity's default nametag.
            // game.label(id, text, {height, color, size}) → an EXTRA label,
            // returns a label id for game.label_text updates ("HELP!" bubbles).
            let id = arg_id(vm, args, 0);
            let text = arg_string(vm, args, 1);
            let opts_v = arg_opt(vm, args, 2);
            let mut height = f32::NAN;
            let mut color = vec4(0.0, 0.0, 0.0, 0.0);
            let mut size = 0.0f32;
            let extra = opts_v.as_object().is_some();
            if let Some(opts) = opts_v.as_object() {
                let height_v = opts_value(vm, opts, id!(height));
                let color_v = opts_value(vm, opts, id!(color));
                let size_v = opts_value(vm, opts, id!(size));
                let ip = vm.bx.threads.cur_ref().trap.ip;
                if !height_v.is_nil() {
                    height = vm.bx.heap.cast_to_f64(height_v, ip) as f32;
                }
                if !color_v.is_nil() {
                    color = value_color(vm, color_v);
                }
                if !size_v.is_nil() {
                    size = vm.bx.heap.cast_to_f64(size_v, ip) as f32;
                }
            }
            let mut world = world.borrow_mut();
            if !extra {
                // Default nametag: replace in place (empty text removes).
                world.labels.retain(|l| !(l.owner == id && l.default));
                if !text.is_empty() && world.entity(id).is_some() {
                    world.next_id += 1;
                    let lid = world.next_id;
                    world.labels.push(LabelDef {
                        lid,
                        owner: id,
                        text,
                        height,
                        color,
                        size,
                        default: true,
                    });
                }
                return NIL;
            }
            if text.is_empty() || world.entity(id).is_none() {
                return NIL;
            }
            world.next_id += 1;
            let lid = world.next_id;
            world.labels.push(LabelDef {
                lid,
                owner: id,
                text,
                height,
                color,
                size,
                default: false,
            });
            ScriptValue::from_f64(lid as f64)
        }
        x if x == live_id!(label_text) => {
            let lid = arg_id(vm, args, 0);
            let text = arg_string(vm, args, 1);
            let mut world = world.borrow_mut();
            if text.is_empty() {
                world.labels.retain(|l| l.lid != lid);
            } else if let Some(label) = world.labels.iter_mut().find(|l| l.lid == lid) {
                label.text = text;
            }
            NIL
        }
        x if x == live_id!(terrain) => {
            let Some(opts) = arg(vm, args, 0).as_object() else {
                return NIL;
            };
            spawn_terrain(vm, world, opts)
        }
        x if x == live_id!(ground_y) => {
            // Terrain height at (x, z) — place spawns/goals on engine noise.
            let x = arg_f32(vm, args, 0);
            let z = arg_f32(vm, args, 1);
            let world = world.borrow();
            match world.terrain.as_ref().and_then(|t| t.height_at(x, z)) {
                Some(h) => ScriptValue::from_f64(h as f64),
                None => NIL,
            }
        }
        x if x == live_id!(ground_peak) => {
            // Highest terrain vertex, as vec3 — where the corpus puts the goal.
            let world = world.borrow();
            let Some(t) = world.terrain.as_ref() else {
                return NIL;
            };
            let mut best = (0usize, f32::MIN);
            for (index, h) in t.heights.iter().enumerate() {
                if *h > best.1 {
                    best = (index, *h);
                }
            }
            let ix = (best.0 % t.cells) as f32;
            let iz = (best.0 / t.cells) as f32;
            let pos = vec3f(
                t.origin + ix * t.cell_size,
                best.1,
                t.origin + iz * t.cell_size,
            );
            drop(world);
            vec3_value(vm, pos)
        }
        x if x == live_id!(reset) => {
            world.borrow_mut().reset_content();
            NIL
        }
        x if x == live_id!(gravity) => {
            let g = arg_f32(vm, args, 0);
            world.borrow_mut().gravity = g;
            NIL
        }
        x if x == live_id!(on_tick) => {
            let func = arg(vm, args, 0);
            world.borrow_mut().on_tick = fn_ref(vm, func);
            NIL
        }
        x if x == live_id!(on_touch) => {
            let func = arg(vm, args, 0);
            world.borrow_mut().on_touch = fn_ref(vm, func);
            NIL
        }
        x if x == live_id!(after) => {
            let secs = arg_f32(vm, args, 0);
            let func = arg(vm, args, 1);
            let func = fn_ref(vm, func);
            let mut world = world.borrow_mut();
            let at_tick = world.tick + (secs.max(0.0) / TICK_DT) as u64;
            if let Some(func) = func {
                world.timers.push(GameTimer { at_tick, func });
            }
            NIL
        }
        x if x == live_id!(walk) => {
            let id = arg_id(vm, args, 0);
            let vx = arg_f32(vm, args, 1);
            let vz = arg_f32(vm, args, 2);
            if let Some(e) = world.borrow_mut().entity_mut(id) {
                // speed_mult is the engine-side debuff (headcrab on your head):
                // the walking script never has to know.
                e.vel.x = vx * e.speed_mult;
                e.vel.z = vz * e.speed_mult;
            }
            NIL
        }
        x if x == live_id!(speed_mult) => {
            let id = arg_id(vm, args, 0);
            let f = arg_f32(vm, args, 1);
            if let Some(e) = world.borrow_mut().entity_mut(id) {
                e.speed_mult = f.clamp(0.0, 10.0);
            }
            NIL
        }
        x if x == live_id!(jump) => {
            let id = arg_id(vm, args, 0);
            let v = arg_f32(vm, args, 1);
            if let Some(e) = world.borrow_mut().entity_mut(id) {
                e.vel.y = v;
            }
            NIL
        }
        x if x == live_id!(on_floor) => {
            let id = arg_id(vm, args, 0);
            let on = world.borrow().entity(id).map(|e| e.on_floor).unwrap_or(false);
            ScriptValue::from_bool(on)
        }
        x if x == live_id!(pos) => {
            let id = arg_id(vm, args, 0);
            let pos = world.borrow().entity(id).map(|e| e.pos).unwrap_or_default();
            vec3_value(vm, pos)
        }
        x if x == live_id!(vel) => {
            let id = arg_id(vm, args, 0);
            let vel = world.borrow().entity(id).map(|e| e.vel).unwrap_or_default();
            vec3_value(vm, vel)
        }
        x if x == live_id!(set_pos) || x == live_id!(teleport) => {
            let id = arg_id(vm, args, 0);
            let v = arg(vm, args, 1);
            let pos = value_vec3(vm, v);
            let mut world = world.borrow_mut();
            if world.is_static_visual(id) {
                world.mark_render_dirty();
            }
            if let Some(e) = world.entity_mut(id) {
                e.pos = pos;
                e.vel = vec3f(0.0, 0.0, 0.0);
            }
            NIL
        }
        x if x == live_id!(set_vel) => {
            let id = arg_id(vm, args, 0);
            let v = arg(vm, args, 1);
            let vel = value_vec3(vm, v);
            if let Some(e) = world.borrow_mut().entity_mut(id) {
                e.vel = vel;
            }
            NIL
        }
        x if x == live_id!(set_color) => {
            let id = arg_id(vm, args, 0);
            let v = arg(vm, args, 1);
            let color = value_color(vm, v);
            let mut world = world.borrow_mut();
            if world.is_static_visual(id) {
                world.mark_render_dirty();
            }
            if let Some(e) = world.entity_mut(id) {
                e.color = color;
            }
            NIL
        }
        x if x == live_id!(remove) => {
            let id = arg_id(vm, args, 0);
            let mut world = world.borrow_mut();
            if world.is_static_visual(id) {
                world.mark_render_dirty();
            }
            world.entities.retain(|e| e.id != id);
            NIL
        }
        x if x == live_id!(tag) => {
            let id = arg_id(vm, args, 0);
            let tag = world
                .borrow()
                .entity(id)
                .map(|e| e.tag.clone())
                .unwrap_or_default();
            vm.bx.heap.new_string_from_str(&tag)
        }
        x if x == live_id!(find) => {
            let tag = arg_string(vm, args, 0);
            let ids: Vec<u64> = world
                .borrow()
                .entities
                .iter()
                .filter(|e| e.tag == tag)
                .map(|e| e.id)
                .collect();
            let array = vm.bx.heap.new_array();
            let trap = vm.bx.threads.cur().trap.pass();
            for id in ids {
                vm.bx
                    .heap
                    .array_push(array, ScriptValue::from_f64(id as f64), trap);
            }
            array.into()
        }
        x if x == live_id!(distance) => {
            let a = arg_id(vm, args, 0);
            let b = arg_id(vm, args, 1);
            let world = world.borrow();
            let d = match (world.entity(a), world.entity(b)) {
                (Some(a), Some(b)) => (a.pos - b.pos).length(),
                _ => f32::MAX,
            };
            ScriptValue::from_f64(d as f64)
        }
        x if x == live_id!(held) => {
            let action = arg_string(vm, args, 0);
            let held = world.borrow().action_held(LiveId::from_str(&action));
            ScriptValue::from_bool(held)
        }
        x if x == live_id!(pressed) => {
            let action = arg_string(vm, args, 0);
            let pressed = world.borrow().action_pressed(LiveId::from_str(&action));
            ScriptValue::from_bool(pressed)
        }
        x if x == live_id!(axis) => {
            let neg = arg_string(vm, args, 0);
            let pos = arg_string(vm, args, 1);
            let world = world.borrow();
            let v = world.action_held(LiveId::from_str(&pos)) as i8 as f64
                - world.action_held(LiveId::from_str(&neg)) as i8 as f64;
            ScriptValue::from_f64(v)
        }
        x if x == live_id!(camera) => {
            let opts_val = arg(vm, args, 0);
            if let Some(opts) = opts_val.as_object() {
                let target_v = opts_value(vm, opts, id!(target));
                let distance_v = opts_value(vm, opts, id!(distance));
                let follow_v = opts_value(vm, opts, id!(follow));
                let side_v = opts_value(vm, opts, id!(side));
                let mut world = world.borrow_mut();
                if !target_v.is_nil() {
                    let target = {
                        let ip = vm.bx.threads.cur_ref().trap.ip;
                        match NumericValue::from_script_value_heap(&vm.bx.heap, target_v, ip) {
                            NumericValue::Vec3(v) => v,
                            _ => world.cam_target,
                        }
                    };
                    world.cam_target = target;
                }
                if !distance_v.is_nil() {
                    let ip = vm.bx.threads.cur_ref().trap.ip;
                    world.cam_distance = vm.bx.heap.cast_to_f64(distance_v, ip) as f32;
                }
                if !follow_v.is_nil() {
                    let ip = vm.bx.threads.cur_ref().trap.ip;
                    world.cam_follow = vm.bx.heap.cast_to_f64(follow_v, ip) as u64;
                }
                if !side_v.is_nil() {
                    world.cam_side = side_v.as_bool().unwrap_or(false);
                }
                // Third-person rig: pivot on an entity, drag orbits around it,
                // boom pulls in when geometry is in the way (Godot player cam).
                let third_v = opts_value(vm, opts, id!(third_person));
                let height_v = opts_value(vm, opts, id!(height));
                let boom_v = opts_value(vm, opts, id!(boom));
                let pitch_v = opts_value(vm, opts, id!(pitch));
                let ip = vm.bx.threads.cur_ref().trap.ip;
                if !third_v.is_nil() {
                    world.cam_third = vm.bx.heap.cast_to_f64(third_v, ip) as u64;
                }
                if !height_v.is_nil() {
                    world.cam_height = vm.bx.heap.cast_to_f64(height_v, ip) as f32;
                }
                if !boom_v.is_nil() {
                    world.cam_boom = (vm.bx.heap.cast_to_f64(boom_v, ip) as f32).max(1.0);
                }
                if !pitch_v.is_nil() {
                    world.cam_pitch_request =
                        Some((vm.bx.heap.cast_to_f64(pitch_v, ip) as f32).clamp(-1.2, 0.25));
                }
            }
            NIL
        }
        x if x == live_id!(text) => {
            // game.text(msg) → center banner (the classic form).
            // game.text(slot, msg, {color, size}) → "center" | "top" | "hint".
            let a1 = arg_opt(vm, args, 1);
            let (slot_name, text) = if a1.is_nil() {
                ("center".to_string(), arg_string(vm, args, 0))
            } else {
                (arg_string(vm, args, 0), arg_string(vm, args, 1))
            };
            let mut color = vec4(0.0, 0.0, 0.0, 0.0);
            let mut size = 0.0f32;
            if let Some(opts) = arg_opt(vm, args, 2).as_object() {
                let color_v = opts_value(vm, opts, id!(color));
                let size_v = opts_value(vm, opts, id!(size));
                if !color_v.is_nil() {
                    color = value_color(vm, color_v);
                }
                if !size_v.is_nil() {
                    let ip = vm.bx.threads.cur_ref().trap.ip;
                    size = vm.bx.heap.cast_to_f64(size_v, ip) as f32;
                }
            }
            let slot = HudSlot { text, color, size };
            let mut world = world.borrow_mut();
            match slot_name.as_str() {
                "top" => world.hud_top = slot,
                "hint" => world.hud_hint = slot,
                _ => world.hud_center = slot,
            }
            NIL
        }
        x if x == live_id!(rand) => ScriptValue::from_f64(world.borrow_mut().rand()),
        x if x == live_id!(rand_range) => {
            let a = arg_f32(vm, args, 0) as f64;
            let b = arg_f32(vm, args, 1) as f64;
            ScriptValue::from_f64(a + (b - a) * world.borrow_mut().rand())
        }
        x if x == live_id!(cam_yaw) => {
            ScriptValue::from_f64(world.borrow().cam_yaw as f64)
        }
        x if x == live_id!(attach) => {
            let rider = arg_id(vm, args, 0);
            let owner = arg_id(vm, args, 1);
            let extra = arg_opt(vm, args, 2);
            // Third arg is either the legacy vec3 offset, or an options object
            // {pos, mode: "ride", spin}. A vec3 parses as a vec3 first.
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let (offset, ride, spin) =
                match NumericValue::from_script_value_heap(&vm.bx.heap, extra, ip) {
                    NumericValue::Vec3(v) => (v, false, 0.0),
                    _ => {
                        if let Some(opts) = extra.as_object() {
                            let pos_v = opts_value(vm, opts, id!(pos));
                            let mode_v = opts_value(vm, opts, id!(mode));
                            let spin_v = opts_value(vm, opts, id!(spin));
                            let offset = if pos_v.is_nil() {
                                vec3f(0.0, 1.0, 0.0)
                            } else {
                                value_vec3(vm, pos_v)
                            };
                            let ride = if mode_v.is_nil() {
                                false
                            } else {
                                let mode = vm.bx.heap.temp_string_with(|heap, out| {
                                    heap.cast_to_string(mode_v, out);
                                    out.to_string()
                                });
                                mode == "ride"
                            };
                            let spin = if spin_v.is_nil() {
                                0.0
                            } else {
                                let ip = vm.bx.threads.cur_ref().trap.ip;
                                vm.bx.heap.cast_to_f64(spin_v, ip) as f32
                            };
                            (offset, ride, spin)
                        } else {
                            (vec3f(0.0, 1.0, 0.0), false, 0.0)
                        }
                    }
                };
            if let Some(e) = world.borrow_mut().entity_mut(rider) {
                e.attached_to = owner;
                e.attach_offset = offset;
                e.attach_ride = ride;
                e.attach_spin = spin;
                e.vel = vec3f(0.0, 0.0, 0.0);
            }
            NIL
        }
        x if x == live_id!(detach) => {
            let rider = arg_id(vm, args, 0);
            if let Some(e) = world.borrow_mut().entity_mut(rider) {
                e.attached_to = 0;
                e.attach_ride = false;
                e.attach_spin = 0.0;
            }
            NIL
        }
        x if x == live_id!(beam) => {
            let from_v = arg(vm, args, 0);
            let from = value_vec3(vm, from_v);
            let to_v = arg(vm, args, 1);
            let to = value_vec3(vm, to_v);
            let opts_v = arg_opt(vm, args, 2);
            let mut size = 0.12f32;
            let mut color = vec4(0.9, 0.9, 0.95, 1.0);
            let mut glow = 0.0f32;
            if let Some(opts) = opts_v.as_object() {
                let size_v = opts_value(vm, opts, id!(size));
                let color_v = opts_value(vm, opts, id!(color));
                let glow_v = opts_value(vm, opts, id!(glow));
                let ip = vm.bx.threads.cur_ref().trap.ip;
                if !size_v.is_nil() {
                    size = vm.bx.heap.cast_to_f64(size_v, ip) as f32;
                }
                if !color_v.is_nil() {
                    color = value_color(vm, color_v);
                }
                if !glow_v.is_nil() {
                    glow = vm.bx.heap.cast_to_f64(glow_v, ip) as f32;
                }
            }
            world.borrow_mut().beams.push(Beam {
                from,
                to,
                size: size.clamp(0.01, 4.0),
                color,
                glow,
            });
            NIL
        }
        x if x == live_id!(crosshair) => {
            let on = arg(vm, args, 0).as_bool().unwrap_or(true);
            world.borrow_mut().crosshair = on;
            NIL
        }
        x if x == live_id!(sfx) => {
            let name = arg_string(vm, args, 0);
            let pitch_v = arg_opt(vm, args, 1);
            let pitch = if pitch_v.is_nil() {
                1.0
            } else {
                let ip = vm.bx.threads.cur_ref().trap.ip;
                vm.bx.heap.cast_to_f64(pitch_v, ip) as f32
            };
            if !crate::synth::play_named(&name, pitch) {
                // An unknown name is a script bug the agent should hear about.
                world.borrow_mut().log(format!("sfx: unknown sound \"{name}\""));
            }
            NIL
        }
        x if x == live_id!(beep) => {
            let Some(opts) = arg(vm, args, 0).as_object() else {
                return NIL;
            };
            let freq_v = opts_value(vm, opts, id!(freq));
            let to_v = opts_value(vm, opts, id!(to));
            let ms_v = opts_value(vm, opts, id!(ms));
            let wave_v = opts_value(vm, opts, id!(wave));
            let gain_v = opts_value(vm, opts, id!(gain));
            let ip = vm.bx.threads.cur_ref().trap.ip;
            let freq = if freq_v.is_nil() { 440.0 } else { vm.bx.heap.cast_to_f64(freq_v, ip) as f32 };
            let to = if to_v.is_nil() { freq } else { vm.bx.heap.cast_to_f64(to_v, ip) as f32 };
            let ms = if ms_v.is_nil() { 120.0 } else { vm.bx.heap.cast_to_f64(ms_v, ip) as f32 };
            let gain = if gain_v.is_nil() { 0.25 } else { vm.bx.heap.cast_to_f64(gain_v, ip) as f32 };
            let wave = if wave_v.is_nil() {
                crate::synth::Wave::Square
            } else {
                let name = vm.bx.heap.temp_string_with(|heap, out| {
                    heap.cast_to_string(wave_v, out);
                    out.to_string()
                });
                crate::synth::Wave::parse(&name)
            };
            crate::synth::beep(freq, to, ms / 1000.0, wave, gain, 0.0);
            NIL
        }
        x if x == live_id!(jingle) => {
            let notes = arg_string(vm, args, 0);
            let ms_v = arg_opt(vm, args, 1);
            let ms = if ms_v.is_nil() {
                100.0
            } else {
                let ip = vm.bx.threads.cur_ref().trap.ip;
                vm.bx.heap.cast_to_f64(ms_v, ip) as f32
            };
            crate::synth::jingle(&notes, ms / 1000.0, crate::synth::Wave::Triangle, 0.22);
            NIL
        }
        x if x == live_id!(log) => {
            let line = arg_string(vm, args, 0);
            world.borrow_mut().log(line);
            NIL
        }
        x if x == live_id!(time) => ScriptValue::from_f64(world.borrow().time),
        _ => {
            let mut w = world.borrow_mut();
            w.log(format!("unknown game method: {:?}", method));
            NIL
        }
    }
}

// ── physics ─────────────────────────────────────────────────────────────

fn step_world(world: &mut GameWorld) {
    let gravity = world.gravity;
    let statics: Vec<Entity> = world
        .entities
        .iter()
        // Sensors report touches but never collide — the documented contract.
        .filter(|e| !e.sensor && matches!(e.kind, BodyKind::Static | BodyKind::Kinematic))
        .cloned()
        .collect();
    // Cloned like `statics` above: the mover loop holds &mut entities.
    let terrain = world.terrain.clone();
    /// A step this tall walks up for free (Godot floor snapping over the
    /// terraced 0.5 steps); anything taller is a cliff wall.
    const CLIMB: f32 = 0.55;

    // Kinematics move first (script set their velocity).
    for e in world.entities.iter_mut() {
        if e.kind == BodyKind::Kinematic {
            e.pos = e.pos + e.vel * TICK_DT;
        }
    }

    for e in world.entities.iter_mut() {
        if e.kind != BodyKind::Mover {
            continue;
        }
        // Riders are pinned to their vehicle after this loop, not simulated.
        if e.attached_to != 0 {
            continue;
        }
        // Carried by the platform we stand on.
        if e.on_floor && e.floor_id != 0 {
            if let Some(base) = statics.iter().find(|s| s.id == e.floor_id) {
                if base.kind == BodyKind::Kinematic {
                    e.pos = e.pos + base.vel * TICK_DT;
                }
            }
        }

        e.vel.y -= gravity * e.gravity_scale * TICK_DT;

        // Axis-separated sweeps: x, z, then y (so walking into a wall while
        // falling doesn't stick, and floors resolve last for on_floor).
        e.hit_wall = 0;
        let feet = e.pos.y - e.half.y;
        let (nx, hx, hx_id) = sweep_axis(&statics, e.id, e.pos, e.half, 0, e.vel.x * TICK_DT);
        e.pos.x = nx;
        if hx != 0.0 {
            e.vel.x = 0.0;
            e.hit_wall = hx_id;
        }
        // Terrain cliffs block sideways movement; steps ≤ CLIMB pass (the y
        // pass snaps the mover up onto them).
        if let Some(t) = &terrain {
            if let Some(ground) = t.floor_under(e.pos, e.half) {
                if ground > feet + CLIMB {
                    e.pos.x = nx - e.vel.x * TICK_DT;
                    e.vel.x = 0.0;
                    if e.hit_wall == 0 {
                        e.hit_wall = TERRAIN_ID;
                    }
                }
            }
        }
        let (nz, hz, hz_id) = sweep_axis(&statics, e.id, e.pos, e.half, 2, e.vel.z * TICK_DT);
        e.pos.z = nz;
        if hz != 0.0 {
            e.vel.z = 0.0;
            if e.hit_wall == 0 {
                e.hit_wall = hz_id;
            }
        }
        if let Some(t) = &terrain {
            if let Some(ground) = t.floor_under(e.pos, e.half) {
                if ground > feet + CLIMB {
                    e.pos.z = nz - e.vel.z * TICK_DT;
                    e.vel.z = 0.0;
                    if e.hit_wall == 0 {
                        e.hit_wall = TERRAIN_ID;
                    }
                }
            }
        }
        let (ny, hy, hy_id) = sweep_axis(&statics, e.id, e.pos, e.half, 1, e.vel.y * TICK_DT);
        e.pos.y = ny;
        e.on_floor = false;
        e.floor_id = 0;
        if hy != 0.0 {
            if e.vel.y < 0.0 {
                e.on_floor = true;
                e.floor_id = hy_id;
            }
            e.vel.y = 0.0;
            if e.hit_wall == 0 && e.hits {
                // A lobbed projectile landing counts as a hit too.
                e.hit_wall = hy_id;
            }
        }
        // The terrain is a floor: feet never sink below the ground surface.
        if let Some(t) = &terrain {
            if let Some(ground) = t.floor_under(e.pos, e.half) {
                let floor_y = ground + e.half.y;
                if e.pos.y <= floor_y {
                    e.pos.y = floor_y;
                    if e.vel.y <= 0.0 {
                        e.on_floor = true;
                        e.floor_id = 0;
                        if e.hit_wall == 0 && e.hits {
                            e.hit_wall = TERRAIN_ID;
                        }
                        e.vel.y = 0.0;
                    }
                }
            }
        }
    }

    // Pin riders to their owners (vehicle seats, latched headcrabs). One pass
    // after integration, same frame the owner moved — the Godot mount pattern.
    let owner_pose: Vec<(u64, Vec3f, f32)> =
        world.entities.iter().map(|e| (e.id, e.pos, e.yaw)).collect();
    for e in world.entities.iter_mut() {
        if e.attached_to == 0 {
            continue;
        }
        if let Some((_, base, owner_yaw)) =
            owner_pose.iter().find(|(id, _, _)| *id == e.attached_to)
        {
            e.pos = *base + e.attach_offset;
            e.vel = vec3f(0.0, 0.0, 0.0);
            if e.attach_ride {
                // A latched rider scrabbles: its model spins in place.
                e.yaw += e.attach_spin * TICK_DT;
            } else {
                // A seated passenger faces where the vehicle faces.
                e.yaw = *owner_yaw;
            }
        } else {
            // Owner despawned: let go rather than freezing in the air.
            e.attached_to = 0;
        }
    }

    // Visual animation: facing, model scale, part poses. Rendering-only
    // state, but stepped with physics so input tapes replay identically.
    for e in world.entities.iter_mut() {
        if e.auto_face && e.kind == BodyKind::Mover {
            let speed = (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt();
            if speed > 0.2 {
                // Godot's shared _drive(): face where you walk, turn-rate
                // clamped, fronts at -z.
                let want = (-e.vel.x).atan2(-e.vel.z);
                let mut diff = want - e.yaw;
                while diff > std::f32::consts::PI {
                    diff -= std::f32::consts::TAU;
                }
                while diff < -std::f32::consts::PI {
                    diff += std::f32::consts::TAU;
                }
                let max_turn = e.turn_rate * TICK_DT;
                e.yaw += diff.clamp(-max_turn, max_turn);
            }
        }
        let ease = (6.0 * TICK_DT).min(1.0);
        e.scale = e.scale + (e.scale_target - e.scale) * ease;
    }
    // Part easing runs only while a move_part animation is live; on arrival the
    // part snaps to its target and settles, making it slab-eligible again.
    let mut settled_owners: Vec<u64> = Vec::new();
    for part in world.parts.iter_mut() {
        if !part.anim_active {
            continue;
        }
        let ease = (part.rate * TICK_DT).min(1.0);
        part.offset = part.offset + (part.target_offset - part.offset) * ease;
        part.rot = part.rot + (part.target_rot - part.rot) * ease;
        part.half = part.half + (part.target_half - part.half) * ease;
        let remaining = (part.target_offset - part.offset).length()
            + (part.target_rot - part.rot).length()
            + (part.target_half - part.half).length();
        if remaining < 1.0e-3 {
            part.offset = part.target_offset;
            part.rot = part.target_rot;
            part.half = part.target_half;
            part.anim_active = false;
            settled_owners.push(part.owner);
        }
    }
    if !settled_owners.is_empty()
        && settled_owners.iter().any(|o| world.is_static_visual(*o))
    {
        // A static owner's decoration finished moving: it re-enters the slab.
        world.mark_render_dirty();
    }

    // Projectile lifetimes: `life` seconds, then gone.
    let mut expired = false;
    for e in world.entities.iter_mut() {
        if e.life > 0.0 {
            e.life -= TICK_DT;
            if e.life <= 0.0 {
                e.life = f32::NEG_INFINITY;
                expired = true;
            }
        }
    }
    if expired {
        world.entities.retain(|e| e.life != f32::NEG_INFINITY);
    }

    // Decoration follows its owner out (lifetime, game.remove, whatever).
    if !world.parts.is_empty() || !world.labels.is_empty() {
        let ids: HashSet<u64> = world.entities.iter().map(|e| e.id).collect();
        world.parts.retain(|p| ids.contains(&p.owner));
        world.labels.retain(|l| ids.contains(&l.owner));
    }
}

fn collect_touches(world: &GameWorld) -> Vec<(u64, u64)> {
    let mut touches = Vec::new();
    for sensor in world.entities.iter().filter(|e| e.sensor) {
        for other in world.entities.iter().filter(|e| e.kind == BodyKind::Mover) {
            if overlaps(sensor.pos, sensor.half, other.pos, other.half) {
                touches.push((sensor.id, other.id));
            }
        }
    }
    // `hits` entities (projectiles) report movers/kinematics they overlap —
    // movers pass through each other spatially, so overlap IS the hit — plus
    // whatever solid the sweep stopped them against this tick.
    for hitter in world.entities.iter().filter(|e| e.hits) {
        if hitter.hit_wall != 0 {
            touches.push((hitter.id, hitter.hit_wall));
        }
        for other in world.entities.iter() {
            if other.id == hitter.id
                || other.sensor
                || other.hits
                || !matches!(other.kind, BodyKind::Mover | BodyKind::Kinematic)
            {
                continue;
            }
            if overlaps(hitter.pos, hitter.half, other.pos, other.half) {
                touches.push((hitter.id, other.id));
            }
        }
    }
    touches
}

// ── widget plumbing ─────────────────────────────────────────────────────

/// Eval/runtime status pushed to the app so it can wake the agent — errors
/// must reach the AI that edits the game, not just wait in `.agent/` for a
/// poll. Carries every error class the isolate produces (parse, runtime,
/// pod, shader-compiler) with file:line text intact.
#[derive(Clone, Debug, Default)]
pub enum GameViewAction {
    EvalOk {
        #[allow(dead_code)]
        generation: u64,
    },
    EvalFailed {
        #[allow(dead_code)]
        generation: u64,
        #[allow(dead_code)]
        error: String,
    },
    RuntimeError {
        generation: u64,
        error: String,
    },
    #[default]
    None,
}

impl WidgetNode for GameView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for GameView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            let time = cx.seconds_since_app_start();
            let last = self.last_time.replace(time).unwrap_or(time);
            self.time_accum += (time - last).min(0.25);
            let mut ticked = false;
            while self.time_accum >= TICK_DT as f64 {
                self.time_accum -= TICK_DT as f64;
                self.run_tick(cx);
                ticked = true;
            }
            if ticked {
                self.area.redraw(cx);
            }
            self.next_frame = cx.new_next_frame();
        }

        // Keyboard -> named actions. Test runs feed the tape instead.
        if self.test_run.is_none() {
            match event {
                Event::KeyDown(ke) if !ke.is_repeat => {
                    if let Some(action) = key_to_action(ke.key_code) {
                        let mut world = self.world.borrow_mut();
                        if world.held.insert(action) {
                            world.pressed.insert(action);
                        }
                    }
                }
                Event::KeyUp(ke) => {
                    if let Some(action) = key_to_action(ke.key_code) {
                        self.world.borrow_mut().held.remove(&action);
                    }
                }
                _ => {}
            }
        }

        // Mouse orbit + wheel zoom on the pane. Raw mouse events with a rect
        // check, NOT event.hits(): the composited-pass quad doesn't take part
        // in finger capture the way plain widgets do, and this is the exact
        // pattern XrCamera's desktop orbit uses.
        match event {
            Event::MouseDown(me) if self.view_rect.contains(me.abs) && me.button.is_primary() => {
                self.orbit_last_abs = Some(me.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Event::MouseMove(me) => {
                if let Some(last) = self.orbit_last_abs {
                    let delta = me.abs - last;
                    self.orbit_yaw -= delta.x as f32 * 0.01;
                    self.orbit_pitch =
                        (self.orbit_pitch + delta.y as f32 * 0.01).clamp(-1.45, 1.45);
                    self.orbit_last_abs = Some(me.abs);
                    self.area.redraw(cx);
                } else if self.view_rect.contains(me.abs) {
                    cx.set_cursor(MouseCursor::Grab);
                }
            }
            Event::MouseUp(me) if me.button.is_primary() => {
                self.orbit_last_abs = None;
            }
            Event::Scroll(se) if self.view_rect.contains(se.abs) => {
                let scroll_axis = if se.scroll.y.abs() > f64::EPSILON {
                    se.scroll.y
                } else {
                    se.scroll.x
                };
                if scroll_axis.abs() > f64::EPSILON {
                    let factor = if scroll_axis > 0.0 { 1.0 / 0.92 } else { 0.92 };
                    let mut world = self.world.borrow_mut();
                    if world.cam_third != 0 {
                        // Third-person: the wheel zooms the boom in and out.
                        world.cam_boom = (world.cam_boom * factor as f32).clamp(2.0, 60.0);
                    } else {
                        world.cam_distance = (world.cam_distance * factor).clamp(2.0, 120.0);
                    }
                    drop(world);
                    self.area.redraw(cx);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return DrawStep::done();
        }

        self.ensure_initialized(cx.cx);
        self.view_rect = rect;
        self.pass.set_size(cx, rect.size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));

        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        if let Some(scene_state) = self.scene_state(rect, cx.time()) {
            self.set_pass_camera(cx.cx, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_scene(cx3d, scene_state);
        }
        cx.end_pass(&self.pass);

        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        cx.set_pass_area(&self.pass, self.area);

        // HUD slots. Center = the big banner, top = a second line under it,
        // hint = small control help top-left (the Godot HUD trio). Color/size
        // of 0 fall back to the slot's default look.
        {
            let (center, top, hint, crosshair) = {
                let world = self.world.borrow();
                (
                    world.hud_center.clone(),
                    world.hud_top.clone(),
                    world.hud_hint.clone(),
                    world.crosshair,
                )
            };
            let default_color = vec4(1.0, 1.0, 1.0, 0.93);
            let draw_slot = |view: &mut Self,
                                 cx: &mut Cx2d,
                                 slot: &HudSlot,
                                 default_size: f32,
                                 pos: DVec2,
                                 centered: bool| {
                if slot.text.is_empty() {
                    return;
                }
                view.draw_hud.text_style.font_size =
                    if slot.size > 0.0 { slot.size } else { default_size };
                view.draw_hud.color = if slot.color.w > 0.0 {
                    slot.color
                } else {
                    default_color
                };
                let pos = if centered {
                    let width = view
                        .draw_hud
                        .layout(cx, 0.0, 0.0, None, false, Align::default(), &slot.text)
                        .size_in_lpxs
                        .width as f64;
                    dvec2(pos.x - width * 0.5, pos.y)
                } else {
                    pos
                };
                view.draw_hud.draw_abs(cx, pos, &slot.text);
            };
            let mid_x = rect.pos.x + rect.size.x * 0.5;
            draw_slot(self, cx, &center, 22.0, dvec2(mid_x, rect.pos.y + 42.0), true);
            draw_slot(self, cx, &top, 15.0, dvec2(mid_x, rect.pos.y + 84.0), true);
            draw_slot(
                self,
                cx,
                &hint,
                9.0,
                dvec2(rect.pos.x + 12.0, rect.pos.y + 10.0),
                false,
            );
            // Restore the banner defaults for anyone else using draw_hud.
            self.draw_hud.text_style.font_size = 22.0;
            self.draw_hud.color = default_color;

            if crosshair {
                let dot = 5.0;
                self.draw_dot.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(
                            rect.pos.x + (rect.size.x - dot) * 0.5,
                            rect.pos.y + (rect.size.y - dot) * 0.5,
                        ),
                        size: dvec2(dot, dot),
                    },
                );
            }
        }

        // Billboard nametags: project each labeled entity into the pane and
        // draw in the 2D overlay — always camera-facing and never hidden by
        // geometry, like the Godot Label3D (billboard + no_depth_test).
        let labels: Vec<(Vec3f, String, Vec4f, f32)> = {
            let world = self.world.borrow();
            world
                .labels
                .iter()
                .filter_map(|label| {
                    world.entity(label.owner).map(|e| {
                        let height = if label.height.is_nan() {
                            e.half.y + 0.7
                        } else {
                            label.height
                        };
                        (
                            e.pos + vec3f(0.0, height, 0.0),
                            label.text.clone(),
                            label.color,
                            label.size,
                        )
                    })
                })
                .collect()
        };
        if !labels.is_empty() {
            if let Some(scene) = self.scene_state(rect, cx.time()) {
                for (anchor, text, color, size) in labels {
                    let clip = scene.projection.transform_vec4(
                        scene
                            .view
                            .transform_vec4(vec4(anchor.x, anchor.y, anchor.z, 1.0)),
                    );
                    if clip.w <= 0.1 {
                        continue; // behind the camera
                    }
                    let ndc_x = clip.x / clip.w;
                    let ndc_y = clip.y / clip.w;
                    if ndc_x < -1.1 || ndc_x > 1.1 || ndc_y < -1.1 || ndc_y > 1.1 {
                        continue;
                    }
                    let px = rect.pos.x + (ndc_x as f64 + 1.0) * 0.5 * rect.size.x;
                    let py = rect.pos.y + (1.0 - ndc_y as f64) * 0.5 * rect.size.y;
                    self.draw_label.text_style.font_size = if size > 0.0 { size } else { 11.0 };
                    self.draw_label.color = if color.w > 0.0 {
                        color
                    } else {
                        vec4(1.0, 1.0, 1.0, 0.87)
                    };
                    // Centre on the anchor (draw_abs is left-anchored).
                    let width = self
                        .draw_label
                        .layout(cx, 0.0, 0.0, None, false, Align::default(), &text)
                        .size_in_lpxs
                        .width as f64;
                    let at = dvec2(px - width * 0.5, py);
                    // Poor-man's outline (Godot Label3D has outline_size 24):
                    // four dark offset copies keep names readable against the
                    // bright sky.
                    let fill = self.draw_label.color;
                    self.draw_label.color = vec4(0.06, 0.07, 0.1, fill.w * 0.9);
                    for (ox, oy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
                        self.draw_label.draw_abs(cx, at + dvec2(ox, oy), &text);
                    }
                    self.draw_label.color = fill;
                    self.draw_label.draw_abs(cx, at, &text);
                }
                self.draw_label.text_style.font_size = 11.0;
                self.draw_label.color = vec4(1.0, 1.0, 1.0, 0.87);
            }
        }
        DrawStep::done()
    }
}

fn key_to_action(key_code: KeyCode) -> Option<LiveId> {
    match key_code {
        KeyCode::ArrowLeft | KeyCode::KeyA => Some(live_id!(left)),
        KeyCode::ArrowRight | KeyCode::KeyD => Some(live_id!(right)),
        KeyCode::ArrowUp | KeyCode::KeyW => Some(live_id!(up)),
        KeyCode::ArrowDown | KeyCode::KeyS => Some(live_id!(down)),
        KeyCode::Space | KeyCode::ReturnKey => Some(live_id!(jump)),
        KeyCode::KeyF => Some(live_id!(shoot)),
        KeyCode::KeyG => Some(live_id!(grab)),
        _ => None,
    }
}
