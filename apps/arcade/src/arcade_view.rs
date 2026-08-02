//! ArcadeView — Makepad Arcade's first real viewport (M0 stage B).
//!
//! Proves the engine runs independent of gamemaker: a GameWorld built
//! directly through the sim API (no script VM), ticked at 60Hz, rendered by
//! makepad-game-render into an offscreen pass composited into the pane.
//! Mouse drag orbits, wheel zooms — the same raw-event pattern GameView uses.

use makepad_game_render::skin::{PoseBuffer, SkinnedModel};
use makepad_game_render::{
    scene_state as render_scene_state, set_pass_camera, CameraRig, DrawGameAlpha, DrawGameCube,
    DrawGameSkinned, DrawGameSky, DrawGameTerrain, DrawGameTexture, GameDraws, GameRenderer,
    SkinnedBatch, SkinnedDraw,
};
use makepad_game_sim::{step_world, BodyKind, Entity, GameWorld, Shape, SkyConfig, TICK_DT};
use makepad_widgets::*;

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
    #[rust]
    world: GameWorld,
    #[rust(false)]
    world_built: bool,
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
    fn build_world(&mut self) {
        let w = &mut self.world;
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
        self.knight = Knight::load();
        self.world_built = true;
    }

    /// The demo's "game logic" — what a script or blocks component will do
    /// later, done directly in Rust here.
    fn run_tick(&mut self) {
        let w = &mut self.world;
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
        step_world(w);
        w.tick += 1;
        w.time += TICK_DT as f64;
        if let Some(knight) = &mut self.knight {
            knight.tick();
        }
    }

    fn scene(&self, rect: Rect, time: f64) -> Option<SceneState3D> {
        render_scene_state(
            &self.world,
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
        if self.next_frame.is_event(event).is_some() {
            let time = cx.seconds_since_app_start();
            let last = self.last_time.replace(time).unwrap_or(time);
            self.time_accum += (time - last).min(0.25);
            let mut ticked = false;
            while self.time_accum >= TICK_DT as f64 {
                self.time_accum -= TICK_DT as f64;
                self.run_tick();
                ticked = true;
            }
            // Test hook: ARCADE_CAPTURE=<path.png> grabs a GPU frame once the
            // world has settled (~2s), then the harness kills the app.
            if self.world.tick == 120 {
                if let Some(path) = std::env::var_os("ARCADE_CAPTURE") {
                    cx.capture_next_frame_to_file(std::path::PathBuf::from(path));
                }
            }
            // Second capture much later in the anim cycle: a different pose
            // proves the animation actually advances.
            if self.world.tick == 300 {
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
                    self.world.cam_distance =
                        (self.world.cam_distance * factor).clamp(2.0, 120.0);
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
            self.build_world();
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
                &self.world,
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
