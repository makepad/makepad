//! ArcadeView — Makepad Arcade's first real viewport (M0 stage B).
//!
//! Proves the engine runs independent of gamemaker: a GameWorld built
//! directly through the sim API (no script VM), ticked at 60Hz, rendered by
//! makepad-game-render into an offscreen pass composited into the pane.
//! Mouse drag orbits, wheel zooms — the same raw-event pattern GameView uses.

use makepad_game_blocks::{
    Blocks, Car, CarConfig, Character, CharacterConfig, ControlSource, DriveInput,
};
use makepad_game_render::skin::{PoseBuffer, SkinnedModel};
use makepad_game_render::stage::{Stage, StageMode};
use makepad_game_render::{
    scene_state as render_scene_state, set_pass_camera, CameraRig, DrawGameAlpha, DrawGameCube,
    DrawGameSkinned, DrawGameSky, DrawGameTerrain, DrawGameTexture, GameDraws, GameRenderer,
    SkinnedBatch, SkinnedDraw,
};
use makepad_game_session::{Session, SessionEvent};
use makepad_game_sim::{BodyKind, Entity, GameWorld, PadState, Shape, SkyConfig, TICK_DT};
use makepad_widgets::*;
use makepad_game_script::ScriptHost;
use std::cell::RefCell;
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
    draw_skinned: DrawGameSkinned,
    #[live(vec4(0.03, 0.045, 0.075, 1.0))]
    clear_color: Vec4f,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[rust]
    knight: Option<Knight>,
    /// Engine-side blocks: the drivable car and the patrolling character.
    #[rust(Rc::new(RefCell::new(Blocks::new())))]
    blocks: Rc<RefCell<Blocks>>,
    /// Held keys for the local player (arrow keys / WASD drive the car).
    #[rust]
    keys: KeySet,
    #[rust]
    knight_texture: Option<Texture>,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    area: Area,
    #[rust]
    renderer: GameRenderer,
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
    world.entities.push(Entity {
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

/// The stock skinned character (KayKit Knight, CC0) + its animation state.
/// Assets are fetched by apps/arcade/download_assets.sh — everything here
/// degrades gracefully when they're absent.
struct Knight {
    model: SkinnedModel,
    texture_png: Vec<u8>,
    idle: usize,
    walk: usize,
    pose_idle: PoseBuffer,
    pose_walk: PoseBuffer,
    blended: PoseBuffer,
    palette: Vec<Mat4f>,
    idle_time: f32,
    walk_time: f32,
    /// 0 = idle, 1 = walking — eased toward the patrol state each tick.
    blend: f32,
    angle: f32,
    pos: Vec3f,
    yaw: f32,
}

impl Knight {
    fn load() -> Option<Knight> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/characters");
        let glb = match std::fs::read(format!("{dir}/knight.glb")) {
            Ok(bytes) => bytes,
            Err(_) => {
                log!("arcade: no skinned character — run apps/arcade/download_assets.sh");
                return None;
            }
        };
        let texture_png = std::fs::read(format!("{dir}/knight_texture.png")).ok()?;
        let model = match SkinnedModel::parse_glb(&glb) {
            Ok(model) => model,
            Err(err) => {
                log!("arcade: knight.glb failed to parse: {err}");
                return None;
            }
        };
        let idle = model.clip_index("idle")?;
        let walk = model
            .clip_index("walking_a")
            .or_else(|| model.clip_index("walk"))?;
        log!(
            "arcade: knight loaded — {} joints, {} verts, {} clips",
            model.joint_count(),
            model.vertex_count(),
            model.clips.len()
        );
        Some(Knight {
            model,
            texture_png,
            idle,
            walk,
            pose_idle: PoseBuffer::new(),
            pose_walk: PoseBuffer::new(),
            blended: PoseBuffer::new(),
            palette: Vec::new(),
            idle_time: 0.0,
            walk_time: 0.0,
            blend: 0.0,
            angle: 0.0,
            pos: vec3f(10.0, 0.0, 0.0),
            yaw: 0.0,
        })
    }

    /// Patrol brain: walk the pillar ring for 6s, stand for 2s. Facing follows
    /// the walk direction (the visual-only yaw convention); animation state is
    /// Derived-tier — recomputed from motion, never authoritative.
    fn tick(&mut self) {
        let dt = TICK_DT;
        let cycle = self.idle_time + self.walk_time; // monotonic clock
        let phase = cycle - (cycle / 8.0).floor() * 8.0;
        let walking = phase < 6.0;
        let target = if walking { 1.0 } else { 0.0 };
        self.blend += (target - self.blend) * 0.08;
        if walking {
            self.angle += dt * 0.22;
            // Between the props (cone at r≈12) and the pillar ring (r=18).
            let radius = 15.0;
            self.pos = vec3f(
                makepad_game_math::cos(self.angle) * radius,
                0.0,
                makepad_game_math::sin(self.angle) * radius,
            );
            // Tangent of the circle — the direction we're moving in.
            let dir = vec3f(
                -makepad_game_math::sin(self.angle),
                0.0,
                makepad_game_math::cos(self.angle),
            );
            self.yaw = makepad_game_math::atan2(dir.x, dir.z);
        }
        self.idle_time += dt;
        self.walk_time += dt;
    }
}

impl ArcadeView {
    /// Switch to script-driven mode: a `game.splash` owns the world.
    /// Returns the eval error, if the first eval failed.
    pub fn load_game(&mut self, cx: &mut Cx, path: &std::path::Path) -> Option<String> {
        let source = std::fs::read_to_string(path).ok()?;
        let mut host = ScriptHost::new();
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
        w.sky = Some(SkyConfig::default());
        w.cam_target = vec3f(0.0, 3.0, 0.0);
        w.cam_distance = 34.0;

        // Ground slab.
        spawn(
            w,
            BodyKind::Static,
            Shape::Box,
            vec3f(0.0, -0.5, 0.0),
            vec3f(60.0, 1.0, 60.0),
            vec4(0.55, 0.62, 0.5, 1.0),
            "ground",
        );
        // Pillar ring.
        for i in 0..8 {
            let a = std::f32::consts::TAU * i as f32 / 8.0;
            let hue = i as f32 / 8.0;
            spawn(
                w,
                BodyKind::Static,
                Shape::Cylinder,
                vec3f(a.cos() * 18.0, 3.0, a.sin() * 18.0),
                vec3f(1.4, 6.0, 1.4),
                vec4(0.4 + 0.5 * hue, 0.45, 0.85 - 0.5 * hue, 1.0),
                "pillar",
            );
        }
        // A ramp, a cone, a glowing beacon sphere.
        spawn(
            w,
            BodyKind::Static,
            Shape::Wedge,
            vec3f(-8.0, 1.0, 4.0),
            vec3f(6.0, 2.0, 8.0),
            vec4(0.75, 0.55, 0.35, 1.0),
            "ramp",
        );
        spawn(
            w,
            BodyKind::Static,
            Shape::Cone,
            vec3f(9.0, 1.5, 8.0),
            vec3f(3.0, 3.0, 3.0),
            vec4(0.9, 0.45, 0.3, 1.0),
            "cone",
        );
        let beacon = spawn(
            w,
            BodyKind::Static,
            Shape::Sphere,
            vec3f(0.0, 9.0, 0.0),
            vec3f(1.6, 1.6, 1.6),
            vec4(1.0, 0.85, 0.3, 1.0),
            "beacon",
        );
        if let Some(e) = w.entity_mut(beacon) {
            e.glow = 2.0;
        }
        // Moving platform (kinematic, driven every tick below).
        spawn(
            w,
            BodyKind::Kinematic,
            Shape::Box,
            vec3f(0.0, 2.5, -8.0),
            vec3f(6.0, 0.6, 3.0),
            vec4(0.35, 0.6, 0.8, 1.0),
            "platform",
        );
        // Falling movers: land, rest, cast blob shadows.
        for i in 0..6 {
            let x = -6.0 + i as f32 * 2.4;
            spawn(
                w,
                BodyKind::Mover,
                if i % 2 == 0 { Shape::Box } else { Shape::Sphere },
                vec3f(x, 6.0 + i as f32 * 2.0, -2.0 + (i % 3) as f32 * 2.0),
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
            vec3f(6.0, 4.0, -6.0),
            vec3f(1.4, 1.4, 1.4),
            vec4(0.3, 0.85, 0.5, 1.0),
            "bouncer",
        );
        // Rigid-body corner (M1a): a crate stack + two balls on real box3d
        // dynamics. The tick loop kicks the stack periodically to show off
        // impulses/tumbling; between kicks it settles and sleeps.
        for i in 0..6 {
            let id = spawn(
                w,
                BodyKind::Rigid,
                Shape::Box,
                vec3f(-9.0 + (i as f32) * 0.04, 0.55 + i as f32 * 1.05, -9.0),
                vec3f(1.0, 1.0, 1.0),
                vec4(0.9, 0.6 - 0.06 * i as f32, 0.2, 1.0),
                "rigid_crate",
            );
            if let Some(e) = w.entity_mut(id) {
                e.restitution = 0.05;
            }
        }
        for i in 0..2 {
            let id = spawn(
                w,
                BodyKind::Rigid,
                Shape::Sphere,
                vec3f(-6.0 + i as f32 * 1.6, 5.0, -10.0),
                vec3f(1.1, 1.1, 1.1),
                vec4(0.4, 0.5, 0.95, 1.0),
                "rigid_ball",
            );
            if let Some(e) = w.entity_mut(id) {
                e.restitution = 0.5;
                e.friction = 0.4;
            }
        }
        self.knight = Knight::load();
        self.world_built = true;
    }

    /// The demo's "game logic" — what a script or blocks component will do
    /// later, done directly in Rust here.
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
            pos: vec3f(-3.0, 2.0, -4.0),
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
        blocks.cars.push(Car::new(
            car,
            CarConfig::default(),
            ControlSource::Player,
        ));

        // The Knight already exists as a mover; give it a character block so
        // its walk animation is driven by real velocity.
        let knight_entity = w
            .entities
            .iter()
            .find(|e| e.tag == "knight")
            .map(|e| e.id);
        if let Some(id) = knight_entity {
            blocks.characters.push(Character::new(
                id,
                CharacterConfig::default(),
                ControlSource::Script,
                Some("Knight".to_string()),
            ));
        }
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
        if let Some(knight) = &mut self.knight {
            knight.tick();
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
            // Knight texture is created before Cx3d mutably borrows cx.
            if let Some(knight) = &self.knight {
                if self.knight_texture.is_none() {
                    match ImageBuffer::from_png(&knight.texture_png) {
                        Ok(image) => {
                            self.knight_texture = Some(image.into_new_texture(cx.cx))
                        }
                        Err(err) => log!("arcade: knight texture failed: {:?}", err),
                    }
                }
            }
            // The skinned character: sample → blend → palette → CPU skin.
            // Items are prepared before the draw so the batch borrows stay
            // disjoint from GameDraws.
            let mut skinned_items = Vec::new();
            if let Some(knight) = &mut self.knight {
                if self.knight_texture.is_some() {
                    knight
                        .model
                        .sample_clip(knight.idle, knight.idle_time, &mut knight.pose_idle);
                    knight
                        .model
                        .sample_clip(knight.walk, knight.walk_time, &mut knight.pose_walk);
                    SkinnedModel::blend_pose(
                        &knight.pose_idle,
                        &knight.pose_walk,
                        knight.blend,
                        &mut knight.blended,
                    );
                    knight.model.palette(&knight.blended, &mut knight.palette);
                    let mut vertices = Vec::new();
                    knight.model.skin_to_pbr(&knight.palette, &mut vertices);
                    let mut transform = Mat4f::rotation(vec3f(0.0, knight.yaw, 0.0));
                    transform.v[12] = knight.pos.x;
                    transform.v[13] = knight.pos.y;
                    transform.v[14] = knight.pos.z;
                    skinned_items.push(SkinnedDraw {
                        key: 1,
                        vertices,
                        indices: knight.model.indices().to_vec(),
                        transform,
                    });
                }
            }
            let batch = match (&mut self.draw_skinned, &self.knight_texture) {
                (skinned, Some(texture)) if !skinned_items.is_empty() => Some(SkinnedBatch {
                    skinned,
                    texture,
                    items: skinned_items,
                }),
                _ => None,
            };

            let cx3d = &mut Cx3d::new(cx.cx);
            let mut draws = GameDraws {
                cube: &mut self.draw_cube,
                alpha: &mut self.draw_alpha,
                sky: &mut self.draw_sky,
                terrain: &mut self.draw_terrain,
            };
            self.renderer.draw_scene_full(
                cx3d,
                &mut self.draw_list,
                &mut draws,
                &self.world.borrow(),
                scene_state,
                batch,
            );
        }
        cx.end_pass(&self.pass);

        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        cx.set_pass_area(&self.pass, self.area);
        DrawStep::done()
    }
}
