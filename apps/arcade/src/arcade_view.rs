//! ArcadeView — Makepad Arcade's first real viewport (M0 stage B).
//!
//! Proves the engine runs independent of gamemaker: a GameWorld built
//! directly through the sim API (no script VM), ticked at 60Hz, rendered by
//! makepad-game-render into an offscreen pass composited into the pane.
//! Mouse drag orbits, wheel zooms — the same raw-event pattern GameView uses.

use makepad_game_blocks::{
    Blocks, Car, CarConfig, ControlSource, DriveInput, Npc, NpcConfig, Poi, PoiSet,
};
use makepad_game_render::skin::{PoseBuffer, SkinnedModel};
use makepad_game_render::particles::ParticleSystem;
use makepad_game_render::stage::{Stage, StageMode};
use makepad_game_render::{
    scene_state as render_scene_state, set_pass_camera, CameraRig, DrawGameAlpha, DrawGameCube,
    DrawGameShadow, DrawGameSkinned, DrawGameSky, DrawGameTerrain, DrawGameTexture, GameDraws,
    GameRenderer, ModelInstance, SkinnedBatch, SkinnedDraw,
};
use makepad_game_session::{Session, SessionEvent};
use makepad_game_sim::{
    BodyKind, EmitterAnchor, Entity, GameWorld, PadState, ParticleKind, ParticleRequest,
    ParticleSpec, Shape, SkyConfig, SunConfig, TICK_DT,
};
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
    #[rust(0.7f32)]
    orbit_yaw: f32,
    #[rust(-0.35f32)]
    orbit_pitch: f32,
    #[rust]
    orbit_last_abs: Option<DVec2>,
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
    tag: &'static str,
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

        // One hero among the townsfolk, on the other rig entirely — which is
        // what makes the multi-rig path real rather than theoretical.
        if let Some(c) = CharacterModel::load(
            &format!("{chars_dir}/knight.glb"),
            &format!("{chars_dir}/knight_texture.png"),
            "knight",
        ) {
            cast.push(c);
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

    fn build_world(&mut self) {
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

        // Ground slab.
        spawn(
            w,
            BodyKind::Static,
            Shape::Box,
            vec3f(0.0, -0.5, 0.0),
            vec3f(64.0, 1.0, 64.0),
            vec4(0.46, 0.56, 0.36, 1.0),
            "ground",
        );
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
        self.world_built = true;
    }

    /// The demo's "game logic" — what a script or blocks component will do
    /// later, done directly in Rust here.
    /// Load the stock props this demo places, choosing them BY DESCRIPTION
    /// through the asset index — the same path a generated game takes, so the
    /// demo exercises what Fable will actually use rather than a private
    /// shortcut with hardcoded paths.
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
                    tag: "scenery",
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
                    tag: "scenery",
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
        w.mark_render_dirty();
        blocks.cars.push(Car::new(
            car,
            CarConfig::default(),
            ControlSource::Player,
        ));

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

    /// Local player's intent from the keyboard: arrows/WASD steer and drive.
    fn player_input(&self) -> DriveInput {
        let down = |k: KeyCode| self.keys.contains(&k);
        let steer = (down(KeyCode::ArrowRight) || down(KeyCode::KeyD)) as i8 as f32
            - (down(KeyCode::ArrowLeft) || down(KeyCode::KeyA)) as i8 as f32;
        let throttle = (down(KeyCode::ArrowUp) || down(KeyCode::KeyW)) as i8 as f32
            - (down(KeyCode::ArrowDown) || down(KeyCode::KeyS)) as i8 as f32;
        // A headset drives the same player as the keyboard: whichever moved
        // wins, so picking up a controller mid-session just works.
        let (xr_steer, xr_throttle) = if self.xr_active {
            (self.xr_pad.axis_x as f32, -self.xr_pad.axis_z as f32)
        } else {
            (0.0, 0.0)
        };
        DriveInput {
            steer: if steer != 0.0 { steer } else { xr_steer },
            throttle: if throttle != 0.0 {
                throttle
            } else {
                xr_throttle
            },
            brake: (down(KeyCode::Space) as i8 as f32).max(self.xr_pad.jump as i8 as f32),
            ..Default::default()
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
        // Cycle the sun across the day so the unified lighting and the
        // projected shadows are visibly doing something: a full 24h in 40s.
        let hours = 6.0 + (t * 0.45) % 12.0;
        w.sun = SunConfig {
            time_of_day: Some(hours),
            latitude: 52.0,
            ..Default::default()
        };

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

        let player_input = self.player_input();
        self.blocks.borrow_mut().player_input = player_input;
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
        render_scene_state(
            &self.world.borrow(),
            rect,
            time,
            &CameraRig {
                yaw: self.orbit_yaw,
                pitch: self.orbit_pitch,
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
            let mut ticked = false;
            while self.time_accum >= TICK_DT as f64 {
                self.time_accum -= TICK_DT as f64;
                if self.script.is_some() {
                    // Script mode: the host owns on_tick, timers and physics.
                    let input = self.player_input();
                    if let Some(host) = &mut self.script {
                        host.blocks.borrow_mut().player_input = input;
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
                self.spawn_blocks();
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
                self.load_props(cx.cx);
                // Compose once, then spawn a collider per solid prop. Doing
                // it here (not in build_world) is forced by ordering: the
                // footprints come from model bounds, which only exist after
                // the GLBs are loaded, which needs a Cx.
                let (models, colliders) = self.compose_village();
                self.village = models;
                if self.script.is_none() {
                    let mut world = self.world.borrow_mut();
                    for c in &colliders {
                        let id = spawn(
                            &mut world,
                            BodyKind::Static,
                            Shape::Box,
                            c.pos,
                            vec3f(c.half.x * 2.0, c.half.y * 2.0, c.half.z * 2.0),
                            vec4(0.5, 0.5, 0.5, 1.0),
                            c.tag,
                        );
                        // The mesh is the prop's appearance; this box is only
                        // its substance.
                        if let Some(e) = world.entity_mut(id) {
                            e.hidden = true;
                        }
                    }
                    log!(
                        "arcade: village {} props, {} colliders",
                        self.village.len(),
                        colliders.len()
                    );
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
                let cast = &self.cast;
                for (i, v) in self.villagers.iter_mut().enumerate() {
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
            // Built before `batch` borrows self mutably.
            let prop_instances = self.village.clone();
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
            self.renderer.set_models(prop_instances);
            let cx3d = &mut Cx3d::new(cx.cx);
            let mut draws = GameDraws {
                cube: &mut self.draw_cube,
                alpha: &mut self.draw_alpha,
                sky: &mut self.draw_sky,
                terrain: &mut self.draw_terrain,
                shadow: Some(&mut self.draw_shadow),
            };
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
        DrawStep::done()
    }
}
