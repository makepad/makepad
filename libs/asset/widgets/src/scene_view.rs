//! The two renderer-backed faces of a preview well: a mesh on a turntable,
//! and a world that walks itself.
//!
//! Both are the same widget because they are the same machine — an offscreen
//! 3D pass composited into the panel, a retained renderer, a fixed-step tick
//! — differing only in what the camera is doing. A prop is framed and turned
//! on a plinth. A level is loaded at its AUTHORED scale (a Doom map squashed
//! to 1.75 units is not walkable) and toured first-person by the walker in
//! [`crate::walk_world`], opening doors as it goes.
//!
//! Bytes in, never an asset id: the host resolved the catalog and hands over
//! a GLB. Parsing and the expensive nav-grid build happen on a worker this
//! widget owns, because a real map is seconds of capsule probes and the
//! frame thread cannot have them.
//!
//! Behind the `renderer` feature.

use crate::walk_world::{build_level, WalkMoment, WalkPrep, WalkWorld};
use makepad_render::level::{BobStyle, WalkerConfig};
use makepad_render::player_nav::NavAnchor;
use makepad_render::{
    preview_scene_state, set_pass_camera, DrawSceneAlpha, DrawSceneCube, DrawSceneShadow,
    DrawSceneSkinned, DrawSceneSky, DrawSceneTerrain, DrawSceneTexture, ModelInstance,
    PreviewLook, PreviewStage, Renderer, SceneDraws, StaticModel, TICK_DT,
};
use makepad_widgets::*;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

/// First-person clip planes for a walked level (world units; the classic
/// importer's scale is Doom map units / 64, so 0.05 is about three map
/// units). The shared preview near plane of 0.15 is half a Doom step, so a
/// wall a body radius away can still clip through it.
const WALK_NEAR: f32 = 0.05;
const WALK_FAR: f32 = 500.0;

/// What a well is being asked to show in 3D.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SceneMode {
    /// Frame it and turn it.
    #[default]
    Turntable,
    /// Walk it.
    World,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SceneViewBase = #(SceneView::register_widget(vm))
    mod.widgets.SceneView = set_type_default() do mod.widgets.SceneViewBase{
        width: Fill
        height: Fill
        draw_cube +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_alpha +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_terrain +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_models +: { light_dir: vec3(0.35, 0.8, 0.45) }
    }
}

/// What the worker parsed out of the GLB bytes.
struct Parsed {
    glb: Vec<u8>,
    texture_png: Option<Vec<u8>>,
    prep: Option<WalkPrep>,
    error: Option<String>,
}

/// A mesh turntable, or an autonomous world walkthrough.
#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct SceneView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawSceneTexture,
    #[live]
    draw_cube: DrawSceneCube,
    #[live]
    draw_alpha: DrawSceneAlpha,
    #[live]
    draw_sky: DrawSceneSky,
    #[live]
    draw_terrain: DrawSceneTerrain,
    #[live]
    draw_shadow: DrawSceneShadow,
    #[live]
    draw_models: DrawSceneSkinned,
    #[live(vec4(0.015, 0.02, 0.04, 1.0))]
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
    #[rust]
    renderer: Renderer,
    #[rust]
    look: PreviewLook,
    #[rust]
    mode: SceneMode,
    /// Head-bob feel of the level being loaded.
    #[rust]
    bob_style: BobStyle,
    /// Manifest anchors of the level being loaded (player start, keys, exit,
    /// doors). Empty for every world published before the anchor lane.
    #[rust]
    anchors: Vec<NavAnchor>,
    /// A parse + nav build in flight on the worker.
    #[rust]
    parsing: Option<Receiver<Parsed>>,
    /// Parsed bytes waiting for a `Cx` to upload with.
    #[rust]
    pending: Option<Box<Parsed>>,
    #[rust]
    statue: Option<ModelInstance>,
    #[rust]
    statue_yaw: f32,
    #[rust]
    tour: Option<WalkWorld>,
    /// View roll of the current tick (Quake tilts as the view swings); the
    /// preview camera has no roll input, so this view builds its own.
    #[rust]
    view_roll: f32,
    /// Doom's teleport white-out, counted down after every cut. While it
    /// burns the pass is CLEARED to white and the scene is not drawn — the
    /// flash has to live in the pass texture.
    #[rust]
    view_flash: f32,
    #[rust]
    load_count: u64,
    /// Status line a host can print beside the well.
    #[rust]
    pub status: String,
    #[rust(0.6f32)]
    orbit_yaw: f32,
    #[rust(-0.18f32)]
    orbit_pitch: f32,
    #[rust]
    orbit_last_abs: Option<DVec2>,
    #[rust]
    view_rect: Rect,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time_accum: f64,
    #[rust]
    last_time: Option<f64>,
    /// Does this well's picture reach a surface anyone is looking at? A
    /// walked level costs ~90 collision rays a tick plus a pass render a
    /// frame, and only the host knows whether it is on screen. Dormant keeps
    /// every bit of state — position, map memory, pose — and stops the clock.
    #[rust(true)]
    live: bool,
}

impl SceneView {
    /// Show a mesh, turning. Bytes in.
    pub fn show_mesh(&mut self, cx: &mut Cx, glb: Vec<u8>, texture_png: Option<Vec<u8>>) {
        self.start(cx, glb, texture_png, SceneMode::Turntable);
    }

    /// Show a world, walked. `source` names the engine the map came from
    /// (a tag, a path, whatever the host knows) so the walker gets that
    /// engine's gait; anything unrecognised gets Doom's.
    pub fn show_world(
        &mut self,
        cx: &mut Cx,
        glb: Vec<u8>,
        texture_png: Option<Vec<u8>>,
        source: &str,
        anchors: Vec<NavAnchor>,
    ) {
        self.bob_style = BobStyle::from_source(source);
        self.anchors = anchors;
        self.start(cx, glb, texture_png, SceneMode::World);
    }

    /// Stop everything and forget the model.
    pub fn clear(&mut self, cx: &mut Cx) {
        self.parsing = None;
        self.pending = None;
        self.statue = None;
        self.tour = None;
        self.anchors.clear();
        self.renderer.set_models(Vec::new());
        self.status.clear();
        self.area.redraw(cx);
    }

    /// Whether the well is on screen. A dormant well keeps its state and
    /// stops its clock.
    pub fn set_live(&mut self, cx: &mut Cx, live: bool) {
        if self.live == live {
            return;
        }
        self.live = live;
        self.last_time = None;
        self.time_accum = 0.0;
        if live {
            self.next_frame = cx.new_next_frame();
            self.area.redraw(cx);
        }
    }

    pub fn has_model(&self) -> bool {
        self.statue.is_some()
            || self.tour.is_some()
            || self.pending.is_some()
            || self.parsing.is_some()
    }

    /// True while the world is actually being toured (rather than orbited
    /// because it had no walkable floor).
    pub fn is_walking(&self) -> bool {
        self.tour.is_some()
    }

    /// A one-line coverage report of the tour, for a host that traces.
    pub fn coverage(&self) -> Option<String> {
        self.tour.as_ref().map(|t| t.coverage())
    }

    fn start(&mut self, cx: &mut Cx, glb: Vec<u8>, texture_png: Option<Vec<u8>>, mode: SceneMode) {
        self.mode = mode;
        self.statue = None;
        self.tour = None;
        self.pending = None;
        self.status = "loading…".to_string();
        // Parsing, and above all the nav grid, run off the frame thread:
        // a real map is a capsule probe per cell and a wall probe per edge.
        let (tx, rx) = channel();
        let cfg = WalkerConfig::for_style(self.bob_style);
        self.parsing = Some(rx);
        std::thread::Builder::new()
            .name("asset-widgets-scene".into())
            .spawn(move || {
                let parsed = match StaticModel::parse_glb(&glb) {
                    Ok(model) => {
                        let prep = match mode {
                            // The grid is built with the SAME body that will
                            // walk it — building for one set of legs and
                            // walking with another is how a tour ends up
                            // refusing steps it can make.
                            SceneMode::World => Some(build_level(&model, &glb, &cfg)),
                            SceneMode::Turntable => None,
                        };
                        Parsed { glb, texture_png, prep, error: None }
                    }
                    Err(e) => Parsed {
                        glb: Vec::new(),
                        texture_png: None,
                        prep: None,
                        error: Some(format!("mesh parse failed: {e}")),
                    },
                };
                let _ = tx.send(parsed);
            })
            .ok();
        self.next_frame = cx.new_next_frame();
        self.area.redraw(cx);
    }

    fn collect_parse(&mut self) {
        let Some(rx) = self.parsing.as_ref() else { return };
        match rx.try_recv() {
            Ok(parsed) => {
                self.parsing = None;
                self.pending = Some(Box::new(parsed));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.parsing = None;
                self.status = "mesh load failed".to_string();
            }
        }
    }

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.look.target = vec3f(0.0, 0.9, 0.0);
        self.look.distance = 4.6;
        self.look.fov = 45.0;
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
        );
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 { size: TextureSize::Auto, initial: true },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
        self.next_frame = cx.new_next_frame();
    }

    /// Upload what the worker parsed. Texture creation and the renderer's own
    /// loader need a `Cx`, so this is the one part that waits for a draw.
    fn load_pending(&mut self, cx: &mut Cx) {
        let Some(parsed) = self.pending.take() else { return };
        if let Some(error) = parsed.error {
            self.status = error;
            return;
        }
        self.load_count += 1;
        let name = format!("asset-widgets/model-{}", self.load_count);
        let png = parsed.texture_png;
        let tris = match self.renderer.load_model(cx, &name, &parsed.glb, png.as_deref()) {
            Ok(tris) => tris,
            Err(e) => {
                self.status = format!("mesh load failed: {e}");
                return;
            }
        };
        let (min, max) = self
            .renderer
            .model_bounds(&name)
            .unwrap_or((vec3f(0.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0)));
        match (self.mode, parsed.prep) {
            (SceneMode::World, Some(prep)) => self.load_world(name, min, max, prep),
            _ => {
                // Framed and turned: normalise to human height so a chair and
                // a castle both read at the same size in the same panel.
                let height = (max.y - min.y).max(0.01);
                let scale = 1.75 / height;
                self.statue = Some(ModelInstance {
                    model: name,
                    transform: trs_yaw(vec3f(0.0, -min.y * scale, 0.0), 0.0, scale),
                    dynamic: false,
                    depth_order: 0.0,
                });
                self.status = format!("{tris} triangles");
            }
        }
        self.area.redraw(cx);
    }

    /// A walkable level: identity transform (authored scale), triangle
    /// collision and nav graph built on the worker, and a tourist dropped on
    /// an interior floor.
    fn load_world(&mut self, name: String, min: Vec3f, max: Vec3f, prep: WalkPrep) {
        self.statue = Some(ModelInstance {
            model: name.clone(),
            transform: trs_yaw(vec3f(0.0, 0.0, 0.0), 0.0, 1.0),
            dynamic: false,
            depth_order: 0.0,
        });
        // A level's door parts are animated nodes, NOT part of the static
        // collision mesh, so their cells are walkable in the graph and the
        // tour opens them on approach. `anim_part_boxes` is world space and
        // the level's transform is identity, so it needs no mapping.
        let doors: Vec<(String, (Vec3f, Vec3f))> = self
            .renderer
            .anim_part_boxes()
            .into_iter()
            .filter(|p| p.model == name)
            .map(|p| (p.part, (p.min, p.max)))
            .collect();
        let triangles = prep.level.as_ref().map(|l| l.triangles()).unwrap_or(0);
        let cfg = WalkerConfig::for_style(self.bob_style);
        // Seeded by the load, so one tour is repeatable while two wells
        // showing the same map do not walk in lockstep.
        let seed = self.load_count.wrapping_mul(0x9e37);
        let anchors = std::mem::take(&mut self.anchors);
        match WalkWorld::new(name, prep, cfg, &anchors, doors, seed) {
            Some(tour) => {
                self.status = format!(
                    "walking {:.0}×{:.0} level ({} triangles, {} nav cells, {} doors)",
                    max.x - min.x,
                    max.z - min.z,
                    tour.triangles(),
                    tour.nav_cells(),
                    tour.doors()
                );
                self.tour = Some(tour);
            }
            None => {
                // Nowhere to stand — a sealed shell, or a mesh with no
                // floors. An honest orbit, never a stuck camera.
                self.tour = None;
                self.status = format!("no walkable floor ({triangles} triangles) — orbiting");
                self.orbit_level(min, max);
            }
        }
    }

    /// Frame the whole level from outside (the no-floor fallback).
    fn orbit_level(&mut self, min: Vec3f, max: Vec3f) {
        self.look.target = vec3f(
            (min.x + max.x) * 0.5,
            (min.y + max.y) * 0.5,
            (min.z + max.z) * 0.5,
        );
        self.look.distance = (max.x - min.x).max(max.z - min.z).max(1.0) * 0.8;
    }

    /// One fixed step. The camera IS the walker: the orbit rig is placed so
    /// its lens sits at the walker's eye looking along its heading
    /// (`preview_scene_state` puts the camera at `target - forward *
    /// distance`, with a 0.5 floor on distance).
    fn tick(&mut self) {
        // ~0.12 s of white, the length of Doom's teleport fog flash.
        self.view_flash = (self.view_flash - TICK_DT * 8.0).max(0.0);
        let Some(tour) = self.tour.as_mut() else {
            self.statue_yaw += TICK_DT * 0.5;
            return;
        };
        let WalkMoment { eye, yaw, roll, flash } = tour.tick(TICK_DT, &mut self.renderer);
        if flash {
            self.view_flash = 1.0;
        }
        self.look.target = eye + makepad_render::level::yaw_forward(yaw) * 0.5;
        self.look.distance = 0.5;
        self.look.fov = 75.0;
        self.orbit_yaw = yaw;
        self.orbit_pitch = 0.0;
        // Smooth the roll so a heading change eases in and out.
        self.view_roll += (roll - self.view_roll) * 0.12;
    }

    /// The preview camera's look-at uses a fixed world up, so a rolled view
    /// needs its own scene state: same eye and target, up vector tilted about
    /// the view direction.
    fn rolled_scene_state(&self, rect: Rect, time: f64) -> Option<SceneState3D> {
        let mut state = preview_scene_state(self.look, rect, time)?;
        if self.tour.is_none() {
            return Some(state);
        }
        let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
        state.projection =
            Mat4f::perspective(self.look.fov.clamp(20.0, 120.0), aspect, WALK_NEAR, WALK_FAR);
        if self.view_roll.abs() < 1e-4 {
            return Some(state);
        }
        let forward = (self.look.target - state.camera_pos).normalize();
        let right = vec3f(self.look.yaw.cos(), 0.0, self.look.yaw.sin());
        let up = vec3f(0.0, 1.0, 0.0) * self.view_roll.cos() + right * self.view_roll.sin();
        state.view = Mat4f::look_at(state.camera_pos, state.camera_pos + forward, up);
        Some(state)
    }

    fn draw_scene(&mut self, cx: &mut Cx3d, scene_state: SceneState3D) {
        // Turntable: retained renderer state, re-posed every frame. A walked
        // level tours itself and must not also spin, so its instance keeps
        // the identity transform it was loaded with.
        let models = match (&self.statue, self.tour.is_some()) {
            (Some(inst), false) => {
                let mut inst = inst.clone();
                let base = inst.transform;
                let scale = (base.v[0] * base.v[0] + base.v[1] * base.v[1] + base.v[2] * base.v[2])
                    .sqrt()
                    .max(0.0001);
                inst.transform =
                    trs_yaw(vec3f(base.v[12], base.v[13], base.v[14]), self.statue_yaw, scale);
                vec![inst]
            }
            (Some(inst), true) => vec![inst.clone()],
            (None, _) => Vec::new(),
        };
        self.renderer.set_models(models);
        let mut draws = SceneDraws {
            cube: &mut self.draw_cube,
            alpha: &mut self.draw_alpha,
            sky: &mut self.draw_sky,
            sky_analytic: None,
            terrain: &mut self.draw_terrain,
            shadow: Some(&mut self.draw_shadow),
            shadow_sdf: None,
            firework: None,
            flare: None,
            water: None,
            screen: None,
            screen_instances: &[],
            view_model: None,
        };
        let stage = if self.tour.is_some() {
            // A level brings its own floor: a plinth slab would slice through
            // it at eye height. Keep the sky for open-air maps.
            let mut stage = PreviewStage::empty();
            stage.sky = true;
            stage
        } else {
            let mut stage = PreviewStage::statue();
            stage.ground_half = 9.0;
            stage.ground_color = vec4(0.10, 0.11, 0.14, 1.0);
            stage
        };
        self.renderer.draw_preview(
            cx,
            &mut self.draw_list,
            &mut draws,
            self.look,
            stage,
            scene_state,
            None,
            Some(&mut self.draw_models),
        );
    }
}

/// yaw-rotation * uniform scale, translated.
fn trs_yaw(pos: Vec3f, yaw: f32, scale: f32) -> Mat4f {
    let mut m = Mat4f::rotation(vec3f(0.0, yaw, 0.0));
    for k in [0usize, 1, 2, 4, 5, 6, 8, 9, 10] {
        m.v[k] *= scale;
    }
    m.v[12] = pos.x;
    m.v[13] = pos.y;
    m.v[14] = pos.z;
    m
}

impl WidgetNode for SceneView {
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

impl Widget for SceneView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            let waiting = self.parsing.is_some();
            self.collect_parse();
            if self.live {
                let time = cx.seconds_since_app_start();
                let last = self.last_time.replace(time).unwrap_or(time);
                self.time_accum += (time - last).min(0.25);
                let mut ticked = false;
                while self.time_accum >= TICK_DT as f64 {
                    self.time_accum -= TICK_DT as f64;
                    self.tick();
                    ticked = true;
                }
                // A tour that moved has to ask for its own frame: the pass is
                // only re-rendered when this view draws.
                if ticked && self.has_model() {
                    self.area.redraw(cx);
                }
            }
            if waiting && self.pending.is_some() {
                self.area.redraw(cx);
            }
            if self.live || self.parsing.is_some() {
                self.next_frame = cx.new_next_frame();
            }
        }
        // Orbit + zoom on the panel. A walked level drives its own camera, so
        // dragging one would fight the tour for a frame and snap back.
        if self.tour.is_some() {
            return;
        }
        match event {
            Event::MouseDown(me) if self.view_rect.contains(me.abs) && me.button.is_primary() => {
                self.orbit_last_abs = Some(me.abs);
            }
            Event::MouseMove(me) => {
                if let Some(last) = self.orbit_last_abs {
                    let delta = me.abs - last;
                    self.orbit_yaw -= delta.x as f32 * 0.01;
                    self.orbit_pitch =
                        (self.orbit_pitch + delta.y as f32 * 0.01).clamp(-1.45, 1.45);
                    self.orbit_last_abs = Some(me.abs);
                    self.area.redraw(cx);
                }
            }
            Event::MouseUp(me) if me.button.is_primary() => {
                self.orbit_last_abs = None;
            }
            Event::Scroll(se) if self.view_rect.contains(se.abs) => {
                let axis = if se.scroll.y.abs() > f64::EPSILON { se.scroll.y } else { se.scroll.x };
                if axis.abs() > f64::EPSILON {
                    let factor = if axis > 0.0 { 1.0 / 0.92 } else { 0.92 };
                    self.look.distance = (self.look.distance * factor).clamp(1.5, 30.0);
                    self.area.redraw(cx);
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if !self.has_model() || rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return DrawStep::done();
        }
        self.ensure_initialized(cx.cx);
        self.load_pending(cx.cx);
        self.view_rect = rect;
        if self.tour.is_some() && self.look.distance != 0.5 {
            // A freshly loaded level draws before its first tick.
            self.tick();
        }
        self.pass.set_size(cx, rect.size);
        let flashing = self.view_flash > 0.0;
        let clear = match flashing {
            true => vec4(1.0, 1.0, 1.0, 1.0),
            false => self.clear_color,
        };
        self.pass
            .set_color_texture(cx, &self.color_texture, DrawPassClearColor::ClearWith(clear));
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        // begin_pass copies the PARENT pass rect into the child; the size has
        // to be re-asserted after it or the texture takes the window's.
        self.pass.set_size(cx, rect.size);
        self.look.yaw = self.orbit_yaw;
        self.look.pitch = self.orbit_pitch;
        if let Some(scene_state) = self.rolled_scene_state(rect, cx.time()) {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            if !flashing {
                let cx3d = &mut Cx3d::new(cx.cx);
                self.draw_scene(cx3d, scene_state);
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

impl SceneViewRef {
    pub fn show_mesh(&self, cx: &mut Cx, glb: Vec<u8>, texture_png: Option<Vec<u8>>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_mesh(cx, glb, texture_png);
        }
    }

    pub fn show_world(
        &self,
        cx: &mut Cx,
        glb: Vec<u8>,
        texture_png: Option<Vec<u8>>,
        source: &str,
        anchors: Vec<NavAnchor>,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.show_world(cx, glb, texture_png, source, anchors);
        }
    }

    pub fn clear(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear(cx);
        }
    }

    pub fn set_live(&self, cx: &mut Cx, live: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_live(cx, live);
        }
    }

    pub fn status(&self) -> String {
        self.borrow().map(|inner| inner.status.clone()).unwrap_or_default()
    }
}
