//! ArcadeView — Makepad Arcade's first real viewport (M0 stage B).
//!
//! Proves the engine runs independent of gamemaker: a GameWorld built
//! directly through the sim API (no script VM), ticked at 60Hz, rendered by
//! makepad-game-render into an offscreen pass composited into the pane.
//! Mouse drag orbits, wheel zooms — the same raw-event pattern GameView uses.

use crate::bigworld;
use makepad_game_blocks::{
    Blocks, Car, CarConfig, Character, CharacterConfig, ControlSource, Npc, NpcConfig, Poi,
    PlayerRig, PoiSet, RawInput, Seat,
};
use makepad_game_script::interact::{self, InteractAction, InteractSet};
use makepad_game_render::skin::{PoseBuffer, SkinnedModel};
use makepad_game_render::firework::FireworkSystem;
use makepad_game_render::particles::ParticleSystem;
use makepad_game_render::stage::{Stage, StageMode};
use makepad_game_render::{
    draw_billboard_labels, draw_hud_overlay, scene_state as render_scene_state, set_pass_camera,
    CameraRig, DrawGameAlpha, DrawGameCube,
    DrawGameFirework, DrawGameShadow, DrawGameSkinned, DrawGameSky, DrawGameTerrain, DrawGameTexture, GameDraws,
    GameRenderer, ModelInstance, SkinnedBatch, SkinnedDraw,
};
use makepad_game_session::{Session, SessionEvent};
use makepad_game_sim::{
    BodyKind, EmitterAnchor, Entity, GameWorld, PadState, ParticleKind, ParticleRequest,
    ParticleSpec, PlayerId, Shape, SkyConfig, SunConfig, TICK_DT,
};
use makepad_platform::event::game_input::{GameInputState, GamepadState};
use makepad_widgets::*;
use makepad_game_script::audio3d::Listener;
use makepad_game_script::{AudioRequest, ScriptHost};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ArcadeViewBase = #(ArcadeView::register_widget(vm))
    mod.widgets.ArcadeView = set_type_default() do mod.widgets.ArcadeViewBase{
        width: Fill
        height: Fill
        draw_cube +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_alpha +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_terrain +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_skinned +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        draw_models +: {
            light_dir: vec3(0.35, 0.8, 0.45)
        }
        // Firework styling lives HERE, in splash, not in Rust. The engine
        // shader owns the structure — where each spark is, how long it lives —
        // and exposes one function for the look. This overrides it by
        // inheritance, which is the whole point of the seam: a generated game
        // can restyle the sky without being able to break the simulation.
        draw_firework +: {
            // Swirl and fizzle. The engine gives a clean ballistic arc; this
            // adds the wobble that makes sparks look like burning matter
            // rather than points on a sphere.
            // Real fireworks are SYMMETRIC. Every canvas demo worth copying
            // picks one uniform radial angle per spark and then applies
            // nothing but friction and gravity — the shape comes from the
            // speed spread, not from moving the sparks around. Adding a curl
            // here made them squiggle and travel in visible waves, which is
            // the one thing a firework never does.
            //
            // The hook stays because it is the right seam for a style that
            // WANTS to be strange (a spiral shell, a jellyfish). The default
            // is zero on purpose.
            spark_motion: fn(dir: vec3, t: float, rnd: float) -> vec3 {
                return vec3(0.0, 0.0, 0.0)
            }

            // Glitter sparks bloom briefly as they strobe; the rest taper.
            spark_size: fn(life_t: float, rnd: float, speed_t: float) -> float {
                let is_glitter = step(0.80, rnd)
                let pulse = 1.0 + 0.9 * is_glitter
                    * clamp(sin(rnd * 137.0 + life_t * 190.0), 0.0, 1.0)
                return pulse * mix(0.75, 1.25, speed_t)
            }

            // A hot core with a soft halo and a faint cross-flare, which is
            // what makes a point of light read as bright rather than big.
            spark_pixel: fn(uv: vec2, tint: vec4) -> vec4 {
                // A round, antialiased dot. Each bead of a streak is one of
                // these; the streak is made by the TRAIN of them, never by
                // stretching the quad.
                //
                // `smoothstep` rather than a linear ramp is what antialiases
                // the rim — a hard cutoff shows the rasteriser's stair edge on
                // something this small, and small bright things are exactly
                // where aliasing is most visible.
                let d = length(uv - vec2(0.5, 0.5)) * 2.0
                // Both terms reach zero at d = 0.8, inside the quad's corners
                // as well as its edges, so the billboard border can never cut
                // the dot.
                let core = 1.0 - smoothstep(0.0, 0.34, d)
                let halo = 1.0 - smoothstep(0.10, 0.80, d)
                let a = clamp(core + halo * 0.55, 0.0, 1.0) * tint.w
                // Unpremultiplied colour with ZERO alpha: pure additive light
                // under premultiplied blending, so overlapping beads brighten
                // toward white and never occlude the sky.
                return vec4(tint.x * a, tint.y * a, tint.z * a, 0.0)
            }

            spark_color: fn(life_t: float, heat: float, rnd: float, style: float) -> vec4 {
                // Three-stage burn, the way a real star behaves: a white-hot
                // flash, then the shell's own metal-salt colour, then a cooling
                // ember. Colour is TEMPERATURE, not a gradient between two
                // arbitrary tints, which is what stops it looking like tinted
                // dots.
                // A DUAL-COLOUR break (style 1): half the stars carry the
                // second colour from the start rather than only cooling into
                // it, which is the two-tone shell in every display photo. The
                // split is per star and stable, so a ray keeps one colour all
                // the way out instead of shimmering between them.
                let dual = step(0.5, style) * (1.0 - step(1.5, style))
                let swap = dual * step(0.5, rnd)
                let base = mix(self.color.xyz, self.color_tail.xyz, swap)
                let ember = mix(self.color_tail.xyz, self.color.xyz, swap)
                let core = mix(base, ember, life_t * life_t)
                let hot = mix(core, vec3(1.0, 0.97, 0.90), heat * heat)

                // The fast outer shell runs hotter and holds its colour; the
                // slow core cools first. That difference is what gives a burst
                // a bright leading edge instead of a uniform ball.
                let lit = hot

                // Glitter: about a fifth of the sparks strobe hard, the rest
                // shimmer gently. Fireworks that twinkle uniformly read as
                // noise; a sparse strobe reads as crackle.
                let is_glitter = step(0.80, rnd)
                let fast = 0.5 + 0.5 * sin(rnd * 137.0 + life_t * 190.0)
                let slow = 0.80 + 0.20 * sin(rnd * 51.0 + life_t * 26.0)
                let flicker = mix(slow, fast * fast, is_glitter)

                // Flash in over the first instant, then a long quadratic decay
                // with a late lift so the tail lingers as embers.
                let rise = clamp(life_t * 26.0, 0.0, 1.0)
                let decay = (1.0 - life_t) * (1.0 - life_t)
                let alpha = rise * decay * mix(0.85, 1.25, is_glitter)

                return vec4(
                    lit.x * flicker,
                    lit.y * flicker,
                    lit.z * flicker,
                    clamp(alpha, 0.0, 1.0)
                )
            }
        }
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
    }
}

/// KeyCode isn't Hash, so held keys live in a small Vec.
type KeySet = Vec<KeyCode>;

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct ArcadeView {
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
    /// 2D overlay, drawn after the 3D pass is blitted: HUD text slots and
    /// gauges, billboard nametags, and the filled rects both lean on.
    #[live]
    draw_hud: DrawText,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_dot: DrawColor,
    #[live]
    draw_cube: DrawGameCube,
    #[live]
    draw_alpha: DrawGameAlpha,
    #[live]
    draw_sky: DrawGameSky,
    #[live]
    draw_terrain: DrawGameTerrain,
    #[live]
    draw_shadow: DrawGameShadow,
    #[live]
    draw_firework: DrawGameFirework,
    #[live]
    draw_skinned: DrawGameSkinned,
    /// Stock props share the skinned shader (both are textured packed
    /// meshes); a separate instance because the skinned batch borrows the
    /// other one for the same frame.
    #[live]
    draw_models: DrawGameSkinned,
    /// role -> the DISTINCT stock models resolved for it, most relevant first.
    ///
    /// A list rather than one id because a village of five identical houses is
    /// what a single `find()` hit gives you, and it reads as copy-paste. The
    /// index holds 21 suburban house designs; `find_many` returns as many
    /// different ones as asked for.
    #[rust]
    props: HashMap<String, Vec<String>>,
    #[rust]
    props_loaded: bool,
    /// The composed scene's model instances, built once after the props load
    /// (their footprints come from model bounds, which need the GLBs).
    #[rust]
    village: Vec<ModelInstance>,
    /// The big world's plan, when `ARCADE_WORLD=big`.
    ///
    /// Decided at world-build time — [`bigworld::plan`] is pure, so the whole
    /// layout exists before there is a GPU — and realised once the models it
    /// asked for are loaded. Holding it between the two is the only state the
    /// two-phase split needs.
    #[rust]
    world_plan: Option<bigworld::WorldPlan>,
    /// Asset id of the driveable car's mesh, once loaded.
    vehicle_model: Option<String>,
    #[live(vec4(0.03, 0.045, 0.075, 1.0))]
    clear_color: Vec4f,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    /// The rigged characters in play, each loaded once. Villagers reference
    /// one by index, so a street of twelve people of six kinds parses six
    /// meshes rather than twelve.
    #[rust]
    cast: Vec<CharacterModel>,
    /// Animation state per villager, parallel to `blocks.npcs`. The rig lives
    /// in `cast`; only the pose is per-villager.
    #[rust]
    villagers: Vec<Villager>,
    /// Engine-side blocks: the drivable car and the patrolling character.
    #[rust(Rc::new(RefCell::new(Blocks::new())))]
    blocks: Rc<RefCell<Blocks>>,
    /// Held keys for the local player (arrow keys / WASD drive the car).
    #[rust]
    keys: KeySet,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    area: Area,
    #[rust]
    renderer: GameRenderer,
    /// Device-local particles: exhaust, dust and impact sparks. Never sim
    /// state, never replicated — a peer may draw a different number.
    #[rust]
    particles: ParticleSystem,
    /// Fireworks. CPU launches shells; the GPU animates every spark from a
    /// closed form, so this costs one instance per shell and nothing per
    /// spark (libs/game/render/src/firework.rs).
    #[rust]
    fireworks: FireworkSystem,
    /// Last frame's rigid speeds, to notice a hard landing worth a spark.
    #[rust]
    impact_speed: Vec<(u64, f32)>,
    /// Shared with `script` when a game.splash is loaded, so both modes read
    /// and write one world.
    #[rust(Rc::new(RefCell::new(GameWorld::new())))]
    world: Rc<RefCell<GameWorld>>,
    /// Script-driven mode: a game.splash owns the world (game.md M4). None =
    /// the built-in Rust demo world.
    #[rust]
    script: Option<ScriptHost>,
    /// Where the loaded game lives, for the mtime watch.
    #[rust]
    game_path: Option<std::path::PathBuf>,
    #[rust]
    game_mtime: Option<std::time::SystemTime>,
    #[rust(0.0f64)]
    watch_accum: f64,
    /// Multiplayer role for this device. `ARCADE_HOST=1` hosts a room;
    /// `ARCADE_JOIN=<tcp_addr>` joins one. Unset means single-player, which is
    /// the same code path with a `Session::Local`.
    #[rust]
    session: Session,
    #[rust]
    session_status: String,
    #[rust(false)]
    world_built: bool,
    /// Sounds the Rust-side demo world asks for. Script games queue theirs in
    /// the host instead; both drain through `drain_audio`.
    #[rust]
    demo_audio: Vec<AudioRequest>,
    /// Offscreen pass targets are created once, independent of which mode
    /// supplied the world (`#[new]` textures have no format and panic on use).
    #[rust(false)]
    pass_ready: bool,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time_accum: f64,
    #[rust]
    last_time: Option<f64>,
    /// Free-orbit pose, used ONLY when no player rig exists (the bare-world
    /// debug view). With a player, the rig owns the camera and these are dead.
    #[rust(0.7f32)]
    orbit_yaw: f32,
    #[rust(-0.35f32)]
    orbit_pitch: f32,
    #[rust]
    orbit_last_abs: Option<DVec2>,
    /// The player's walker. 0 before `spawn_blocks` has run.
    #[rust]
    player: u64,
    /// Most-active gamepad this frame, or `None` when no pad is connected.
    #[rust]
    pad: Option<GamepadState>,
    /// Edge detection for the pad's buttons — `use`/`jump` are presses, not
    /// holds, and a pad reports level rather than edges.
    #[rust]
    pad_use_prev: bool,
    /// Keyboard press edges, same reason as the pad's.
    #[rust]
    jump_prev: bool,
    #[rust]
    use_prev: bool,
    #[rust]
    pad_jump_prev: bool,
    /// Which device the player last actually moved. Drives nothing but the
    /// affordance glyph — the intent itself is merged, never switched.
    #[rust]
    pad_is_active: bool,
    /// Was a pad connected last frame? Logged on change only, so "the pad does
    /// nothing" is answerable from the log without a debug build: either the
    /// app never saw the device, or it saw it and the mapping is at fault.
    #[rust]
    pad_present_prev: bool,
    /// Mouse look accumulated since the last tick, in pixels. Drained by
    /// `raw_input` so a 120Hz mouse and a 60Hz tick agree on total rotation.
    #[rust]
    look_accum: DVec2,
    /// What the primary activity button would do right now, recomputed each
    /// tick from the same call that performs it.
    #[rust]
    prompt: Option<String>,
    /// Script-declared interactables. Empty here: arcade builds its world in
    /// Rust, and cars and doors are DERIVED, so nothing needs declaring.
    #[rust]
    interact: InteractSet,
    #[rust]
    view_rect: Rect,
    #[rust]
    captured_1: bool,
    #[rust]
    captured_2: bool,
    /// Render/bake numbers are logged once, not per frame.
    #[rust]
    logged_render_stats: bool,
    #[rust]
    logged_skin_cost: bool,
    /// How this device presents the world (game.md §Presentation modes).
    /// `ARCADE_XR=mr|vr` picks a headset stage; unset stays flat. The
    /// simulation is identical in all three — only the projection differs.
    #[rust(crate::xr_input::stage_from_env())]
    stage: Stage,
    /// Last XR frame's intent, kept so button edges survive between ticks.
    #[rust]
    xr_pad: PadState,
    /// Head yaw from the last XR frame: this player's "forward".
    #[rust]
    xr_head_yaw: f32,
    /// True once an XR frame has arrived, so the flat mouse-orbit camera
    /// stops fighting the headset for the camera rig.
    #[rust(false)]
    xr_active: bool,
}

/// Entity literal with the same defaults gamemaker's spawn verb uses.
/// Sit every static on the ground under it.
///
/// Movers get this for free — `step.rs` clamps them to the terrain every tick
/// — but a static is placed once and never simulated, so on a heightfield it
/// keeps whatever y it was authored with and either floats or sinks. Run once
/// after the world is composed, so placement code stays terrain-unaware and
/// there is exactly one place that knows the rule.
///
/// The ground height is ADDED to the authored y rather than replacing it.
///
/// Replacing looks equivalent — everything is authored resting on flat ground,
/// so `y = ground + half.y` puts it back where it was — right up until
/// something is deliberately in the air. A platform authored six units up, a
/// ramp with a buried base, a sign on a post: replacing flattens all of them
/// onto the dirt, and the failure looks like the level designer's fault rather
/// than this function's.
///
/// Adding keeps both properties: on flat ground nothing moves at all (so this
/// is a no-op against everything authored before terrain existed), and on a
/// slope everything rides up with the ground it was placed on.
///
/// Anything tagged `ground` or `water` is skipped — those are world-spanning
/// slabs whose height is the whole point of them.
fn conform_statics_to_terrain(world: &mut GameWorld) {
    let Some(terrain) = world.terrain.clone() else {
        return;
    };
    for e in world.entities.iter_mut() {
        if e.kind != BodyKind::Static || e.tag == "ground" || e.tag == "water" {
            continue;
        }
        if let Some(h) = terrain.height_at(e.pos.x, e.pos.z) {
            e.pos.y += h;
        }
    }
    world.mark_render_dirty();
}

fn spawn(
    world: &mut GameWorld,
    kind: BodyKind,
    shape: Shape,
    pos: Vec3f,
    size: Vec3f,
    color: Vec4f,
    tag: &str,
) -> u64 {
    world.mark_render_dirty();
    world.next_id += 1;
    let id = world.next_id;
    // `push_entity`, never `entities.push`: M0r made it the only spawn path
    // because it asserts ascending-id order, and entity lookup is a BINARY
    // SEARCH over that invariant. Bypassing it means a future ordering
    // mistake returns the wrong entity — or silently loses a collider nothing
    // can find — instead of tripping the assert where it happened.
    world.push_entity(Entity {
        id,
        kind,
        shape,
        pos,
        vel: vec3f(0.0, 0.0, 0.0),
        half: vec3f(
            (size.x * 0.5).max(0.01),
            (size.y * 0.5).max(0.01),
            (size.z * 0.5).max(0.01),
        ),
        color,
        tag: tag.to_string(),
        sensor: false,
        collide: true,
        hidden: false,
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
        auto_face: kind == BodyKind::Mover,
        turn_rate: 7.0,
        scale: vec3f(1.0, 1.0, 1.0),
        scale_target: vec3f(1.0, 1.0, 1.0),
        glow: 0.0,
            orient: Quat::default(),
        density: 1.0,
        friction: 0.6,
        restitution: 0.0,
        // 0.0 reads as 1.0 (see Entity::push_mass): normal shove resistance.
        push_mass: 0.0,
    });
    id
}

/// What a placed prop does to something walking into it.
#[derive(Clone, Copy, PartialEq)]
enum Blocking {
    /// Scenery only — lamps, ground decals. Walk straight through.
    None,
    /// The whole footprint stops you: buildings, fences, benches, rocks.
    Solid,
    /// Only the trunk. A canopy that blocked would make a wood impassable
    /// and read as invisible walls.
    Trunk,
}

/// A collider to spawn for a placed prop. Kept separate from the visual
/// instance because a prop's silhouette and its obstruction are not the
/// same shape.
struct PropCollider {
    pos: Vec3f,
    half: Vec3f,
}

/// The stock skinned character (KayKit Knight, CC0) + its animation state.
/// Assets are fetched by apps/arcade/download_assets.sh — everything here
/// degrades gracefully when they're absent.
/// One rigged character kind, loaded once and shared by everyone playing it.
///
/// A villager holds only its POSE (each is at a different point in its own
/// walk cycle) and its build; the mesh, clips and atlas are shared, so twelve
/// people of six kinds cost six parses rather than twelve.
///
/// Clip indices are resolved BY NAME per model, never borrowed across rigs.
/// The two rigs in the village name their locomotion differently — Kenney's
/// 7-joint civilians use `idle`/`walk`, KayKit's 41-joint heroes use
/// `Idle`/`Walking_A` — so an index that means "walk" for one is a spellcast
/// or a death pose for the other.
struct CharacterModel {
    model: SkinnedModel,
    texture_png: Vec<u8>,
    idle: usize,
    walk: usize,
    /// Uploaded lazily on the first frame that has a `Cx`.
    texture: Option<Texture>,
    /// Index into the batch's texture palette, assigned at upload.
    texture_slot: usize,
    label: String,
}

/// One animated villager: the sim entity it follows, plus the pose state that
/// makes it look like a person rather than a sliding statue.
///
/// Deliberately holds NO opinion about where to walk. Motion comes from the
/// [`Npc`] block through the mover sweep; this reads the result back and turns
/// it into an animation. Facing follows travel and animation follows speed —
/// both Derived-tier, recomputed from motion, never authoritative — which is
/// why a villager that gets stuck against a fence stops walking on the spot
/// instead of moonwalking into it.
struct Villager {
    entity: u64,
    pose_idle: PoseBuffer,
    pose_walk: PoseBuffer,
    blended: PoseBuffer,
    palette: Vec<Mat4f>,
    /// Walk-cycle phase. Seeded apart per villager so a crowd doesn't step in
    /// unison, which reads as a chorus line rather than a street.
    clock: f32,
    /// 0 = idle, 1 = walking. Eased, so stopping settles rather than snaps.
    blend: f32,
    yaw: f32,
    /// Has a travel direction ever been observed? Until it has, the yaw is
    /// seeded from the entity's heading rather than left at its authored zero.
    faced: bool,
    /// Where the SWEEP put it, read back after the step — not where the walk
    /// wanted to go. The two differ exactly when something is in the way.
    pos: Vec3f,
    tint: Vec4f,
    scale: f32,
    /// Which [`CharacterModel`] this villager wears. Set when the cast is
    /// loaded; the pose buffers below size themselves to that rig's joints.
    kind: usize,
}

impl Villager {
    fn new(entity: u64, seed: u64) -> Self {
        // Cheap deterministic spread from the seed: hue and build vary per
        // villager so one rig furnishes a street. Twelve identical knights is
        // the same failure the prop variety work just fixed.
        let mut r = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut draw = || {
            r ^= r >> 12;
            r ^= r << 25;
            r ^= r >> 27;
            ((r.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / 16_777_216.0
        };
        let warm = 0.75 + draw() * 0.45;
        let cool = 0.75 + draw() * 0.45;
        let mid = 0.75 + draw() * 0.45;
        Self {
            entity,
            pose_idle: PoseBuffer::new(),
            pose_walk: PoseBuffer::new(),
            blended: PoseBuffer::new(),
            palette: Vec::new(),
            clock: draw() * 4.0,
            blend: 0.0,
            yaw: 0.0,
            faced: false,
            pos: vec3f(0.0, 0.0, 0.0),
            tint: vec4(warm, mid, cool, 1.0),
            scale: 0.92 + draw() * 0.22,
            kind: 0,
        }
    }

    /// Read this tick's motion off the entity and turn it into animation.
    fn follow(&mut self, world: &GameWorld) {
        let Some(e) = world.entity(self.entity) else {
            return;
        };
        // The mesh's origin is at the feet; the mover's is its centre.
        self.pos = vec3f(e.pos.x, e.pos.y - e.half.y, e.pos.z);
        let speed = (e.vel.x * e.vel.x + e.vel.z * e.vel.z).sqrt();
        // Face where it is actually going, not where it intended to.
        if speed > 0.15 {
            self.yaw = makepad_game_sim::math::atan2(e.vel.x, e.vel.z);
            self.faced = true;
        } else if !self.faced {
            // Never moved yet, so there is no travel direction to read. Seed
            // from the entity's own heading instead, or the character stands
            // at its authored facing — which for these rigs is straight at the
            // camera, so a third-person player spawns looking at their own
            // face until they take a step.
            //
            // The half-turn is an ASSET offset, not an engine convention:
            // these Kenney rigs are authored facing +Z while the engine's
            // forward is -Z. The engine itself applies no correction — step.rs
            // sets e.yaw in the heading convention and renderer.rs draws every
            // model at `Mat4f::rotation(vec3f(0, e.yaw, 0))` with no offset,
            // so a correctly-authored -Z model needs none of this. The
            // atan2(x, z) above happens to bake in the same half turn (atan2
            // of a heading's forward vector gives heading + PI), which is why
            // the two branches agree once moving.
            //
            // Belongs in the pack's own metadata so it travels with the art
            // that needs it, rather than being re-applied by whoever draws.
            self.yaw = e.yaw + std::f32::consts::PI;
        }
        let target = if speed > 0.35 { 1.0 } else { 0.0 };
        self.blend += (target - self.blend) * 0.12;
        // Advance the walk cycle with distance covered, so a villager blocked
        // by a bench stops its legs instead of running in place.
        self.clock += TICK_DT * (0.9 + speed * 0.28);
    }
}

impl CharacterModel {
    /// Load one character from a GLB plus the atlas its materials reference.
    ///
    /// `skin.rs` deliberately ignores glTF materials — the caller binds the
    /// texture — so the caller is the one that has to bind the RIGHT one. The
    /// two packs differ: KayKit embeds its atlas in the GLB and ships a
    /// sidecar PNG beside it, while Kenney's characters reference a shared
    /// `Textures/colormap.png` that every model in the pack samples. Both
    /// arrive here as bytes and the distinction stops mattering.
    fn load(glb_path: &str, texture_path: &str, label: &str) -> Option<CharacterModel> {
        let glb = std::fs::read(glb_path).ok()?;
        let texture_png = std::fs::read(texture_path).ok()?;
        let model = match SkinnedModel::parse_glb(&glb) {
            Ok(model) => model,
            Err(err) => {
                log!("arcade: {label} failed to parse: {err}");
                return None;
            }
        };
        // BY NAME, per rig. Kenney: idle/walk. KayKit: Idle/Walking_A.
        // clip_index is case-insensitive, so one ordered list covers both;
        // borrowing an index from another rig would animate a spellcast.
        let idle = ["idle", "unarmed_idle", "static"]
            .iter()
            .find_map(|n| model.clip_index(n))?;
        let walk = ["walk", "walking_a", "walking_b", "run", "running_a"]
            .iter()
            .find_map(|n| model.clip_index(n))?;
        Some(CharacterModel {
            texture_png,
            idle,
            walk,
            texture: None,
            texture_slot: 0,
            label: label.to_string(),
            model,
        })
    }

    /// Normalise the two rigs to the 1.8-unit mover they inhabit.
    ///
    /// KayKit's heroes are modelled at roughly human height; Kenney's "mini"
    /// civilians are about a unit tall, so drawn raw they read as children
    /// beside their own front doors. Keyed off joint count rather than
    /// measured bounds because `SkinnedModel` exposes no rest-pose extents —
    /// crude, but the two rigs in play are far enough apart to be
    /// unambiguous, and a third rig would want real bounds instead.
    fn height_scale(&self) -> f32 {
        if self.model.joint_count() >= 41 {
            1.0
        } else {
            1.7
        }
    }

    /// The village cast, chosen THROUGH THE INDEX rather than by hardcoded
    /// paths, so this exercises the path a generated game takes.
    ///
    /// Townsfolk come from the 7-joint Kenney civilian rig — a village wants
    /// people, not nine fantasy heroes — with the KayKit knight kept as a
    /// single standout because he is the one the player already knows.
    fn load_cast() -> Vec<CharacterModel> {
        let models_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/models");
        let chars_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/characters");
        let mut cast = Vec::new();

        {
            let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/resources"));
            let idx = makepad_game_assets::AssetIndex::build(&root);
            // Take the rig with the MOST members — asking by that property
            // rather than by joint count means a library that later grows a
            // better-populated rig is picked up without editing this.
            let civilians = idx
                .casts()
                .into_iter()
                .max_by_key(|c| c.members.len())
                .map(|c| c.members)
                .unwrap_or_default()
                .into_iter()
                .filter(|id| id.contains("mini-characters"))
                .take(8)
                .collect::<Vec<_>>();
            for id in civilians {
                // id is `kenney/<pack>/<stem>`; the atlas is the pack's own.
                let mut parts = id.splitn(3, '/');
                let (_src, pack, stem) = match (parts.next(), parts.next(), parts.next()) {
                    (Some(a), Some(b), Some(c)) => (a, b, c),
                    _ => continue,
                };
                let glb = format!("{models_dir}/kenney/{pack}/{stem}.glb");
                let tex = format!("{models_dir}/kenney/{pack}/Textures/colormap.png");
                if let Some(c) = CharacterModel::load(&glb, &tex, stem) {
                    cast.push(c);
                }
            }
        }

        // Two heroes among the townsfolk, on the other rig entirely — which
        // is what makes the multi-rig path real rather than theoretical. The
        // barbarian is the one the player wears (see `PLAYER_CHARACTER`).
        for (stem, tex) in [
            ("barbarian", "barbarian_texture.png"),
            ("knight", "knight_texture.png"),
        ] {
            if let Some(c) = CharacterModel::load(
                &format!("{chars_dir}/{stem}.glb"),
                &format!("{chars_dir}/{tex}"),
                stem,
            ) {
                cast.push(c);
            }
        }

        if cast.is_empty() {
            log!("arcade: no skinned characters — run apps/arcade/download_assets.sh");
        } else {
            for c in &cast {
                log!(
                    "arcade: character {} — {} joints, {} verts, {} clips",
                    c.label,
                    c.model.joint_count(),
                    c.model.vertex_count(),
                    c.model.clips.len()
                );
            }
        }
        cast
    }
}

impl ArcadeView {
    /// Take the frame's queued sounds together with the listener this device
    /// hears from. Local tier by construction: the listener is *this*
    /// camera, so two players in a room hear the same game differently and
    /// none of it reaches the wire.
    pub fn drain_audio(&mut self) -> (Vec<AudioRequest>, Listener) {
        let mut requests = std::mem::take(&mut self.demo_audio);
        if let Some(host) = &self.script {
            requests.extend(host.take_audio());
        }
        let world = self.world.borrow();
        let listener = Listener::from_yaw(world.cam_target, world.cam_yaw);
        (requests, listener)
    }

    /// Switch to script-driven mode: a `game.splash` owns the world.
    /// Returns the eval error, if the first eval failed.
    pub fn load_game(&mut self, cx: &mut Cx, path: &std::path::Path) -> Option<String> {
        self.load_game_with_trust(cx, path, makepad_game_script::Trust::Local)
    }

    /// As `load_game`, but for a game that arrived from a registry or a peer:
    /// its isolate is capability-stripped (no fs, no process spawn, no net).
    pub fn load_game_with_trust(
        &mut self,
        cx: &mut Cx,
        path: &std::path::Path,
        trust: makepad_game_script::Trust,
    ) -> Option<String> {
        let source = std::fs::read_to_string(path).ok()?;
        let mut host = if trust.is_sandboxed() {
            ScriptHost::new_sandboxed()
        } else {
            ScriptHost::new()
        };
        // Share the host's world/blocks so render and input keep working
        // through exactly the same fields as the demo path.
        self.world = host.world.clone();
        self.blocks = host.blocks.clone();
        let report = {
            let r = host.set_source(cx, &source);
            r.map(|r| (r.ok, r.error, r.entities))
        };
        self.game_path = Some(path.to_path_buf());
        self.game_mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        self.script = Some(host);
        self.world_built = true;
        match report {
            Some((false, error, _)) => error,
            Some((true, _, entities)) => {
                log!("arcade: eval ok, {entities} entities");
                None
            }
            _ => None,
        }
    }

    /// Poll the game file; a changed mtime re-evals with last-good rollback.
    /// Returns the new error text when an edit fails to compile.
    fn watch_game_file(&mut self, cx: &mut Cx, dt: f64) -> Option<String> {
        const WATCH_PERIOD: f64 = 0.25;
        self.watch_accum += dt;
        if self.watch_accum < WATCH_PERIOD {
            return None;
        }
        self.watch_accum = 0.0;
        let path = self.game_path.clone()?;
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if mtime == self.game_mtime {
            return None;
        }
        self.game_mtime = mtime;
        let source = std::fs::read_to_string(&path).ok()?;
        let host = self.script.as_mut()?;
        // A failed eval rolls the world back inside the host, so the player
        // keeps the last world that worked.
        host.set_source(cx, &source).and_then(|r| r.error)
    }

    /// `ARCADE_WORLD=big` swaps the village street for the whole map.
    ///
    /// An env switch rather than a replacement because the street is still the
    /// useful demo for anything close-range — collision, the physics yard, the
    /// crowd — and a world that takes seconds to plan is the wrong thing to
    /// pay for when checking whether a bench blocks.
    fn big_world_requested() -> bool {
        std::env::var("ARCADE_WORLD").map(|v| v == "big").unwrap_or(false)
    }

    /// The Zelda-scale world: six regions, each from one art pack, connected
    /// by generated roads.
    ///
    /// Only the parts that need a world are done here — ground, inhabitants,
    /// camera. The props themselves cannot be placed until their models are
    /// loaded (a prop is scaled from its own bounds), so the plan is held and
    /// realised in the first frame that has a `Cx`, exactly as the street's
    /// props are.
    fn build_big_world(&mut self) {
        let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/resources"));
        let index = makepad_game_assets::AssetIndex::build(&root);
        const SEED: u64 = 0xB16_0F0A;
        let plan = bigworld::plan(&index, SEED);
        log!(
            "arcade: world planned in {} ms — {} props ({} distinct models), {} tiles, {} npcs, \
             {} pois, {} interactables",
            plan.stats.gen_us / 1000,
            plan.stats.props,
            plan.stats.distinct_models,
            plan.stats.tiles,
            plan.stats.npcs,
            plan.pois.len(),
            plan.interactables.len(),
        );
        for (region, n) in &plan.stats.per_region {
            log!("arcade:   {:<8} {} props", region.name(), n);
        }

        let mut world = self.world.borrow_mut();
        let w = &mut *world;
        w.reset_content();
        w.sky = Some(SkyConfig {
            horizon: vec4(0.66, 0.76, 0.80, 1.0),
            // Thinner than the street's: the castle is 100 units from the
            // village and has to stay readable as a landmark, which is the
            // whole reason it is where it is.
            fog: 0.0009,
            ..SkyConfig::default()
        });

        // ARCADE_VIEW picks among the plan's own viewpoints: 0 is the
        // establishing shot down the road toward the castle, then one per
        // region. The plan knows where its regions are, so the camera does not
        // have to be told twice.
        let vp = std::env::var("ARCADE_VIEW")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        if let Some((eye, target)) = plan.viewpoints.get(vp).copied() {
            let d = eye - target;
            w.cam_target = target;
            w.cam_distance = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt().max(6.0);
            w.orbit_yaw = d.x.atan2(d.z);
            w.orbit_pitch = -(d.y / w.cam_distance).clamp(-0.999, 0.999).asin();
        }

        // Ground: sized to the map rather than the street. The regions sit
        // inside ±110, so 260 across leaves the horizon beyond the far woods
        // instead of cutting them off mid-scatter.
        spawn(
            w,
            BodyKind::Static,
            Shape::Box,
            vec3f(0.0, -0.5, 0.0),
            vec3f(260.0, 1.0, 260.0),
            vec4(0.46, 0.56, 0.36, 1.0),
            "ground",
        );

        // Inhabitants. Same Movers as the street's villagers — hidden boxes
        // whose appearance is a skinned mesh — so they collide with the props
        // and each other exactly as the player does.
        for (i, npc) in plan.npcs.iter().enumerate() {
            let id = spawn(
                w,
                BodyKind::Mover,
                Shape::Box,
                vec3f(npc.pos.x, 0.9, npc.pos.z),
                vec3f(0.7, 1.8, 0.7),
                vec4(0.85, 0.85, 0.9, 1.0),
                "villager",
            );
            if let Some(e) = w.entity_mut(id) {
                e.hidden = true;
                // Zero under Entity::default(): a villager with gravity_scale
                // 0 hangs in the air, and with speed_mult 0 never moves however
                // hard the brain pushes.
                e.gravity_scale = 1.0;
                e.speed_mult = 1.0;
            }
            self.villagers.push(Villager::new(id, 0xB16_5EED ^ i as u64));
        }

        // The car, at the village square: a world you can only walk is a
        // diorama, and the roads exist to be driven.
        w.next_id += 1;
        let car = w.next_id;
        w.push_entity(Entity {
            id: car,
            kind: BodyKind::Rigid,
            pos: vec3f(plan.player_start.x + 6.0, 1.2, plan.player_start.z + 4.0),
            half: vec3f(0.9, 0.4, 1.6),
            color: vec4(0.86, 0.32, 0.28, 1.0),
            tag: "car".to_string(),
            collide: true,
            gravity_scale: 1.0,
            speed_mult: 1.0,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            density: 1.0,
            friction: 0.7,
            ..Default::default()
        });
        w.mark_render_dirty();
        drop(world);

        self.blocks.borrow_mut().cars.push(Car::new(
            car,
            CarConfig::default(),
            ControlSource::Player,
        ));

        self.cast = CharacterModel::load_cast();
        let kinds = self.cast.len().max(1);
        for (i, v) in self.villagers.iter_mut().enumerate() {
            v.kind = i % kinds;
        }
        self.world_plan = Some(plan);
        self.world_built = true;
    }

    fn build_world(&mut self) {
        if Self::big_world_requested() {
            self.build_big_world();
            return;
        }
        let mut world = self.world.borrow_mut();
        let w = &mut *world;
        w.reset_content();
        // Fog is `1 - exp(-dist * density)`, so the default 0.004 puts the far
        // treeline (~70 units out at this camera) 24% of the way to the horizon
        // colour — and that colour is a very pale blue-white, so a saturated
        // green pine arrives grey. Three separate reviews called the scene
        // "washed out"; this is the whole of it.
        //
        // 0.0015 leaves ~10% at the same distance: enough that depth still
        // reads, not enough to launder the colour out of anything. The horizon
        // also comes down a little and warms toward the sun, so what haze
        // remains looks like air rather than a grey veil. Set on the demo
        // rather than in SkyConfig::default(), which gamemaker and every
        // existing game also read.
        w.sky = Some(SkyConfig {
            horizon: vec4(0.66, 0.76, 0.80, 1.0),
            fog: 0.0015,
            ..SkyConfig::default()
        });
        // Framed on the street rather than on the origin: the road runs
        // east-west through z=0, so looking slightly north of it puts the
        // houses in frame and keeps the empty foreground off the bottom edge.
        // Pulled in and swung toward the yard: at 56 units the street sat in
        // the top third with a third of the frame given to bare lawn, which
        // reads as an unfinished map. Closer, and centred between the houses
        // and the yard, both ends of the village earn their space.
        w.cam_target = vec3f(-8.0, 1.0, 6.0);
        w.cam_distance = 44.0;
        w.orbit_yaw = 0.62;
        w.orbit_pitch = -0.34;

        // Sun: set ONCE, as an explicit direction rather than a time of day.
        //
        // 38° elevation is the whole choice. The engine default sits at 54°,
        // which is nearly overhead — objects get a bright top and shadows
        // barely clear their own footprint, so nothing reads as standing on
        // anything. At 38° a shadow runs about 1.3x the caster's height:
        // long enough to describe the shape that cast it and to show the
        // ground's slope, short enough that the village does not disappear
        // into its own shade.
        //
        // Given as `dir` because the solar model's answer depends on
        // latitude and season, and this is a look, not a date.
        w.sun = SunConfig {
            dir: Some(vec3f(0.55, 0.62, 0.56).normalize()),
            ..Default::default()
        };

        // The ground is a heightfield, not a slab.
        //
        // The village itself sits on a plaza — genuinely flat, because the
        // road is one long box and the houses are boxes with square feet, and
        // none of those can follow a slope. Past the plaza the ramp lets go
        // and `rim_relief` grows the landscape outward, so the horizon is
        // hills rather than the edge of a green table. That combination is
        // the point: a playfield that still works and a view worth turning
        // the camera for.
        //
        // Movers need no help — `step.rs` already clamps them to the terrain,
        // so villagers, the player and the car find the ground themselves.
        // Statics do not, which is what `conform_statics_to_terrain` is for.
        let field = makepad_game_gen::terrain::generate(&makepad_game_gen::terrain::TerrainParams {
            seed: 0x5747_4E56,
            span: 260.0,
            cells: 261,
            base: 0.0,
            amp: 5.0,
            feature_size: 90.0,
            flatten: 1.25,
            rim_relief: 5.0,
            rim_start: 0.34,
            plaza: Some(makepad_game_gen::terrain::TerrainPlaza {
                center_x: 0.0,
                center_z: 0.0,
                // Covers the road (±58) and the houses either side of it.
                radius: 66.0,
                ramp: 26.0,
                height: 0.0,
            }),
            ..Default::default()
        });
        w.terrain = Some(makepad_game_sim::Terrain {
            cells: field.cells,
            cell_size: field.cell_size,
            origin: field.origin,
            heights: field.heights,
            colors: field.colors,
            revision: 1,
        });
        // The road: a strip through the middle of the green. Flat enough to
        // drive over, and `collide: false` so it is a surface rather than a
        // kerb the car has to climb.
        let road = spawn(
            w,
            BodyKind::Static,
            Shape::Box,
            vec3f(0.0, 0.02, 0.0),
            vec3f(58.0, 0.05, 7.0),
            vec4(0.30, 0.30, 0.32, 1.0),
            "road",
        );
        if let Some(e) = w.entity_mut(road) {
            e.collide = false;
        }
        // A crossroads. One straight road reads as a corridor you drive along;
        // a junction is the smallest thing that makes a place feel like it has
        // somewhere else to be. Same flat slab treatment — `collide: false`,
        // so it is a surface rather than a kerb.
        let cross = spawn(
            w,
            BodyKind::Static,
            Shape::Box,
            vec3f(0.0, 0.02, 0.0),
            vec3f(7.0, 0.05, 52.0),
            vec4(0.30, 0.30, 0.32, 1.0),
            "road",
        );
        if let Some(e) = w.entity_mut(cross) {
            e.collide = false;
        }
        // A side street out to the yard, so the physics corner is somewhere
        // you drive TO rather than somewhere that happens to be nearby.
        let side = spawn(
            w,
            BodyKind::Static,
            Shape::Box,
            vec3f(-20.0, 0.02, 11.0),
            vec3f(4.0, 0.05, 11.0),
            vec4(0.32, 0.32, 0.34, 1.0),
            "road",
        );
        if let Some(e) = w.entity_mut(side) {
            e.collide = false;
        }

        // A short verge path from the road up to the middle house, so the
        // houses read as connected to the street rather than parked near it.
        let path = spawn(
            w,
            BodyKind::Static,
            Shape::Box,
            vec3f(0.5, 0.03, -6.0),
            vec3f(2.0, 0.05, 6.0),
            vec4(0.62, 0.58, 0.48, 1.0),
            "path",
        );
        if let Some(e) = w.entity_mut(path) {
            e.collide = false;
        }

        // --- the yard -------------------------------------------------
        // The physics demo lives in one corner of the green, deliberately
        // placed as a builder's yard rather than sprinkled across the map.
        const YARD_X: f32 = -20.0;
        const YARD_Z: f32 = 20.0;
        // A ramp to drive up, at the yard entrance.
        spawn(
            w,
            BodyKind::Static,
            Shape::Wedge,
            vec3f(YARD_X + 12.0, 1.0, YARD_Z - 2.0),
            vec3f(6.0, 2.0, 7.0),
            vec4(0.72, 0.60, 0.42, 1.0),
            "ramp",
        );
        // Moving platform (kinematic, driven every tick below).
        spawn(
            w,
            BodyKind::Kinematic,
            Shape::Box,
            vec3f(YARD_X + 4.0, 2.5, YARD_Z + 4.0),
            vec3f(6.0, 0.6, 3.0),
            vec4(0.35, 0.6, 0.8, 1.0),
            "platform",
        );
        // --- the jump course ------------------------------------------
        // A route you can actually complete: step up, cross a platform that
        // slides, ride one that rises, land on the far side. Laid out so the
        // ramp's high edge is a natural run-up, which is what turns two
        // separate toys into one thing to do.
        //
        // Gaps are sized against the character's jump, not eyeballed: the
        // controller clears roughly 4 units of horizontal distance, so 4.5-unit
        // spacing needs a committed jump without being a coin flip.
        let course: [(&str, Vec3f, Vec3f, Vec4f); 4] = [
            // A low static step to start from — somewhere to stand and look
            // at the rest of the course before committing.
            ("step", vec3f(-24.0, 1.2, 26.0), vec3f(2.2, 0.4, 2.2), vec4(0.55, 0.52, 0.46, 1.0)),
            // Slides side to side across your path.
            ("platform_x", vec3f(-28.5, 2.6, 30.0), vec3f(2.6, 0.4, 2.6), vec4(0.35, 0.60, 0.80, 1.0)),
            // Rises and falls: an elevator you have to time.
            ("platform_y", vec3f(-33.0, 4.2, 34.0), vec3f(2.6, 0.4, 2.6), vec4(0.42, 0.70, 0.52, 1.0)),
            // The payoff — a wide, still ledge that is obviously the end.
            ("goal", vec3f(-38.0, 6.0, 38.0), vec3f(3.2, 0.5, 3.2), vec4(0.80, 0.66, 0.32, 1.0)),
        ];
        for (tag, pos, half, color) in course {
            let kind = if tag.starts_with("platform") {
                BodyKind::Kinematic
            } else {
                BodyKind::Static
            };
            spawn(w, kind, Shape::Box, pos, half, color, tag);
        }

        // Falling movers: land, rest, cast blob shadows.
        for i in 0..5 {
            spawn(
                w,
                BodyKind::Mover,
                if i % 2 == 0 { Shape::Box } else { Shape::Sphere },
                vec3f(
                    YARD_X - 2.0 + i as f32 * 2.2,
                    6.0 + i as f32 * 2.0,
                    YARD_Z + 1.0 + (i % 3) as f32 * 1.6,
                ),
                vec3f(1.2, 1.2, 1.2),
                vec4(0.85, 0.35 + 0.1 * i as f32, 0.35, 1.0),
                "crate",
            );
        }
        // The bouncer: a mover the tick loop re-launches on every landing.
        spawn(
            w,
            BodyKind::Mover,
            Shape::Sphere,
            vec3f(YARD_X + 7.0, 4.0, YARD_Z + 6.0),
            vec3f(1.4, 1.4, 1.4),
            vec4(0.3, 0.85, 0.5, 1.0),
            "bouncer",
        );
        // Rigid-body stack (M1a): real box3d dynamics, kicked periodically by
        // the tick loop; between kicks it settles and sleeps.
        //
        // Stacked as a PYRAMID, not a column. Six crates in a single tower is
        // the same six bodies but reads as a chimney — a shape nobody stacks
        // on purpose, so it looks like a bug rather than a demo. Three-two-one
        // is what a yard actually looks like, and it still topples on a kick.
        const CRATE: f32 = 1.0;
        let mut crate_i = 0;
        for (row, count) in [(0usize, 3usize), (1, 2), (2, 1)] {
            for c in 0..count {
                // Centre each row over the one below.
                let span = (count as f32 - 1.0) * CRATE * 2.05;
                let x = YARD_X - 5.0 - span * 0.5 + c as f32 * CRATE * 2.05;
                let id = spawn(
                    w,
                    BodyKind::Rigid,
                    Shape::Box,
                    vec3f(x, CRATE * 0.55 + row as f32 * CRATE * 2.02, YARD_Z - 4.0),
                    vec3f(CRATE, CRATE, CRATE),
                    vec4(0.9, 0.6 - 0.06 * crate_i as f32, 0.2, 1.0),
                    "rigid_crate",
                );
                if let Some(e) = w.entity_mut(id) {
                    e.restitution = 0.05;
                }
                crate_i += 1;
            }
        }
        for i in 0..2 {
            let id = spawn(
                w,
                BodyKind::Rigid,
                Shape::Sphere,
                vec3f(YARD_X - 2.0 + i as f32 * 1.6, 5.0, YARD_Z - 5.0),
                vec3f(1.1, 1.1, 1.1),
                vec4(0.4, 0.5, 0.95, 1.0),
                "rigid_ball",
            );
            if let Some(e) = w.entity_mut(id) {
                e.restitution = 0.5;
                e.friction = 0.4;
            }
        }
        // The Knight is a MOVER in the world, not a transform the demo draws
        // wherever it likes. He used to be the latter — `Knight::tick` wrote a
        // triangle wave straight into `self.pos` and nothing looked him up in
        // `world.entities` — so he walked through benches, houses and trees
        // alike. Not a collider bug: he was never in the world to be collided
        // with. Now the walk sets his VELOCITY and the sweep decides where he
        // actually ends up.
        //
        // Half-extents are a person — 0.7 m across, 1.8 m tall — so a bench at
        // 0.9 m intersects him and a doorway does not. `spawn` takes full
        // size, so these are doubled from the half-extents.
        //
        // He walks the pavement BETWEEN the road edge (z 3.5) and the bench
        // line (z 5.2), because a pedestrian route that runs through the
        // furniture leaves him jammed against the first bench forever now
        // that he genuinely collides. Walking round obstacles is NPC
        // behaviour, not layout, and belongs with the brains work.
        // A dozen villagers along the street. They are Movers like the player,
        // so they collide with the houses, the benches and the fence exactly
        // as he does; where they GO is decided by the NPC block, which scores
        // the village's points of interest against each villager's own
        // temperament and the time of day.
        const VILLAGERS: usize = 11;
        for i in 0..VILLAGERS {
            // Spread the start line along the pavement so they don't all
            // spawn inside one another and spend the first seconds pushing
            // apart — a crowd that begins as a pile reads as a bug.
            let x = -22.0 + i as f32 * 4.3;
            let z = if i % 2 == 0 { 4.1 } else { 7.6 };
            let id = spawn(
                w,
                BodyKind::Mover,
                Shape::Box,
                vec3f(x, 0.9, z),
                vec3f(0.7, 1.8, 0.7),
                vec4(0.85, 0.85, 0.9, 1.0),
                "villager",
            );
            if let Some(e) = w.entity_mut(id) {
                // The skinned mesh is the villager's appearance; this box is
                // only its substance, exactly as for the stock props.
                e.hidden = true;
                // Both of these are ZERO under Entity::default() — the trap
                // this codebase has now been bitten by three times. A villager
                // with gravity_scale 0 hangs in the air; with speed_mult 0 it
                // never moves however hard the brain pushes.
                e.gravity_scale = 1.0;
                e.speed_mult = 1.0;
            }
            self.villagers.push(Villager::new(id, 0x5eed_1701 ^ i as u64));
        }
        self.cast = CharacterModel::load_cast();
        // Deal the cast round-robin so neighbours differ, then let the
        // per-villager seed vary build and tint WITHIN a kind — two people of
        // the same kind should still not be the same person.
        let kinds = self.cast.len().max(1);
        for (i, v) in self.villagers.iter_mut().enumerate() {
            v.kind = i % kinds;
        }
        conform_statics_to_terrain(w);
        self.world_built = true;
    }

    /// The demo's "game logic" — what a script or blocks component will do
    /// later, done directly in Rust here.
    /// Load the stock props this demo places, choosing them BY DESCRIPTION
    /// through the asset index — the same path a generated game takes, so the
    /// demo exercises what Fable will actually use rather than a private
    /// shortcut with hardcoded paths.
    /// Load exactly the models the big-world plan named.
    ///
    /// No searching here: the plan already chose, and choosing again at load
    /// time would let the two disagree — the layout would be fitted for one
    /// model and drawn with another.
    fn load_plan_models(&mut self, cx: &mut Cx) {
        let Some(plan) = self.world_plan.as_ref() else {
            return;
        };
        let models_root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/resources/models"));
        let mut wanted: Vec<String> = plan.placements.iter().map(|p| p.model.clone()).collect();
        wanted.sort();
        wanted.dedup();
        let (mut ok, mut failed) = (0usize, 0usize);
        for id in &wanted {
            // id is `<source>/<pack>/<stem>`; the atlas is the pack's own.
            let mut parts = id.splitn(3, '/');
            let (src, pack, stem) = match (parts.next(), parts.next(), parts.next()) {
                (Some(a), Some(b), Some(c)) => (a, b, c),
                _ => {
                    failed += 1;
                    continue;
                }
            };
            let dir = models_root.join(src).join(pack);
            let glb = dir.join(format!("{stem}.glb"));
            let glb = if glb.exists() { glb } else { dir.join(format!("{stem}.gltf")) };
            let png = std::fs::read(dir.join("Textures").join("colormap.png")).ok();
            let Ok(bytes) = std::fs::read(&glb) else {
                failed += 1;
                continue;
            };
            if self.renderer.load_model(cx, id, &bytes, png.as_deref()).is_ok() {
                ok += 1;
            } else {
                failed += 1;
            }
        }
        log!("arcade: world models {ok} loaded, {failed} unavailable");
    }

    /// The driveable car's mesh, positioned from its rigid body this frame.
    ///
    /// A coloured slab standing in for a car was the last placeholder in the
    /// scene, and the library has 4,442 models. The chassis stays the box3d
    /// rigid body — collision fidelity past a sane box buys nothing for an
    /// arcade car — and this is only its appearance, scaled from the model's
    /// own bounds to the body it rides so the wheels meet the road.
    fn vehicle_instance(&self) -> Option<ModelInstance> {
        let id = self.vehicle_model.as_ref()?;
        let bounds = self.renderer.model_bounds(id)?;
        let (min, max) = bounds;
        let world = self.world.borrow();
        let car = world.entities.iter().find(|e| e.tag == "car")?;
        // Scale the model's length onto the chassis length: a car reads by its
        // proportions, and the packs author them at every size.
        let native_len = (max.z - min.z).max(max.x - min.x).max(0.001);
        let s = (car.half.z * 2.0) / native_len;
        // Where the road is under this body. NOT the chassis box: a raycast
        // vehicle's box hangs a wheel radius plus an uncompressed spring clear
        // of the ground and never touches it, so sitting the mesh on the box —
        // the rule that IS right for a walker, whose box bottom is its feet —
        // left the car hovering by the difference. The car block measures it
        // from the suspension it just ran. A rigid with no car block has no
        // wheels holding it up either, so there the box bottom really is the
        // contact plane.
        let drop = self
            .blocks
            .borrow()
            .cars
            .iter()
            .find(|c| c.entity == car.id)
            .map_or(car.half.y, |c| c.contact_drop());
        Some(ModelInstance::on_body(
            id.clone(),
            bounds,
            s,
            drop,
            &GameRenderer::rigid_transform(car),
        ))
    }

    /// Turn the plan's placements into draw instances and collider boxes.
    ///
    /// The plan is pure and knows nothing about what loaded, so this is where
    /// the two meet: `realise` asks for each model's real bounds and gets
    /// `None` for anything whose pack never downloaded. A missing model leaves
    /// a hole rather than a wrongly-scaled stand-in, and the count is logged —
    /// a gap in a region is otherwise indistinguishable from a layout bug.
    fn realise_plan(&mut self) -> (Vec<ModelInstance>, Vec<(Vec3f, Vec3f)>) {
        let Some(plan) = self.world_plan.take() else {
            return (Vec::new(), Vec::new());
        };
        let renderer = &self.renderer;
        let out = bigworld::realise(&plan, |id| {
            let (min, max) = renderer.model_bounds(id)?;
            Some(bigworld::ModelGeometry {
                min,
                max,
                triangles: renderer.model_triangles(id).unwrap_or(0),
                collider_parts: renderer
                    .model_collider_parts(id)
                    .map(|p| p.to_vec())
                    .unwrap_or_default(),
            })
        });
        if !out.missing.is_empty() {
            log!(
                "arcade: {} models unavailable, e.g. {}",
                out.missing.len(),
                out.missing.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
            );
        }
        let colliders = out.colliders.iter().map(|c| (c.pos, c.half)).collect();
        self.world_plan = Some(plan);
        (out.instances, colliders)
    }

    /// Give the plan's inhabitants their destinations and brains.
    fn wire_plan_blocks(&mut self) {
        let Some(plan) = self.world_plan.as_ref() else {
            return;
        };
        let mut blocks = self.blocks.borrow_mut();
        let mut pois = PoiSet::default();
        for p in &plan.pois {
            pois.push(Poi::new(p.pos, p.tag).with_capacity(p.capacity));
        }
        blocks.pois = pois;

        let world = self.world.borrow();
        let ids: Vec<u64> = world
            .entities
            .iter()
            .filter(|e| e.tag == "villager")
            .map(|e| e.id)
            .collect();
        // The plan's Nth npc is the Nth villager entity: build_big_world
        // spawned them in plan order and nothing has removed one since.
        for (i, id) in ids.iter().enumerate() {
            let (home, speed) = plan
                .npcs
                .get(i)
                .map(|n| (n.home, n.speed))
                .unwrap_or((vec3f(0.0, 0.9, 0.0), 2.4));
            let mut cfg = NpcConfig::default();
            cfg.speed = speed;
            blocks.npcs.push(Npc::new(
                *id,
                cfg,
                home,
                0x51de_0000 ^ (i as u64).wrapping_mul(0x9E37_79B9),
            ));
        }
        log!(
            "arcade: {} npcs over {} pois",
            blocks.npcs.len(),
            blocks.pois.list.len()
        );
    }

    /// The driveable car's mesh, chosen by description like every other prop.
    ///
    /// Searched rather than hardcoded so the demo exercises the path a
    /// generated game takes — and `Variants` rather than `Mixed`, because a
    /// car must be ONE recognisable object: spreading across families returns
    /// a car, then a tractor, then a boat.
    fn load_vehicle_model(&mut self, cx: &mut Cx) {
        use makepad_game_assets::{AssetKind, Filters, Spread, VarietyParams};
        let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/resources"));
        let index = makepad_game_assets::AssetIndex::build(&root);
        let params = VarietyParams {
            count: 6,
            spread: Spread::Variants,
            seed: 0x5eed_1701,
            filters: Filters {
                kind: Some(AssetKind::Model),
                ..Default::default()
            },
        };
        for entry in index.find_many("race car vehicle", &params) {
            let (id, path) = (entry.id.clone(), entry.path.clone());
            let png = path
                .parent()
                .map(|d| d.join("Textures").join("colormap.png"))
                .and_then(|p| std::fs::read(p).ok());
            let Ok(glb) = std::fs::read(&path) else { continue };
            if self.renderer.load_model(cx, &id, &glb, png.as_deref()).is_ok() {
                log!("arcade: vehicle model = {id}");
                // Put the simulated wheels under the drawn ones. Until this
                // runs, the car pitches about contact points that are not
                // where its visible wheels are, and on a slope the wheels
                // leave the ground.
                if let Some(bounds) = self.renderer.model_bounds(&id) {
                    let (min, max) = bounds;
                    let native_len = (max.z - min.z).max(max.x - min.x).max(0.001);
                    let car_half_z = {
                        let world = self.world.borrow();
                        world
                            .entities
                            .iter()
                            .find(|e| e.tag == "car")
                            .map(|e| e.half.z)
                    };
                    if let Some(car_half_z) = car_half_z {
                        let s = (car_half_z * 2.0) / native_len;
                        // Wheels sit at the model's horizontal extremes; its
                        // wheel radius is half the ground clearance it was
                        // authored with, which for an origin-at-the-contact-
                        // patch vehicle is half the lowest axle height. Taking
                        // it as a fraction of the body height is the robust
                        // reading across kits that vary wildly in proportion.
                        let half_x = (max.x - min.x) * 0.5 * s;
                        let half_z = (max.z - min.z) * 0.5 * s;
                        let radius = (max.y - min.y) * 0.25 * s;
                        let mut blocks = self.blocks.borrow_mut();
                        for c in blocks.cars.iter_mut() {
                            c.fit_wheels_to_model(half_x * 0.92, half_z * 0.92, radius);
                        }
                        log!(
                            "arcade: wheels fitted to model — half_track {:.2}, \
                             half_wheelbase {:.2}, radius {:.2}",
                            half_x * 0.92,
                            half_z * 0.92,
                            radius
                        );
                    }
                }
                self.vehicle_model = Some(id);
                // The chassis box is now only the physics body; leaving it
                // visible would draw a coloured slab inside the car.
                let mut world = self.world.borrow_mut();
                let car = world.entities.iter().find(|e| e.tag == "car").map(|e| e.id);
                if let Some(car) = car {
                    if let Some(e) = world.entity_mut(car) {
                        e.hidden = true;
                    }
                }
                world.mark_render_dirty();
                return;
            }
        }
        log!("arcade: no loadable vehicle model — car stays a box");
    }

    fn load_props(&mut self, cx: &mut Cx) {
        use makepad_game_assets::{AssetKind, Filters, Spread, VarietyParams};
        let root = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/resources"));
        let index = makepad_game_assets::AssetIndex::build(&root);
        // Fixed so the demo is reproducible: the same seed picks the same
        // models every run, which is also what lets multiplayer replicate a
        // scene as (query, seed) instead of shipping a model list.
        const SEED: u64 = 0x5eed_1701;
        // (query, role, how many DISTINCT models to resolve, spread)
        //
        // `Variants` where the point is a row of the same KIND of thing that
        // differs in detail — five house designs from one suburb, not one
        // house from each of five art styles. `Mixed` where genuine species
        // variety is wanted, as in a wood.
        //
        // `Variants` is also the safe choice for anything that must be ONE
        // recognisable object: it takes only the best-matching family, so it
        // cannot wander. `Mixed` round-robins across families, which is right
        // for a wood (pine, oak, birch) and wrong for a bench — asking for a
        // "park bench wooden" spread across families returned the bench, then
        // a COASTER TRAIN and a PARK ENTRANCE, because each of those matches
        // one word of the query.
        let wanted: &[(&str, &str, usize, Spread)] = &[
            ("pine tree", "tree", 4, Spread::Mixed),
            ("broadleaf tree", "tree2", 3, Spread::Mixed),
            // Two, not three: the third hit is `cliff_blockCave_rock`, a
            // cave-mouth tile that reads as a small teal-roofed building
            // dropped on the grass. A known ranking wart — asking for fewer
            // is the honest workaround until "rock" stops meaning that.
            // `Variants`, not `Mixed`: mixing round-robins across FAMILIES,
            // and the neighbouring family is `cliff_blockCave_rock` — a
            // cave-mouth tile that reads as a small teal-roofed building
            // dropped on the grass. Staying inside the best family keeps a
            // rock a rock.
            ("rock stone", "rock", 2, Spread::Variants),
            // Yard dressing: a builder's yard of bare primitives reads as a
            // physics test, which is what it looked like.
            ("wooden crate box", "crate_prop", 2, Spread::Variants),
            ("barrel", "barrel", 2, Spread::Variants),
            ("fence", "fence", 1, Spread::Variants),
            ("suburban house building", "house", 5, Spread::Variants),
            ("park bench", "bench", 2, Spread::Variants),
            ("street light post", "lamp", 2, Spread::Variants),
        ];
        for (query, role, count, spread) in wanted {
            // Ask for more than needed: some candidates belong to packs whose
            // atlas was never downloaded and cannot render, so the surplus is
            // what keeps the scene full rather than leaving holes. The same
            // over-ask is what a generated game wants.
            //
            // Kit tiles welded to a chunk of ground (`hexagon-kit/building-
            // house` is a house on a hex of grass) read as floating islands on
            // open grass. Excluding whole kits by name is the wrong lever —
            // it pushed "house" to a HOUSEBOAT — so the query steers and the
            // load walk skips what will not render.
            let params = VarietyParams {
                count: count + 6,
                spread: *spread,
                seed: SEED,
                filters: Filters {
                    kind: Some(AssetKind::Model),
                    ..Default::default()
                },
            };
            let mut ids = Vec::new();
            for entry in index.find_many(query, &params) {
                if ids.len() >= *count {
                    break;
                }
                if entry.id.contains("hexagon-kit") {
                    continue;
                }
                let (id, path) = (entry.id.clone(), entry.path.clone());
                // The atlas sits beside the model in the pack's Textures/ dir.
                let png = path
                    .parent()
                    .map(|d| d.join("Textures").join("colormap.png"))
                    .and_then(|p| std::fs::read(p).ok());
                let Ok(glb) = std::fs::read(&path) else { continue };
                if self.renderer.load_model(cx, &id, &glb, png.as_deref()).is_ok() {
                    ids.push(id);
                }
            }
            if ids.is_empty() {
                log!("arcade: no loadable stock model for '{query}'");
            } else {
                log!("arcade: prop '{role}' = {} distinct: {}", ids.len(), ids.join(", "));
                self.props.insert(role.to_string(), ids);
            }
        }
    }

    /// One placed prop: where it stands and what it does to a walker.
    ///
    /// `blocking` is the half-extent of its collider, or `None` for scenery a
    /// player should walk through. It is separate from the visual scale
    /// because the two genuinely differ for foliage: a pine's canopy is most
    /// of its silhouette and none of its obstruction.
    fn compose_village(&self) -> (Vec<ModelInstance>, Vec<PropCollider>) {
        let mut models = Vec::new();
        let mut colliders = Vec::new();

        // Kenney packs are authored at wildly different native sizes, so a
        // fixed multiplier gives a 12-unit bench beside a 2-unit house. Scale
        // to a TARGET HEIGHT read off the model's own bounds instead, and the
        // scene keeps its proportions whichever pack a query resolves to.
        // `variant` selects among the DISTINCT models resolved for this role,
        // wrapping if fewer came back than the scene asks for. Every caller
        // passes a different one, which is the whole point: the index returned
        // five house designs and placing hit #1 five times is what made the
        // street look copy-pasted.
        let mut place = |role: &str,
                         variant: usize,
                         x: f32,
                         z: f32,
                         yaw: f32,
                         target_h: f32,
                         block: Blocking| {
            let Some(ids) = self.props.get(role) else { return };
            if ids.is_empty() {
                return;
            }
            let id = &ids[variant % ids.len()];
            let Some((min, max)) = self.renderer.model_bounds(id) else {
                return;
            };
            let native_h = (max.y - min.y).max(0.001);
            let mut s = target_h / native_h;
            // Scaling by height alone assumes the model is roughly as tall as
            // it is wide. A short, wide model — a ground patch, a fallen log,
            // a canopy authored flat — then blows up sideways into a coloured
            // slab lying across the scene. Cap the footprint at a few times
            // the target height and re-derive the scale from width when it
            // would exceed that. Any library picked by description will
            // eventually hand back something oddly proportioned, so the guard
            // belongs here rather than in a list of models to avoid.
            let native_w = (max.x - min.x).max(max.z - min.z).max(0.001);
            let max_w = target_h * 2.5;
            if native_w * s > max_w {
                s = max_w / native_w;
            }
            let s = s;
            let mut m = Mat4f::rotation(vec3f(0.0, yaw, 0.0));
            for i in 0..12 {
                m.v[i] *= s;
            }
            m.v[12] = x;
            m.v[13] = 0.0;
            m.v[14] = z;
            models.push(ModelInstance {
                model: id.clone(),
                transform: m,
            });
            // The footprint is the model's own, scaled. A yaw of a quarter
            // turn swaps x/z extents; anything between would need the rotated
            // hull, and for props sitting on axis-aligned streets it never is.
            let (hx, hz) = ((max.x - min.x) * 0.5 * s, (max.z - min.z) * 0.5 * s);
            let quarter_turned = (yaw.abs() - std::f32::consts::FRAC_PI_2).abs() < 0.2;
            let (hx, hz) = if quarter_turned { (hz, hx) } else { (hx, hz) };
            if block == Blocking::None {
                return;
            }
            // The prop's OWN primitives are the collider: a house arrives as
            // walls and roof, so its doorway stays a gap; a tree as trunk and
            // canopy, so the canopy can be dropped without inventing a shape.
            // One AABB round either would be worse than nothing — a solid
            // doorway, or a wall you bump into ten feet from the trunk.
            let parts = self
                .renderer
                .model_collider_parts(id)
                .unwrap_or_default()
                .to_vec();
            let quarter = quarter_turned;
            let mut pushed = 0;
            for (pa, pb) in &parts {
                // Model space -> world: scale about the origin, then place.
                let (cx, cy, cz) = (
                    (pa.x + pb.x) * 0.5 * s,
                    (pa.y + pb.y) * 0.5 * s,
                    (pa.z + pb.z) * 0.5 * s,
                );
                let (mut ex, ey, mut ez) = (
                    (pb.x - pa.x) * 0.5 * s,
                    (pb.y - pa.y) * 0.5 * s,
                    (pb.z - pa.z) * 0.5 * s,
                );
                let (mut ox, mut oz) = (cx, cz);
                if quarter {
                    std::mem::swap(&mut ex, &mut ez);
                    let t = ox;
                    ox = if yaw > 0.0 { -oz } else { oz };
                    oz = if yaw > 0.0 { t } else { -t };
                }
                // Trunk-only: a canopy sits high and wide, a trunk low and
                // narrow. Keeping the narrow low part is what lets a walker
                // pass under branches but not through the tree.
                if block == Blocking::Trunk {
                    let slim = ex.max(ez) < (max.x - min.x) * 0.5 * s * 0.45;
                    let low = cy < target_h * 0.6;
                    if !(slim && low) {
                        continue;
                    }
                }
                colliders.push(PropCollider {
                    pos: vec3f(x + ox, cy.max(ey), z + oz),
                    half: vec3f(ex.max(0.08), ey.max(0.08), ez.max(0.08)),
                });
                pushed += 1;
            }
            // A model whose primitives were all filtered out still needs to be
            // solid, or the prop silently becomes scenery again — which is the
            // exact bug this work exists to fix. Trees need this most: a pine
            // modelled as one merged mesh has no separate trunk to find, so
            // fall back to a narrow post at its base.
            if pushed == 0 {
                let (ph, hy) = match block {
                    Blocking::Trunk => (0.16, target_h * 0.35),
                    _ => (1.0, target_h * 0.5),
                };
                colliders.push(PropCollider {
                    pos: vec3f(x, hy, z),
                    half: vec3f((hx * ph).max(0.18), hy, (hz * ph).max(0.18)),
                });
            }
        };

        // --- the street ------------------------------------------------
        // Houses stand back from the road on the north side, all facing it.
        // Uniform facing is the point: a row of houses that agree about where
        // the street is reads as a street, and random yaw reads as debris.
        const FACE_SOUTH: f32 = 0.0;
        // A different house design at every lot, and heights that vary a
        // little: a terrace of clones reads as wallpaper, however good the
        // model is.
        for (i, x) in [-19.0f32, -10.0, 0.5, 10.0, 19.0].iter().enumerate() {
            let h = 4.2 + (i % 3) as f32 * 0.35;
            place("house", i, *x, -10.0, FACE_SOUTH, h, Blocking::Solid);
        }
        // Lamps down the north verge, evenly spaced like street furniture.
        for i in 0..5 {
            place("lamp", i, -18.0 + i as f32 * 9.0, -4.6, 0.0, 3.2, Blocking::None);
        }
        // Benches on the south verge, turned to face the road.
        for (i, x) in [-12.0f32, 0.0, 12.0].iter().enumerate() {
            place("bench", i, *x, 5.2, std::f32::consts::PI, 0.9, Blocking::Solid);
        }

        // --- boundary --------------------------------------------------
        // A fence along the south edge of the green: a continuous run, not a
        // scatter, so it reads as an enclosure. One design for the whole run —
        // a fence that changes style panel to panel is a fence nobody built.
        //
        // Spacing comes from the panel's OWN scaled width. A fixed 4-unit step
        // left a one-unit panel with three units of air between it and the
        // next, which reads as a line of signposts rather than a fence — the
        // model is the only thing that knows how wide a panel is.
        const FENCE_H: f32 = 1.1;
        let fence_step = self
            .props
            .get("fence")
            .and_then(|ids| ids.first())
            .and_then(|id| self.renderer.model_bounds(id))
            .map(|(min, max)| {
                let s = FENCE_H / (max.y - min.y).max(0.001);
                // Panels butt up rather than overlap; a hair of slack keeps
                // coincident faces from z-fighting along the run.
                ((max.x - min.x) * s * 0.98).max(0.4)
            })
            .unwrap_or(1.6);
        // The run BOUNDS THE YARD rather than crossing the green. A fence that
        // encloses nothing is scenery pretending to be structure: it reads as
        // a line dropped across the grass, which is exactly how the previous
        // layout looked. Two legs meeting at a corner say "this is the yard"
        // with the same panel count.
        let mut fence_leg = |x0: f32, z0: f32, x1: f32, z1: f32, yaw: f32| {
            let len = ((x1 - x0) * (x1 - x0) + (z1 - z0) * (z1 - z0)).sqrt();
            let panels = (len / fence_step).ceil().max(1.0) as usize;
            for i in 0..panels {
                let t = i as f32 / panels as f32;
                place(
                    "fence",
                    0,
                    x0 + (x1 - x0) * t,
                    z0 + (z1 - z0) * t,
                    yaw,
                    FENCE_H,
                    Blocking::Solid,
                );
            }
        };
        // North side, along the road — the face a passer-by sees.
        fence_leg(-27.0, 13.5, -5.0, 13.5, 0.0);
        // East side, closing the corner back toward the trees.
        fence_leg(-5.0, 13.5, -5.0, 26.0, std::f32::consts::FRAC_PI_2);

        // --- woodland --------------------------------------------------
        // Clustered, not sprinkled: three stands with gaps between them is
        // what makes a wood read as a wood rather than an orchard.
        let stands: [(f32, f32, usize); 3] = [(-20.0, -22.0, 7), (2.0, -25.0, 6), (20.0, -21.0, 6)];
        for (si, (cx, cz, n)) in stands.iter().enumerate() {
            for i in 0..*n {
                // Deterministic jitter — a fixed pattern, so captures compare.
                let a = (si * 7 + i * 13) as f32 * 1.107;
                let r = 3.0 + ((si * 5 + i * 3) % 7) as f32 * 1.3;
                let x = cx + makepad_game_math::cos(a) * r;
                let z = cz + makepad_game_math::sin(a) * r * 0.7;
                // Species AND variant both turn over: a wood of one silhouette
                // repeated is an orchard, and that is what a single hit gives.
                let role = if (si + i) % 3 == 0 { "tree2" } else { "tree" };
                let h = 5.5 + ((i * 3 + si) % 4) as f32 * 1.4;
                place(role, si + i, x, z, a, h, Blocking::Trunk);
            }
        }
        // Rocks at the wood's edge, where scree actually collects.
        for (i, (x, z)) in [(-13.0f32, -16.0f32), (9.0, -17.0), (25.0, -14.0)]
            .iter()
            .enumerate()
        {
            place(
                "rock",
                i,
                *x,
                *z,
                i as f32 * 1.3,
                1.1 + i as f32 * 0.35,
                Blocking::Solid,
            );
        }
        // Yard dressing. The rigid-body demo still does the physics; these are
        // the props that make the corner read as a working yard rather than a
        // test harness — stacked stock alongside the crates that topple.
        for (i, (x, z)) in [(-24.0f32, 22.5f32), (-22.4, 23.4), (-11.0, 21.0)]
            .iter()
            .enumerate()
        {
            place("crate_prop", i, *x, *z, i as f32 * 0.7, 1.0, Blocking::Solid);
        }
        for (i, (x, z)) in [(-19.5f32, 24.0f32), (-18.3, 23.2), (-9.5, 23.5)]
            .iter()
            .enumerate()
        {
            place("barrel", i, *x, *z, i as f32 * 1.1, 1.1, Blocking::Solid);
        }
        (models, colliders)
    }

    /// Spawn the driveable car and the patrolling Knight, proving blocks work
    /// outside gamemaker: no script VM anywhere in this app.
    fn spawn_blocks(&mut self) {
        let mut blocks = self.blocks.borrow_mut();
        blocks.clear();
        let mut world = self.world.borrow_mut();
        let w = &mut *world;
        w.next_id += 1;
        let car = w.next_id;
        w.push_entity(Entity {
            id: car,
            kind: BodyKind::Rigid,
            pos: vec3f(-6.0, 1.2, 1.6),
            half: vec3f(0.9, 0.4, 1.6),
            color: vec4(0.86, 0.32, 0.28, 1.0),
            tag: "car".to_string(),
            collide: true,
            hidden: false,
            gravity_scale: 1.0,
            speed_mult: 1.0,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            density: 1.0,
            friction: 0.7,
            ..Default::default()
        });
        blocks.cars.push(Car::new(
            car,
            CarConfig::default(),
            ControlSource::Player,
        ));

        // The player. THIS is the whole binding — a body, a character block,
        // and a rig. Everything else (third-person camera, mount state
        // machine, the walk/drive modality switch, getting back out beside the
        // car) comes from the prefab with no further wiring here.
        w.next_id += 1;
        self.player = w.next_id;
        w.push_entity(Entity {
            id: self.player,
            kind: BodyKind::Mover,
            // Out on the open verge rather than tucked against a house: the
            // boom correctly pulls in when a wall is behind you, and spawning
            // in that pocket makes the very first frame look broken.
            pos: vec3f(-6.0, 2.0, 9.5),
            half: vec3f(0.35, 0.9, 0.35),
            color: vec4(0.35, 0.55, 0.95, 1.0),
            tag: "player".to_string(),
            // Hidden box, skinned mesh for the appearance — the same split
            // the villagers use, so the player collides like everyone else
            // without being a floating grey slab in the middle of the screen.
            hidden: true,
            collide: true,
            gravity_scale: 1.0,
            speed_mult: 1.0,
            scale: vec3f(1.0, 1.0, 1.0),
            scale_target: vec3f(1.0, 1.0, 1.0),
            density: 1.0,
            friction: 0.7,
            ..Default::default()
        });
        blocks.characters.push(Character::new(
            self.player,
            CharacterConfig::default(),
            ControlSource::Player,
            None,
        ));
        blocks
            .player_rigs
            .insert(PlayerId(0), PlayerRig::new(self.player));

        w.mark_render_dirty();

        // Destinations first: the NPC block scores POIs, so a village with
        // none degrades to aimless wandering. These come from the composed
        // street rather than being listed as coordinates — the same call a
        // generated game would make over its own props.
        blocks.pois = self.village_pois();

        // Then the villagers. Each gets its own seed, so identical config
        // still yields unlike people: one hurries, one dawdles, one is
        // sociable enough to fall into step beside a neighbour.
        let villager_ids: Vec<u64> = w
            .entities
            .iter()
            .filter(|e| e.tag == "villager")
            .map(|e| e.id)
            .collect();
        for (i, id) in villager_ids.iter().enumerate() {
            let home = w.entity(*id).map(|e| e.pos).unwrap_or(vec3f(0.0, 0.9, 4.1));
            blocks.npcs.push(Npc::new(
                *id,
                NpcConfig::default(),
                home,
                0x51de_0000 ^ (i as u64).wrapping_mul(0x9E37_79B9),
            ));
        }
        drop(world);
        drop(blocks);
        // The player wears a real rig like everyone else. A cube with a
        // camera behind it reads as a debug view; the whole point of the
        // third-person controller is that you can see yourself walk.
        let me = self.player;
        let mut mine = Villager::new(me, 0x91ae_5eed);
        // Pinned BY NAME, not by index. The cast is assembled from whatever
        // the asset library happens to contain, so an index would silently
        // become a different person the moment a pack is added or the civilian
        // filter changes — and the failure looks like a cosmetic surprise
        // rather than a bug. If the named model is missing (no asset packs
        // downloaded) the player just keeps the default kind.
        if let Some(kind) = self
            .cast
            .iter()
            .position(|c| c.label == Self::PLAYER_CHARACTER)
        {
            mine.kind = kind;
        }
        self.villagers.push(mine);
    }

    /// Points of interest derived from the composed street.
    ///
    /// Positions come from `compose_village`'s own layout constants rather
    /// than from the prop entities, because the props are ModelInstances and
    /// their collider entities are all tagged "scenery" — a tag the camera
    /// boom and the raycast both key on, so re-tagging them per role to feed
    /// `PoiSet::from_tags` would change camera behaviour to buy nothing.
    fn village_pois(&self) -> PoiSet {
        let mut pois = PoiSet::default();
        // Benches on the south verge: two seats each, so a third villager
        // walks past a full one instead of standing inside the pair on it.
        for x in [-16.0f32, -4.0, 8.0, 20.0] {
            pois.push(Poi::new(vec3f(x, 0.0, 6.4), "bench").with_capacity(2));
        }
        // House doors on the north side: one at a time, which is what makes a
        // villager wait or pick elsewhere rather than merging into a doorway.
        for x in [-18.0f32, -9.0, 0.0, 9.0, 18.0] {
            pois.push(Poi::new(vec3f(x, 0.0, -6.6), "door").with_capacity(1));
        }
        // The lamps and the yard gate are open ground: somewhere to stand
        // about, which is what keeps the street from being only a commute
        // between benches and doors.
        for x in [-13.5f32, 4.5] {
            pois.push(Poi::new(vec3f(x, 0.0, -3.4), "lamp").with_capacity(3));
        }
        pois.push(Poi::new(vec3f(-8.0, 0.0, 14.0), "yard").with_capacity(4));
        pois
    }

    /// How fast the right stick turns the camera, in mouse-pixel equivalents
    /// per second. The rig's own `sensitivity` converts to radians, so stick
    /// and mouse share one tuning constant instead of drifting apart.
    /// Who the player is. The barbarian: bearded, and on the full 41-joint
    /// hero rig rather than the 7-joint civilian one, so the character you
    /// spend the whole game looking at is the best-animated thing on screen.
    const PLAYER_CHARACTER: &'static str = "barbarian";

    const PAD_LOOK_SPEED: f32 = 520.0;
    /// Right-stick deadzone. Sticks drift; without this the camera creeps.
    const PAD_LOOK_DEADZONE: f32 = 0.18;
    /// Deflection at which the walk becomes a run. Below it the stick scales
    /// speed on its own; above it `run` fades in the run multiplier, so the
    /// whole range from a crawl to a sprint is one continuous push.
    const PAD_RUN_KNEE: f32 = 0.75;

    /// Poll the most active gamepad into `self.pad`.
    ///
    /// Arcade never did this. That — not a wrong button number — is why a
    /// paired PS5 pad did nothing: the device's state never entered the app,
    /// so every binding downstream was reading a struct nobody filled.
    ///
    /// "Most active" rather than "first": a machine with a wheel, a headset
    /// and a pad connected should follow whichever the player is holding.
    /// gamemaker's precedent, and it has the same known limitation — the
    /// runners-up are DISCARDED, so two pads cannot be two local players yet.
    ///
    /// Convert a right-stick deflection into the mouse-pixel look units the
    /// rig consumes.
    ///
    /// The negation is the whole function. A stick reports UP as +y. Screen
    /// space grows DOWNWARD, and `look_dy` is in screen units — `+look_dy`
    /// raises the camera and looks down (see `controller.rs`, "`+pitch` lifts
    /// the camera and looks down"). Feed the stick straight through and
    /// pushing it up looks DOWN, while pushing the mouse up looks UP: two
    /// devices disagreeing about which way is up, on the same camera.
    ///
    /// `inverted` restores that for players who want it — which is a
    /// preference, not the default it used to be by accident.
    fn pad_look_units(lx: f32, ly: f32, dt: f32, inverted: bool) -> (f32, f32) {
        let sign = if inverted { 1.0 } else { -1.0 };
        (
            lx * Self::PAD_LOOK_SPEED * dt,
            ly * Self::PAD_LOOK_SPEED * dt * sign,
        )
    }

    /// The headless backend has no game input at all, so this is stubbed
    /// there — without the split, arcade stops building headless and the
    /// whole render-to-PNG test path goes with it.
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
                + pad.right_stick.x.abs() as f32
                + pad.right_stick.y.abs() as f32
                + pad.left_trigger
                + pad.right_trigger
                + pad.a
                + pad.b
                + pad.x
                + pad.y
                + pad.dpad_up
                + pad.dpad_down
                + pad.dpad_left
                + pad.dpad_right;
            if best.is_none() || score > best_score {
                best_score = score;
                best = Some(pad.clone());
            }
        }
        // Device switching is by activity, not by menu: touch the pad and it
        // takes over the affordance glyph; touch the keyboard and it hands
        // back (see `raw_input`). The intent itself is always merged, so
        // nothing is ever ignored — only the hint changes.
        if best_score > 0.05 {
            self.pad_is_active = true;
        }
        if std::env::var("ARCADE_PAD_DEBUG").is_ok() {
            if let Some(p) = &best {
                log!(
                    "pad: LS({:+.2},{:+.2}) RS({:+.2},{:+.2}) LT{:.2} RT{:.2} \
                     a{:.0} b{:.0} x{:.0} y{:.0} L1{:.0} R1{:.0}",
                    p.left_stick.x, p.left_stick.y, p.right_stick.x, p.right_stick.y,
                    p.left_trigger, p.right_trigger, p.a, p.b, p.x, p.y,
                    p.left_shoulder, p.right_shoulder
                );
            } else {
                log!("pad: none connected");
            }
        }
        let present = best.is_some();
        if present != self.pad_present_prev {
            self.pad_present_prev = present;
            log!("arcade: gamepad {}", if present { "connected" } else { "disconnected" });
        }
        self.pad = best;
    }

    #[cfg(headless)]
    fn poll_gamepad(&mut self, _cx: &mut Cx) {}

    /// **One normalised intent, three devices.**
    ///
    /// Keyboard, gamepad and headset all land in the same [`RawInput`]; the
    /// rig has no idea which produced it, which is what keeps walking and
    /// driving from growing a per-device branch each. Merged rather than
    /// switched — a keyboard press during a stick push still counts.
    ///
    /// The ONE place the devices legitimately differ is `run`: a key has no
    /// deflection, so shift means full run, while a stick derives it from how
    /// far it is pushed. Everything else is the same signal from a different
    /// wire.
    fn raw_input(&mut self, dt: f32) -> RawInput {
        let down = |k: KeyCode| self.keys.contains(&k);
        let axis = |pos: bool, neg: bool| pos as i8 as f32 - neg as i8 as f32;

        let key_x = axis(
            down(KeyCode::ArrowRight) || down(KeyCode::KeyD),
            down(KeyCode::ArrowLeft) || down(KeyCode::KeyA),
        );
        // +y is forward, and W/Up is forward.
        let key_y = axis(
            down(KeyCode::ArrowUp) || down(KeyCode::KeyW),
            down(KeyCode::ArrowDown) || down(KeyCode::KeyS),
        );
        let key_run = down(KeyCode::Shift);
        let key_jump = down(KeyCode::Space);
        let key_use = down(KeyCode::KeyE);
        if key_x != 0.0 || key_y != 0.0 || key_jump || key_use {
            self.pad_is_active = false;
        }

        let mut raw = RawInput {
            move_x: key_x,
            move_y: key_y,
            // A keyboard cannot express "half forward", so shift is the whole
            // continuum it gets. This is the only device-specific line here.
            run: if key_run { 1.0 } else { 0.0 },
            jump: key_jump,
            jump_pressed: key_jump && !self.jump_prev,
            use_pressed: key_use && !self.use_prev,
            // Held keys drive BOTH modalities; the rig picks by seat, so one
            // key never needs to know whether the player is walking today.
            throttle: key_y,
            brake: 0.0,
            handbrake: if key_jump { 1.0 } else { 0.0 },
            look_dx: self.look_accum.x as f32,
            look_dy: self.look_accum.y as f32,
        };
        self.jump_prev = key_jump;
        self.use_prev = key_use;
        self.look_accum = dvec2(0.0, 0.0);

        if let Some(pad) = self.pad.clone() {
            // Left stick + dpad: movement. Passed RAW — the rig's own
            // `movement_intent` deadzones and normalises it, so there is
            // exactly one deadzone implementation in the walking path.
            let stick_x = pad.left_stick.x as f32
                + axis(pad.dpad_right > 0.5, pad.dpad_left > 0.5);
            // Stick up reports +y and means forward, same as W.
            let stick_y = pad.left_stick.y as f32
                + axis(pad.dpad_up > 0.5, pad.dpad_down > 0.5);
            if stick_x != 0.0 || stick_y != 0.0 {
                raw.move_x = stick_x.clamp(-1.0, 1.0);
                raw.move_y = stick_y.clamp(-1.0, 1.0);
                // Walk→run from deflection alone: below the knee the stick
                // scales speed by itself, above it the run multiplier fades
                // in. No threshold, so nothing snaps.
                let mag = (raw.move_x * raw.move_x + raw.move_y * raw.move_y)
                    .sqrt()
                    .min(1.0);
                raw.run = ((mag - Self::PAD_RUN_KNEE) / (1.0 - Self::PAD_RUN_KNEE))
                    .clamp(0.0, 1.0);
            }
            // Analog triggers. THIS is why a pad beats a keyboard in a car:
            // partial pressure is partial acceleration. Confirmed real 0..1
            // floats from GCController's leftTrigger.value / rightTrigger.value.
            let trigger = pad.right_trigger - pad.left_trigger;
            if trigger != 0.0 {
                raw.throttle = trigger;
            }
            // LT is deliberately NOT mapped to `brake` as well. The car block
            // already does brake-then-reverse by context from the throttle
            // sign alone (car.rs: negative throttle while rolling forward
            // applies braking; once stopped it drives backward). Setting
            // `brake` at the same time adds a force that opposes the reverse
            // it just asked for, which reads as "reverse barely works".
            raw.handbrake = raw.handbrake.max(pad.right_shoulder);
            // Right stick: camera. Its own deadzone, then scaled into the
            // same pixel-equivalent units the mouse produces so both share
            // the rig's clamped pitch and smoothing.
            let (lx, ly) = makepad_game_blocks::movement_intent(
                pad.right_stick.x as f32,
                pad.right_stick.y as f32,
                Self::PAD_LOOK_DEADZONE,
            );
            let (dx, dy) = Self::pad_look_units(lx, ly, dt, std::env::var("ARCADE_INVERT_Y").is_ok());
            raw.look_dx += dx;
            raw.look_dy += dy;
            // Cross jumps; Square always interacts. Cross ALSO interacts when
            // something is in reach, because "Press ✕ to drive" is the hint a
            // player expects — the prompt is on screen at that moment, so the
            // meaning is never ambiguous, and Square stays available if you
            // did want to jump beside a car.
            let cross = pad.a > 0.5;
            let square = pad.x > 0.5;
            let offered = self.prompt.is_some();
            let cross_edge = cross && !self.pad_jump_prev;
            let square_edge = square && !self.pad_use_prev;
            raw.jump |= cross && !offered;
            raw.jump_pressed |= cross_edge && !offered;
            raw.use_pressed |= square_edge || (cross_edge && offered);
            self.pad_jump_prev = cross;
            self.pad_use_prev = square;
        }

        // A headset drives the same player as everything else.
        if self.xr_active {
            if raw.move_x == 0.0 && raw.move_y == 0.0 {
                raw.move_x = self.xr_pad.axis_x as f32;
                raw.move_y = -self.xr_pad.axis_z as f32;
            }
            raw.jump |= self.xr_pad.jump;
            raw.jump_pressed |= self.xr_pad.jump_pressed;
            raw.use_pressed |= self.xr_pad.grab_pressed;
        }
        raw
    }

    /// Resolve the activity button, then advance the player rig.
    ///
    /// Runs BEFORE `session.tick` (which does pre_step → step_world →
    /// post_step) because the rig produces the input those steps consume, and
    /// before `sync_local_player` so slot 0 replicates this tick's camera yaw
    /// rather than the last one's.
    fn tick_player(&mut self, mut raw: RawInput) {
        let world_rc = self.world.clone();
        let blocks_rc = self.blocks.clone();
        let mut world = world_rc.borrow_mut();
        let mut blocks = blocks_rc.borrow_mut();

        let Some(rig) = blocks.player_rigs.get(&PlayerId(0)).copied() else {
            self.prompt = None;
            return;
        };

        // Ask ONCE what the button would do. The same answer draws the prompt
        // and performs the press, so the hint can never promise something
        // different from what happens.
        let subject = rig.mount.subject();
        let facing = makepad_game_sim::heading_to_forward(rig.camera.yaw);
        let found = world.entity(subject).map(|e| e.pos).and_then(|pos| {
            interact::choose(&world, &blocks, &self.interact, rig.seat(), pos, facing)
        });
        self.prompt = found
            .as_ref()
            .map(|f| format!("Press {} to {}", self.activity_glyph(), f.prompt));
        // Publish on the engine's own HUD channel rather than a private field,
        // so the affordance appears wherever a HUD is drawn instead of only in
        // whichever host happened to build it.
        world.hud_slots.retain(|(name, _)| name != "interact");
        if let Some(text) = &self.prompt {
            world.hud_slots.push((
                "interact".to_string(),
                makepad_game_sim::HudSlot {
                    text: text.clone(),
                    color: vec4(1.0, 1.0, 1.0, 0.9),
                    // An absolute point size, NOT a scale — the renderer takes
                    // any value above zero literally, so a "1.0" meant as
                    // 100% draws a one-point speck. Bigger than the 12.0
                    // default on purpose: this is the one line of text a
                    // player has to notice without being told to look.
                    size: 17.0,
                    anchor: makepad_game_sim::HudAnchor::Bottom,
                },
            ));
        }

        // Standing controls hint. Nobody sits a child down with a manual, and
        // the affordance prompt only appears once you are already next to
        // something — this is what tells you how to get there.
        world.hud_slots.retain(|(name, _)| name != "hint");
        world.hud_slots.push((
            "hint".to_string(),
            makepad_game_sim::HudSlot {
                text: if self.pad_is_active {
                    "Left stick move · Right stick look · ✕ jump".to_string()
                } else {
                    "WASD move · mouse look · space jump".to_string()
                },
                color: vec4(1.0, 1.0, 1.0, 0.55),
                size: 11.0,
                anchor: makepad_game_sim::HudAnchor::TopLeft,
            },
        ));

        if raw.use_pressed {
            match found.map(|f| f.action(self.player)) {
                // The rig owns the seat, so the press passes through to it.
                Some(InteractAction::ToggleMount) => {}
                // An interior is a pocket elsewhere in the same world, so
                // going inside is a position write — the same shape as the
                // NPCs' DoorUse, performed by the host rather than a block.
                Some(InteractAction::Teleport { entity, to }) => {
                    if let Some(e) = world.entity_mut(entity) {
                        e.pos = to;
                        e.vel = vec3f(0.0, 0.0, 0.0);
                    }
                    raw.use_pressed = false;
                }
                // Arcade has no script VM, so a declared interactable has
                // nowhere to dispatch to yet. Consumed rather than leaked to
                // the mount, which would silently put the player in a car.
                Some(InteractAction::Script { .. }) => raw.use_pressed = false,
                // Nothing in reach: a silent no-op, not a mount attempt.
                None => raw.use_pressed = false,
            }
        }

        let mut intents = HashMap::new();
        intents.insert(PlayerId(0), raw);
        blocks.tick_player_rigs(&mut world, &intents);
        // Publish the smoothed pivot and the obstruction-limited boom for the
        // renderer. Local tier: the camera never reaches the sim, so this is
        // the last thing that happens to it.
        if let Some(rig) = blocks.player_rigs.get(&PlayerId(0)) {
            rig.apply_camera(&mut world);
        }
        world.sync_local_player();
    }

    /// Name the primary activity button for the device in the player's hands.
    /// A hint that says "E" to someone holding a pad is worse than no hint.
    fn activity_glyph(&self) -> &'static str {
        if self.pad_is_active {
            "✕"
        } else {
            "E"
        }
    }

    /// Fold a headset frame into this device's input. The result is an
    /// ordinary player packet — the sim cannot tell a Quest from a laptop.
    fn apply_xr_state(&mut self, state: &makepad_platform::event::xr::XrState) {
        let intent = crate::xr_input::intent_from_xr(state);
        crate::xr_input::apply_intent_to_pad(&intent, &mut self.xr_pad);
        self.xr_head_yaw = intent.head_yaw;
        self.xr_active = true;
        // Right stick turns the world under a seated player. In MR the
        // diorama spins instead of the room, which is what "turn the track
        // to see the far corner" should feel like.
        if intent.turn.abs() > 0.0 && self.stage.mode == StageMode::MrDiorama {
            self.stage.yaw += intent.turn * 0.03;
            self.renderer.set_stage(self.stage);
        }
        let mut world = self.world.borrow_mut();
        world.pad = self.xr_pad;
        // Movement resolves against the head's forward, carried per player.
        world.cam_yaw = intent.head_yaw;
    }

    fn run_tick(&mut self) {
        let mut world = self.world.borrow_mut();
        let w = &mut *world;
        let t = w.time as f32;
        for e in w.entities.iter_mut() {
            match e.tag.as_str() {
                // Kinematic platform: glide side to side.
                "platform" => e.vel.x = makepad_game_math::cos(t * 0.7) * 5.0,
                // The jump course. Deliberately different periods, so the two
                // moving platforms drift in and out of phase instead of
                // presenting the same crossing every lap.
                "platform_x" => e.vel.x = makepad_game_math::cos(t * 0.55) * 4.2,
                "platform_y" => e.vel.y = makepad_game_math::cos(t * 0.42) * 2.6,
                // Bouncer: relaunch on landing.
                "bouncer" => {
                    if e.on_floor {
                        e.vel.y = 14.0;
                    }
                }
                // Anything that fell off the slab comes back up.
                _ => {
                    if e.kind == BodyKind::Mover && e.pos.y < -20.0 {
                        e.pos = vec3f(0.0, 12.0, 0.0);
                        e.vel = vec3f(0.0, 0.0, 0.0);
                    }
                }
            }
        }
        // The sun is FIXED. It used to sweep a full day every 40 seconds,
        // which looked lively for about ten seconds and then simply cost
        // money: AO and cast shadows are baked, and the baker rebakes
        // whenever the sun moves past `sun_rebake_angle`, so a sun crossing
        // the sky at that rate re-bakes the whole world continuously and
        // nothing on screen ever settles.
        //
        // A day cycle is still a feature worth having — it just has to be a
        // choice rather than the default, and it wants incremental rebaking
        // before it earns its place.
        if std::env::var("ARCADE_DAYCYCLE").is_ok() {
            let hours = 6.0 + (t * 0.45) % 12.0;
            w.sun = SunConfig {
                time_of_day: Some(hours),
                latitude: 52.0,
                ..Default::default()
            };
        }

        // Hard landings throw sparks. Compared against last frame's speed so
        // a resting crate does not spark forever.
        let mut impacts: Vec<Vec3f> = Vec::new();
        let mut speeds: Vec<(u64, f32)> = Vec::new();
        for e in w.entities.iter() {
            if e.kind != BodyKind::Rigid {
                continue;
            }
            let speed = e.vel.length();
            if let Some((_, was)) = self.impact_speed.iter().find(|(id, _)| *id == e.id) {
                if *was > 6.0 && speed < was * 0.45 {
                    impacts.push(e.pos);
                }
            }
            speeds.push((e.id, speed));
        }
        self.impact_speed = speeds;

        // Kick the rigid corner every 5 seconds: crates tumble, balls fly.
        if w.tick % 300 == 200 {
            let ids: Vec<u64> = w
                .entities
                .iter()
                .filter(|e| e.tag == "rigid_crate" || e.tag == "rigid_ball")
                .map(|e| e.id)
                .collect();
            for (i, id) in ids.iter().enumerate() {
                let side = if i % 2 == 0 { 1.0 } else { -1.0 };
                w.dynamics
                    .rigid_impulse(*id, vec3f(2.5 * side, 7.0, 2.0));
                w.dynamics.rigid_spin(*id, vec3f(0.6 * side, 0.9, 0.3));
            }
        }
        // Release the world before the session borrows it below — `let _ = w`
        // only drops the reborrow, not the RefMut it came from, so without
        // this the next `self.world.borrow()` panics.
        let _ = w;
        drop(world);

        // A crate hitting the slab clanks where it landed — the demo's proof
        // that positional audio is wired, not merely implemented.
        for at in &impacts {
            self.demo_audio.push(AudioRequest::SfxAt {
                name: "clank".to_string(),
                pitch: 1.0,
                at: *at,
                range: 60.0,
            });
        }

        // Particles, entirely device-local: the requests never touch the sim,
        // and the system carries its own RNG (see render/particles.rs).
        let mut requests: Vec<ParticleRequest> = Vec::new();
        for at in impacts {
            let mut spec = ParticleSpec::new(ParticleKind::Spark);
            spec.rate = 14.0;
            spec.color = vec4f(1.0, 0.8, 0.35, 1.0);
            requests.push(ParticleRequest::Burst { at, spec });
        }
        if self.particles.emitter_count() == 0 {
            // Exhaust follows the car by entity id — no sim state involved.
            // Gated on the car actually moving: a parked car standing under a
            // column of its own smoke reads as a bug, not as atmosphere.
            let driving = self
                .blocks
                .borrow()
                .cars
                .first()
                .map(|c| c.entity)
                .and_then(|id| {
                    let world = self.world.borrow();
                    makepad_game_sim::entity_index_sorted(&world.entities, id)
                        .map(|i| world.entities[i].vel.length() > 3.0)
                })
                .unwrap_or(false);
            if let Some(car) = self
                .blocks
                .borrow()
                .cars
                .first()
                .map(|c| c.entity)
                .filter(|_| driving)
            {
                let mut smoke = ParticleSpec::new(ParticleKind::Smoke);
                smoke.rate = 12.0;
                smoke.size = 0.09;
                smoke.color = vec4f(0.7, 0.7, 0.75, 0.5);
                requests.push(ParticleRequest::Emitter {
                    id: 1,
                    anchor: EmitterAnchor::Entity(car),
                    spec: smoke,
                });
            }
        }
        if !requests.is_empty() {
            self.particles.apply(&requests);
        }
        {
            let world = self.world.borrow();
            let lookup = |id: u64| {
                makepad_game_sim::entity_index_sorted(&world.entities, id)
                    .map(|i| world.entities[i].pos)
            };
            self.particles.step(TICK_DT, &lookup);
        }
        // Fireworks: the whole per-frame CPU cost is ageing a dozen shells and
        // occasionally starting one.
        self.fireworks.step(TICK_DT);
        {
        }

        let raw = self.raw_input(TICK_DT);
        self.tick_player(raw);
        // One tick, whichever role this device holds: Local and Host simulate,
        // a Client applies host truth and derives the rest.
        let now = self.world.borrow().tick as f64 * TICK_DT as f64;
        for event in self
            .session
            .tick(&mut self.world.borrow_mut(), &mut self.blocks.borrow_mut(), now)
        {
            self.session_status = match event {
                SessionEvent::Joined { name, .. } => format!("{name} joined"),
                SessionEvent::Left { name, .. } => format!("{name} left"),
                SessionEvent::Disconnected { reason } => format!("disconnected: {reason:?}"),
            };
            log!("arcade: {}", self.session_status);
        }
        // Villagers: the NPC blocks already ran inside `blocks.pre_step`, and
        // `step_world` has already turned their intent into actual positions.
        // All that is left is to read the result back into animation — facing
        // from travel, walk-cycle from speed. A villager the sweep stopped
        // against a bench therefore stops its legs too, rather than moonwalking
        // into the obstacle.
        {
            let world = self.world.borrow();
            for villager in &mut self.villagers {
                villager.follow(&world);
            }
        }
    }

    /// Read the room configuration from the environment. Kept env-driven on
    /// purpose: it is the same switch a headless test client uses, so the
    /// multiplayer path is exercised without a window.
    fn start_session(&mut self) {
        const SECRET: &[u8] = b"makepad-arcade-lan";
        if std::env::var("ARCADE_HOST").is_ok() {
            match Session::host("arcade", SECRET) {
                Ok(session) => {
                    if let Some((tcp, udp)) = session.host_addrs() {
                        log!("arcade: hosting on tcp {tcp} / udp {udp}");
                        log!("arcade: join with ARCADE_JOIN={tcp}");
                    }
                    self.session = session;
                }
                Err(e) => log!("arcade: could not host: {e}"),
            }
            return;
        }
        let Ok(addr) = std::env::var("ARCADE_JOIN") else {
            return;
        };
        let Ok(tcp) = addr.parse::<std::net::SocketAddr>() else {
            log!("arcade: ARCADE_JOIN must be host:port, got {addr}");
            return;
        };
        // The host's UDP port is its TCP port + 0 by convention only when it
        // was bound explicitly; ask for both to be passed when they differ.
        let udp: std::net::SocketAddr = std::env::var("ARCADE_JOIN_UDP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(tcp);
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        match Session::join(id, "player", tcp, udp, SECRET, 0.0) {
            Ok(session) => {
                log!("arcade: joined {tcp}");
                self.session = session;
            }
            Err(e) => log!("arcade: could not join {tcp}: {e}"),
        }
    }

    fn scene(&self, rect: Rect, time: f64) -> Option<SceneState3D> {
        // With a player, the rig owns the camera — including WHAT it follows,
        // so it tracks the car the moment you get in with no branch here.
        // view_angles() is already in the renderer's convention (which is the
        // mirror of the sim's heading convention); handing it straight over is
        // deliberate, and negating anything here would be the bug.
        let (yaw, pitch) = match self.blocks.borrow().player_rigs.get(&PlayerId(0)) {
            Some(rig) => rig.view_angles(),
            None => (self.orbit_yaw, self.orbit_pitch),
        };
        render_scene_state(
            &self.world.borrow(),
            rect,
            time,
            &CameraRig {
                yaw,
                pitch,
                in_test: false,
            },
        )
    }
}

impl WidgetNode for ArcadeView {
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

impl Widget for ArcadeView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event {
            Event::KeyDown(key) => {
                if !self.keys.contains(&key.key_code) {
                    self.keys.push(key.key_code);
                }
            }
            Event::KeyUp(key) => {
                self.keys.retain(|k| *k != key.key_code);
            }
            Event::XrUpdate(xr) => {
                let state = xr.state.clone();
                self.apply_xr_state(&state);
            }
            _ => {}
        }
        if self.next_frame.is_event(event).is_some() {
            let time = cx.seconds_since_app_start();
            let last = self.last_time.replace(time).unwrap_or(time);
            self.time_accum += (time - last).min(0.25);
            // Once per frame, not once per tick: the pad is device state, and
            // polling it twice inside one catch-up would double-count edges.
            self.poll_gamepad(cx);
            let mut ticked = false;
            while self.time_accum >= TICK_DT as f64 {
                self.time_accum -= TICK_DT as f64;
                if self.script.is_some() {
                    // Script mode: the host owns on_tick, timers and physics.
                    // The rig still runs here, because it produces the input
                    // those steps consume.
                    let raw = self.raw_input(TICK_DT);
                    self.tick_player(raw);
                    if let Some(host) = &mut self.script {
                        host.tick(cx, TICK_DT);
                    }
                } else {
                    self.run_tick();
                }
                ticked = true;
            }
            if let Some(err) = self.watch_game_file(cx, (time - last).min(0.25)) {
                // Push-back channel: the agent that made the edit needs this.
                log!("arcade: eval failed, keeping last good world:\n{err}");
            }
            // Test hook: ARCADE_CAPTURE=<path.png> grabs a GPU frame once the
            // world has settled (~2s), then the harness kills the app.
            // `>=` + take-once: the accumulator can step several ticks per
            // frame, so an exact tick compare would skip silently.
            if self.world.borrow().tick >= 120 && !self.captured_1 {
                self.captured_1 = true;
                if let Some(path) = std::env::var_os("ARCADE_CAPTURE") {
                    cx.capture_next_frame_to_file(std::path::PathBuf::from(path));
                }
            }
            // Second capture much later in the anim cycle: a different pose
            // proves the animation actually advances.
            if self.world.borrow().tick >= 300 && !self.captured_2 {
                self.captured_2 = true;
                if let Some(path) = std::env::var_os("ARCADE_CAPTURE2") {
                    cx.capture_next_frame_to_file(std::path::PathBuf::from(path));
                }
            }
            if ticked {
                self.area.redraw(cx);
            }
            self.next_frame = cx.new_next_frame();
        }

        match event {
            Event::MouseDown(me)
                if self.view_rect.contains(me.abs) && me.button.is_primary() =>
            {
                self.orbit_last_abs = Some(me.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Event::MouseMove(me) => {
                if let Some(last) = self.orbit_last_abs {
                    let delta = me.abs - last;
                    // Accumulated, not applied: the mouse reports faster than
                    // the 60Hz tick, and dropping the extra samples would make
                    // a fast flick turn less than a slow one.
                    self.look_accum += delta;
                    self.pad_is_active = false;
                    // Fallback pose for the no-player debug view.
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
                    let mut w = self.world.borrow_mut();
                    w.cam_distance = (w.cam_distance * factor).clamp(2.0, 120.0);
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
        if !self.world_built {
            log!("{}", crate::capability::Capabilities::detect().report());
            if let Some(path) = std::env::var_os("ARCADE_GAME") {
                let path = std::path::PathBuf::from(path);
                match self.load_game(cx, &path) {
                    Some(err) => log!("arcade: {} failed to eval:\n{err}", path.display()),
                    None => log!("arcade: loaded {}", path.display()),
                }
            }
        }
        if !self.pass_ready {
            self.pass_ready = true;
            // The stage is a render-side projection: telling the renderer is
            // the whole of "switch to MR", and the sim never hears about it.
            self.renderer.set_stage(self.stage);
            log!(
                "arcade: stage {:?} (scale {:.3}) — environment {}",
                self.stage.mode,
                self.stage.scale,
                if self.stage.shows_environment() {
                    "game-supplied"
                } else {
                    "the room (passthrough)"
                }
            );
            // Offscreen pass targets (same formats GameView uses).
            self.color_texture = Texture::new_with_format(
                cx.cx,
                TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            );
            self.depth_texture = Texture::new_with_format(
                cx.cx,
                TextureFormat::DepthD32 {
                    size: TextureSize::Auto,
                    initial: true,
                },
            );
            self.pass.set_color_texture(
                cx.cx,
                &self.color_texture,
                DrawPassClearColor::ClearWith(self.clear_color),
            );
            self.pass.set_depth_texture(
                cx.cx,
                &self.depth_texture,
                DrawPassClearDepth::ClearWith(1.0),
            );
            cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
            self.start_session();
            // A client's world arrives from the host; building a local one
            // would only be overwritten on the first state batch. Script mode
            // already built its world from game.splash.
            if self.session.simulates() && !self.world_built {
                self.build_world();
                // The big world spawns its own car in build_big_world and gets
                // its POIs and brains from the plan once the props exist
                // (wire_plan_blocks). Running the street's spawn_blocks too
                // gave every villager a SECOND Npc block — two brains steering
                // one body against each other — and a second car.
                if !Self::big_world_requested() {
                    self.spawn_blocks();
                }
            }
            self.next_frame = cx.new_next_frame();
        }
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
        if let Some(scene_state) = self.scene(rect, cx.time()) {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            // Stock props are uploaded once, on the first frame that has a Cx.
            if !self.props_loaded {
                self.props_loaded = true;
                // Both worlds place props the same way — models uploaded, then
                // instances and collider boxes derived from the models' own
                // bounds. They differ only in who chose the layout: the street
                // composes it inline, the big world reads a plan that was
                // computed with no Cx at all.
                self.load_vehicle_model(cx.cx);
                let (models, colliders) = if Self::big_world_requested() {
                    self.load_plan_models(cx.cx);
                    self.realise_plan()
                } else {
                    self.load_props(cx.cx);
                    let (m, c) = self.compose_village();
                    (m, c.into_iter().map(|c| (c.pos, c.half)).collect())
                };
                self.village = models;
                if self.script.is_none() {
                    let mut world = self.world.borrow_mut();
                    for (pos, half) in &colliders {
                        let id = spawn(
                            &mut world,
                            BodyKind::Static,
                            Shape::Box,
                            *pos,
                            vec3f(half.x * 2.0, half.y * 2.0, half.z * 2.0),
                            vec4(0.5, 0.5, 0.5, 1.0),
                            "scenery",
                        );
                        // The mesh is the prop's appearance; this box is only
                        // its substance.
                        if let Some(e) = world.entity_mut(id) {
                            e.hidden = true;
                        }
                    }
                    log!(
                        "arcade: {} props, {} colliders, {} triangles",
                        self.village.len(),
                        colliders.len(),
                        self.village
                            .iter()
                            .filter_map(|i| self.renderer.model_triangles(&i.model))
                            .sum::<usize>(),
                    );
                }
                // The plan's inhabitants need destinations, and those only
                // become blocks once the world exists. Doing it here rather
                // than in build_big_world keeps the ordering honest: POIs are
                // positions, but the NPCs that score them are entities.
                if Self::big_world_requested() {
                    self.wire_plan_blocks();
                }
            }
            // Character atlases are created before Cx3d mutably borrows cx.
            // One per distinct pack; the slot each character samples is
            // recorded on it, because binding the wrong atlas does not fail —
            // it renders, wrongly, and reads as a shading bug.
            {
                let mut slot = 0usize;
                for c in self.cast.iter_mut() {
                    if c.texture.is_none() {
                        match ImageBuffer::from_png(&c.texture_png) {
                            Ok(image) => c.texture = Some(image.into_new_texture(cx.cx)),
                            Err(err) => {
                                log!("arcade: {} texture failed: {:?}", c.label, err)
                            }
                        }
                    }
                    if c.texture.is_some() {
                        c.texture_slot = slot;
                        slot += 1;
                    }
                }
            }
            // The skinned character: sample → blend → palette → CPU skin.
            // Items are prepared before the draw so the batch borrows stay
            // disjoint from GameDraws.
            let mut skinned_items = Vec::new();
            let mut skinned_verts = 0usize;
            if !self.cast.is_empty() {
                // Many rigs, many poses. Each villager samples ITS OWN model's
                // clips at its own phase and skins into its own buffer: the
                // geometry key is per villager because they hold different
                // poses, and the clip indices are per MODEL because the rigs
                // name their locomotion differently.
                //
                // COST, because this is the one thing here that does not
                // scale: skinning is on the CPU, so a crowd costs the sum of
                // its characters' vertex counts EVERY FRAME. The mixed cast is
                // cheaper than the all-knight village it replaces — a Kenney
                // civilian is 1,259 verts against the knight's 3,716 — but the
                // shape of the cost is unchanged. A town, or a Quest, wants
                // GPU skinning (the bone palette is already computed here, so
                // the swap is this loop plus a shader) or impostors for the
                // distant ones.
                // While seated the player IS the car; drawing their body too
                // leaves a second copy of them standing in the road.
                let hide_player = matches!(
                    self.blocks
                        .borrow()
                        .player_rigs
                        .get(&PlayerId(0))
                        .map(|r| r.seat()),
                    Some(Seat::Driving(_))
                );
                let player_entity = self.player;
                let cast = &self.cast;
                for (i, v) in self.villagers.iter_mut().enumerate() {
                    if hide_player && v.entity == player_entity {
                        continue;
                    }
                    let Some(c) = cast.get(v.kind).filter(|c| c.texture.is_some()) else {
                        continue;
                    };
                    c.model.sample_clip(c.idle, v.clock, &mut v.pose_idle);
                    c.model.sample_clip(c.walk, v.clock, &mut v.pose_walk);
                    SkinnedModel::blend_pose(
                        &v.pose_idle,
                        &v.pose_walk,
                        v.blend,
                        &mut v.blended,
                    );
                    c.model.palette(&v.blended, &mut v.palette);
                    let mut vertices = Vec::new();
                    c.model.skin_to_packed(&v.palette, &mut vertices);
                    skinned_verts += c.model.vertex_count();
                    let mut transform = Mat4f::rotation(vec3f(0.0, v.yaw, 0.0));
                    // Build varies a little per villager. The two rigs are
                    // modelled at different heights, so normalise first or the
                    // knight towers over the townsfolk.
                    let s = v.scale * c.height_scale();
                    for k in [0usize, 1, 2, 4, 5, 6, 8, 9, 10] {
                        transform.v[k] *= s;
                    }
                    transform.v[12] = v.pos.x;
                    transform.v[13] = v.pos.y;
                    transform.v[14] = v.pos.z;
                    skinned_items.push(
                        SkinnedDraw::new(
                            i as u64 + 1,
                            vertices,
                            c.model.indices().to_vec(),
                            transform,
                        )
                        .with_tint(v.tint)
                        .with_texture(c.texture_slot),
                    );
                }
            }
            if !self.logged_skin_cost && !skinned_items.is_empty() {
                self.logged_skin_cost = true;
                log!(
                    "arcade: {} villagers of {} kinds — {} verts skinned/frame ({} KB)",
                    skinned_items.len(),
                    self.cast.len(),
                    skinned_verts,
                    skinned_verts * 6 * 4 / 1024,
                );
            }
            // Built before `batch` borrows self mutably. The static props are
            // fixed; the car's instance is rebuilt each frame from its rigid
            // body, so its mesh rides the physics rather than the other way
            // round — the chassis stays the box3d body and the model is only
            // its appearance, which is the same split the villagers use.
            let mut prop_instances = self.village.clone();
            if let Some(inst) = self.vehicle_instance() {
                prop_instances.push(inst);
            }
            let textures: Vec<&Texture> =
                self.cast.iter().filter_map(|c| c.texture.as_ref()).collect();
            let batch = if skinned_items.is_empty() || textures.is_empty() {
                None
            } else {
                Some(SkinnedBatch {
                    skinned: &mut self.draw_skinned,
                    textures,
                    items: skinned_items,
                })
            };

            self.renderer.set_particles(self.particles.instances());
            self.renderer.set_fireworks(self.fireworks.instances());
            self.renderer.set_models(prop_instances);
            let cx3d = &mut Cx3d::new(cx.cx);
            let mut draws = GameDraws {
                cube: &mut self.draw_cube,
                alpha: &mut self.draw_alpha,
                sky: &mut self.draw_sky,
                terrain: &mut self.draw_terrain,
                shadow: Some(&mut self.draw_shadow),
                firework: Some(&mut self.draw_firework),
            };
            let fw_count = self.fireworks.live_shells();
            let stats = self.renderer.draw_scene_full(
                cx3d,
                &mut self.draw_list,
                &mut draws,
                &self.world.borrow(),
                scene_state,
                batch,
                Some(&mut self.draw_models),
            );
            // One line, once: the numbers BUDGETS.md quotes come from here
            // rather than from counting struct fields by hand.
            if std::env::var("ARCADE_FW_DEBUG").is_ok() && fw_count > 0 {
                log!("firework draw: {} live shells, {} drawn", fw_count, stats.firework_shells);
            }
            if !self.logged_render_stats {
                self.logged_render_stats = true;
                log!(
                    "arcade: stock props {} instances in {} draw items, {} tris; \
                     {} shadow casters ({} projected)",
                    stats.model_instances,
                    stats.model_draws,
                    stats.model_triangles,
                    stats.shadows,
                    stats.projected_shadows
                );
                let b = stats.bake;
                log!(
                    "arcade: {} floats/instance ({} B) · bake ao {} us, sun {} us, probes {} us \
                     ({} probes, {} statics, {} occluders)",
                    stats.instance_floats,
                    stats.instance_floats * 4,
                    b.ao_us,
                    b.sun_us,
                    b.probe_us,
                    b.probes,
                    b.statics,
                    b.occluders
                );
            }
        }
        cx.end_pass(&self.pass);

        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        cx.set_pass_area(&self.pass, self.area);

        // HUD slots + gauges + crosshair. Without this the interact prompt
        // ("press E to drive") is computed every tick and never seen, so the
        // mount mechanic is only discoverable by guessing the key.
        let (slots, bars, crosshair) = {
            let world = self.world.borrow();
            (
                world.hud_slots.clone(),
                world.hud_bars.clone(),
                world.crosshair,
            )
        };
        draw_hud_overlay(
            cx,
            rect,
            &mut self.draw_hud,
            &mut self.draw_dot,
            &slots,
            &bars,
            crosshair,
        );

        // Billboard nametags: projected into the 2D overlay so they always
        // face the camera and are never occluded by geometry.
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
            if let Some(scene) = self.scene(rect, cx.time()) {
                draw_billboard_labels(cx, rect, &scene, &mut self.draw_label, &labels);
            }
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod look_tests {
    use super::*;

    /// Reported as "the camera joystick feels y inverted", and it was.
    ///
    /// Stated as agreement between the two devices rather than as a raw sign,
    /// because a sign test can be satisfied by flipping whichever end of the
    /// chain you happened to be looking at. What must hold is that pushing
    /// the stick away from you and pushing the mouse away from you do the
    /// same thing to the same camera.
    #[test]
    fn the_stick_and_the_mouse_agree_which_way_is_up() {
        // Mouse moved AWAY from the player: screen y grows downward, so the
        // delta this produces is negative.
        let mouse_up_dy = -12.0f32;
        // Stick pushed AWAY from the player: a pad reports up as +y.
        let (_, stick_up_dy) = ArcadeView::pad_look_units(0.0, 1.0, 1.0 / 60.0, false);
        assert!(
            mouse_up_dy.signum() == stick_up_dy.signum(),
            "mouse-up gives {mouse_up_dy} and stick-up gives {stick_up_dy} — the two devices \
             disagree about which way is up"
        );
    }

    /// `+look_dy` raises the camera and looks down (controller.rs), so
    /// stick-up must arrive negative for the default to be non-inverted.
    #[test]
    fn stick_up_looks_up_by_default_and_down_when_inverted() {
        let (_, normal) = ArcadeView::pad_look_units(0.0, 1.0, 1.0 / 60.0, false);
        let (_, inverted) = ArcadeView::pad_look_units(0.0, 1.0, 1.0 / 60.0, true);
        assert!(normal < 0.0, "stick-up should look up by default, got {normal}");
        assert!(inverted > 0.0, "inverted stick-up should look down, got {inverted}");
        assert_eq!(normal, -inverted, "inversion should only change the sign");
    }

    /// Horizontal is NOT inverted — only y was wrong. A fix that flipped both
    /// would trade one complaint for another.
    #[test]
    fn stick_right_still_turns_the_view_right() {
        let (dx, _) = ArcadeView::pad_look_units(1.0, 0.0, 1.0 / 60.0, false);
        assert!(dx > 0.0, "stick-right should turn right, got {dx}");
    }
}
