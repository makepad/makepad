//! The 3D program slot: one animated (skinned) GLB on a minimal stage,
//! looping its `dance` clip (fallback: first clip), orbit camera.
//!
//! Trimmed adaptation of the static-prop Renderer embedding
//! (offscreen 3D child pass composited into the pane by `DrawSceneTexture`),
//! with ai-content's embedded base-color extraction so generated GLBs need
//! no sidecar texture. Unskinned GLBs fall back to the static-prop path and
//! turntable slowly instead of dancing. Loading is bytes-in (the verified
//! cache path is read on a worker; see `main.rs`), deferred to the next
//! draw because texture upload needs a `Cx`.

use crate::media::PreparedMesh;
use makepad_render::level::{
    BobStyle, LevelCollision, LevelWalker, NavGrid, WalkerConfig, WalkerEvent,
};
use makepad_render::player_nav::{NavAnchor, PlayerNav};
use makepad_render::skin::{PoseBuffer, SkinnedModel};
use makepad_render::{
    preview_scene_state, set_pass_camera, DrawSceneAlpha, DrawSceneCube,
    DrawSceneShadow, DrawSceneSkinned, DrawSceneSkinnedGpu, DrawSceneSky, DrawSceneTerrain,
    DrawSceneTexture, SceneDraws, Renderer, ModelInstance, PreviewLook, PreviewStage,
    SkinnedBatch, SkinnedDraw, TICK_DT,
};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.VjMeshViewBase = #(VjMeshView::register_widget(vm))
    mod.widgets.VjMeshView = set_type_default() do mod.widgets.VjMeshViewBase{
        width: Fill
        height: Fill
        draw_cube +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_alpha +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_terrain +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_skinned +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_models +: { light_dir: vec3(0.35, 0.8, 0.45) }
    }
}

/// PNG or JPEG bytes into a texture, falling back to 1×1 white so an
/// untextured mesh still draws.
fn image_texture(cx: &mut Cx, bytes: Option<Vec<u8>>) -> Texture {
    if let Some(bytes) = bytes {
        let decoded = if bytes.starts_with(&[0xff, 0xd8]) {
            ImageBuffer::from_jpg(&bytes).ok()
        } else {
            ImageBuffer::from_png(&bytes).ok()
        };
        if let Some(image) = decoded {
            return image.into_new_texture(cx);
        }
    }
    Texture::new_with_format(
        cx,
        TextureFormat::VecBGRAu8_32 {
            width: 1,
            height: 1,
            data: Some(vec![0xffff_ffff]),
            updated: TextureUpdated::Full,
        },
    )
}

/// yaw-rotation * uniform scale, translated (the sandbox transform idiom).
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

struct Dancer {
    model: SkinnedModel,
    texture: Texture,
    rig: u64,
    clip: usize,
    clip_time: f32,
    pose: PoseBuffer,
    palette: Vec<Mat4f>,
    scale: f32,
    lift: f32,
}

/// A walkable level loaded at its authored scale, plus the NPC touring it.
/// Collision is the level's own triangles (see `makepad_render::level`);
/// the instance transform is identity for a map, so model space IS world.
struct WalkWorld {
    walker: LevelWalker,
    level: Box<LevelCollision>,
    /// Route graph over the whole map. Without it the walker falls back to
    /// scoring headings locally, which paces one corridor forever.
    nav: Option<Box<NavGrid>>,
    // player_nav: the player-behaviour planner — rooms, the entry
    // look-around, unexplored-exit choices, backtracking, door requests
    // and the hazard-never rule. It steers; the walker keeps doing all
    // locomotion. None only when the level has no nav grid at all (the
    // walker then falls back to its built-in frontier tour).
    player: Option<PlayerNav>,
    /// Where the tour restarts from when the walker strands itself.
    home: Vec3f,
    /// Renderer model id of the level, for door commands.
    model: String,
    /// Door parts in the order `NavGrid::mark_doors` was given them, so a
    /// walker's door index names a part again.
    doors: Vec<String>,
    /// Consecutive ticks the walker reported no floor under its feet. One
    /// bad probe (a downward ray that catches a wall triangle's top edge)
    /// must not cost the tour everything it has learned about the map.
    stranded_ticks: u32,
    /// Trace bookkeeping: seconds since the last coverage line.
    trace: bool,
    since_trace: f32,
    goals_logged: usize,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct VjMeshView {
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
    draw_skinned: DrawSceneSkinnedGpu,
    #[live]
    draw_models: DrawSceneSkinned,
    #[live(vec4(0.015, 0.02, 0.04, 1.0))]
    clear_color: Vec4f,
    /// Blit the offscreen pass onto this widget. Slot views leave this
    /// off so VideoProgram composites their private texture.
    #[live(true)]
    composite: bool,
    /// Stage + sky. Overlay slots turn this off so only the model has
    /// coverage and the clear stays transparent.
    #[live(true)]
    stage: bool,
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
    #[rust(false)]
    scene_built: bool,
    #[rust]
    renderer: Renderer,
    #[rust]
    look: PreviewLook,
    /// Worker-prepared mesh queued by the app; consumed on the next draw
    /// (texture/rig upload needs Cx). Parsing/measuring/AO already happened
    /// off-thread.
    #[rust]
    pending: Option<Box<PreparedMesh>>,
    #[rust]
    dancer: Option<Dancer>,
    /// Static fallback statue for unskinned GLBs.
    #[rust]
    statue: Option<ModelInstance>,
    #[rust]
    statue_yaw: f32,
    /// Set before `set_prepared` when the asset is a walkable level
    /// (catalog kind `World` + a GLB): the mesh is then loaded at its
    /// authored scale and toured by an NPC walker instead of being shrunk
    /// to 1.75 units and turned on a plinth.
    #[rust]
    world_mode: bool,
    /// Head-bob feel for the next level (set with `set_world_mode`).
    #[rust]
    bob_style: BobStyle,
    // player_nav: manifest anchors of the NEXT level (player_start, keys,
    // exit, doors). Set by the host before `set_prepared`; empty on every
    // classic map published before the anchor lane.
    #[rust]
    world_anchors: Vec<NavAnchor>,
    #[rust]
    tour: Option<WalkWorld>,
    /// View roll of the current tick (Quake tilts as the view swings); the
    /// preview camera has no roll input, so the slot builds its own.
    #[rust]
    view_roll: f32,
    /// Doom's teleport white-out, counted down after every cut. While it
    /// burns the pass is CLEARED to white and the scene is not drawn — the
    /// flash has to live in the pass texture, because a slot's picture is
    /// that texture and never touches this widget's own area.
    #[rust]
    view_flash: f32,
    /// Monotonic ids: every load gets fresh renderer residency keys.
    #[rust(1u64)]
    next_rig: u64,
    #[rust]
    load_count: u64,
    /// Status line the app renders next to the pane.
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
    /// A parked (held) slot keeps its last pose instead of dancing on.
    #[rust]
    paused: bool,
    /// Does this view's picture reach a surface anyone is looking at? A
    /// walked level costs ~90 collision rays per tick plus a full pass
    /// render per frame, and the host (`sync_mesh_liveness`) is the only
    /// thing that knows whether the program or a cue well is showing it.
    /// Dormant keeps every bit of state — position, map memory, pose — and
    /// simply stops the clock.
    #[rust(true)]
    live: bool,
}

/// First-person clip planes for a walked level (world units; the classic
/// importer's scale is Doom map units / 64, so 0.05 ≈ 3 map units).
const WALK_NEAR: f32 = 0.05;
const WALK_FAR: f32 = 500.0;

/// Offscreen size for a slot mesh. Independent of the widget rect so a
/// tiny hidden view still produces a program-resolution texture, and so
/// two overlapping models never share a depth buffer.
const SLOT_PASS: DVec2 = dvec2(1280.0, 720.0);

impl VjMeshView {
    /// Queue a worker-prepared mesh; the next draw uploads and shows it.
    pub fn set_prepared(&mut self, cx: &mut Cx, prepared: Box<PreparedMesh>) {
        self.pending = Some(prepared);
        self.status = "uploading mesh…".to_string();
        self.area.redraw(cx);
    }

    /// Present the NEXT loaded mesh as a walkable level (see `world_mode`).
    /// `source` is the asset's alias/namespace, which picks the engine's
    /// head-bob feel. Call before [`Self::set_prepared`].
    pub fn set_world_mode(&mut self, world: bool, source: &str) {
        self.world_mode = world;
        self.bob_style = BobStyle::from_source(source);
    }

    // player_nav: hand over the NEXT level's manifest anchors (converted by
    // the host — `player_start`, `key_*`, `exit`, height hints). Call
    // between `set_world_mode` and `set_prepared`; stale anchors are
    // consumed by the next world load.
    pub fn set_world_anchors(&mut self, anchors: Vec<NavAnchor>) {
        self.world_anchors = anchors;
    }

    /// Where the NPC currently stands, for a status line.
    pub fn walker_pos(&self) -> Option<Vec3f> {
        self.tour.as_ref().map(|w| w.walker.feet())
    }

    pub fn color_texture(&self) -> Texture {
        self.color_texture.clone()
    }

    pub fn has_mesh(&self) -> bool {
        self.dancer.is_some() || self.statue.is_some() || self.pending.is_some()
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.pending = None;
        self.dancer = None;
        self.statue = None;
        self.tour = None;
        self.world_anchors = Vec::new();
        self.world_mode = false;
        self.paused = false;
        self.status.clear();
        self.area.redraw(cx);
    }

    /// Turn the simulation clock on or off (see the `live` field). Waking a
    /// dormant view re-arms its frame request and drops the elapsed wall
    /// time on the floor, so the tour continues from where it stood instead
    /// of fast-forwarding through however long it was hidden.
    pub fn set_live(&mut self, cx: &mut Cx, live: bool) {
        if self.live == live {
            return;
        }
        self.live = live;
        if live {
            self.last_time = None;
            self.time_accum = 0.0;
            self.next_frame = cx.new_next_frame();
            self.area.redraw(cx);
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Host-driven orbit (drag on the cue well): radians per pixel applied
    /// like the pane's own drag handler.
    pub fn orbit_by(&mut self, cx: &mut Cx, dx: f32, dy: f32) {
        self.orbit_yaw -= dx * 0.01;
        self.orbit_pitch = (self.orbit_pitch + dy * 0.01).clamp(-1.45, 1.45);
        self.area.redraw(cx);
    }

    /// Host-driven zoom (wheel on the cue well): same curve as the pane.
    pub fn zoom_by(&mut self, cx: &mut Cx, axis: f64) {
        if axis.abs() > f64::EPSILON {
            let factor = if axis > 0.0 { 1.0 / 0.92 } else { 0.92 };
            self.look.distance = (self.look.distance * factor).clamp(1.5, 30.0);
            self.area.redraw(cx);
        }
    }

    /// Animation tracks of the loaded dancer (empty for statues/none).
    pub fn clip_names(&self) -> Vec<String> {
        self.dancer
            .as_ref()
            .map(|d| d.model.clips.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default()
    }

    pub fn clip_index(&self) -> Option<usize> {
        self.dancer.as_ref().map(|d| d.clip)
    }

    pub fn set_clip(&mut self, cx: &mut Cx, index: usize) {
        if let Some(d) = self.dancer.as_mut() {
            if index < d.model.clips.len() {
                d.clip = index;
                d.clip_time = 0.0;
                self.area.redraw(cx);
            }
        }
    }

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
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
            DrawPassClearColor::ClearWith(self.pass_clear_color()),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
        self.next_frame = cx.new_next_frame();
    }

    /// A dark reflective stage: one slab with its top at y=0, sky on.
    /// Overlay slots skip the slab and sky so the pass clear stays
    /// transparent and only the model writes color+depth.
    fn ensure_scene(&mut self) {
        if self.scene_built {
            return;
        }
        self.scene_built = true;
        self.look.target = vec3f(0.0, 0.9, 0.0);
        self.look.distance = 4.6;
        self.look.fov = 45.0;
    }

    fn pass_clear_color(&self) -> Vec4f {
        if self.stage {
            self.clear_color
        } else {
            vec4(0.0, 0.0, 0.0, 0.0)
        }
    }

    fn load_pending(&mut self, cx: &mut Cx) {
        let Some(prepared) = self.pending.take() else { return };
        let step = crate::media::UiStep::new(match *prepared {
            PreparedMesh::Skinned { .. } => "mesh upload (skinned)",
            PreparedMesh::Statue { .. } => "mesh upload (static/world)",
        });
        self.load_count += 1;
        match *prepared {
            PreparedMesh::Skinned { model, rest, clip, scale, lift, base_color } => {
                // GPU-only: texture create + rest-bundle upload; every CPU
                // step (parse, measure, flat AO) ran on the decode worker.
                let texture = image_texture(cx, base_color);
                let rig = self.next_rig;
                self.next_rig += 1;
                self.renderer.upload_skin_rig(cx, rig, rest);
                self.status = format!(
                    "dancing: clip '{}' of {:?}",
                    model.clips[clip].name,
                    model.clips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
                );
                self.statue = None;
                self.renderer.set_models(Vec::new());
                self.dancer = Some(Dancer {
                    model: *model,
                    texture,
                    rig,
                    clip,
                    clip_time: 0.0,
                    pose: PoseBuffer::new(),
                    palette: Vec::new(),
                    scale,
                    lift,
                });
            }
            PreparedMesh::Statue { model, base_color, level, nav, start } => {
                // GPU-only: the GLB was parsed on the decode worker (30ms of
                // a 35ms Doom level), so this is the buffer/texture upload.
                let name = format!("vj/statue-{}", self.load_count);
                let png = base_color;
                let parse = crate::media::UiStep::new("renderer.load_model_parsed (upload)");
                let loaded = self
                    .renderer
                    .load_model_parsed(cx, &name, *model, png.as_deref(), None);
                parse.done(cx);
                match loaded {
                    Ok(_tris) => {
                        let (min, max) = self
                            .renderer
                            .model_bounds(&name)
                            .unwrap_or((vec3f(0.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0)));
                        self.dancer = None;
                        self.statue_yaw = 0.0;
                        if self.world_mode {
                            self.load_world(name, min, max, level, nav, start);
                        } else {
                            let height = (max.y - min.y).max(0.01);
                            let scale = 1.75 / height;
                            self.tour = None;
                            self.statue = Some(ModelInstance {
                                model: name,
                                transform: trs_yaw(vec3f(0.0, -min.y * scale, 0.0), 0.0, scale),
                                dynamic: false,
                                depth_order: 0.0,
                            });
                            self.status =
                                "static mesh (no animation clips) — turntable".to_string();
                        }
                        log!("VjMeshView: statue loaded ({})", self.load_count);
                    }
                    Err(e) => {
                        self.status = format!("mesh load failed: {e}");
                        log!("VjMeshView: {}", self.status);
                    }
                }
            }
        }
        step.done(cx);
        self.area.redraw(cx);
    }

    /// A walkable level: identity transform (authored scale — a Doom map
    /// squashed to 1.75 units is not walkable), TRIANGLE collision built on
    /// the decode worker, and an NPC dropped on an interior floor.
    ///
    /// The catalog carries no player start today (classic worlds publish
    /// `anchors: []`), so the start is found by probing the geometry: a
    /// floor with a ceiling over it and room to walk.
    fn load_world(
        &mut self,
        name: String,
        min: Vec3f,
        max: Vec3f,
        level: Option<Box<LevelCollision>>,
        mut nav: Option<Box<NavGrid>>,
        start: Option<Vec3f>,
    ) {
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
        if let Some(nav) = nav.as_mut() {
            let boxes: Vec<(Vec3f, Vec3f)> = doors.iter().map(|(_, b)| *b).collect();
            if !boxes.is_empty() {
                nav.mark_doors(&boxes);
            }
        }
        let doors: Vec<String> = doors.into_iter().map(|(p, _)| p).collect();
        // Body, gait and gravity of the source engine.
        let cfg = WalkerConfig::for_style(self.bob_style);
        let trace = std::env::var_os("VJ_WALKER_TRACE").is_some();
        let Some(level) = level else {
            self.tour = None;
            self.status = "level has no collision — orbiting".to_string();
            self.orbit_level(min, max);
            return;
        };
        // Seeded by the load, so one tour is repeatable while two
        // slots showing the same map do not walk in lockstep.
        let seed = self.load_count.wrapping_mul(0x9e37) ^ level.triangles() as u64;
        // player_nav: the player-behaviour planner over the door-marked
        // grid. Its anchored start (when the manifest carries one) beats
        // the grid's best guess; without a grid the walker's built-in tour
        // keeps the picture alive.
        let anchors = std::mem::take(&mut self.world_anchors);
        let player =
            nav.as_deref().and_then(|g| PlayerNav::new(g, &level, &cfg, &anchors, seed));
        let (start, start_yaw) = match player.as_ref() {
            Some(p) => {
                let (feet, yaw) = p.start_hint();
                (Some(feet), yaw)
            }
            None => (start, 0.0),
        };
        if trace {
            if let Some(p) = player.as_ref() {
                log!(
                    "player: {} rooms, {} portals, {} anchors",
                    p.graph().rooms(),
                    p.graph().portals(),
                    anchors.len()
                );
            }
        }
        match start {
            Some(start) => {
                self.status = format!(
                    "walking {:.0}×{:.0} level ({} triangles)",
                    max.x - min.x,
                    max.z - min.z,
                    level.triangles()
                );
                let reachable = nav
                    .as_ref()
                    .and_then(|g| g.cell_at(start).map(|c| g.reachable_from(c)))
                    .unwrap_or(0);
                log!(
                    "VjMeshView: walking level, {} triangles, {} nav cells ({} reachable, {} doors), start {:.2},{:.2},{:.2}",
                    level.triangles(),
                    nav.as_ref().map(|g| g.len()).unwrap_or(0),
                    reachable,
                    doors.len(),
                    start.x,
                    start.y,
                    start.z
                );
                if trace {
                    let floor = level.floor_below(
                        vec3f(start.x, start.y + cfg.step_up, start.z),
                        cfg.step_up + cfg.fall_limit,
                    );
                    let ceiling = level.ceiling_above(vec3f(start.x, start.y + 0.02, start.z), 8.0);
                    log!("walker: start floor {floor:?} ceiling {ceiling:?} bounds {min:?}..{max:?}");
                    if let Some(g) = nav.as_ref() {
                        let (nx, nz) = g.dims();
                        log!(
                            "walker: nav grid {nx}×{nz} columns @ {:.2} units, {} cells, {} edges",
                            g.cell_size(),
                            g.len(),
                            g.edge_count()
                        );
                    }
                }
                self.tour = Some(WalkWorld {
                    walker: LevelWalker::new(start, start_yaw, cfg, seed),
                    level,
                    nav,
                    player,
                    home: start,
                    model: name,
                    doors,
                    stranded_ticks: 0,
                    trace,
                    since_trace: 0.0,
                    goals_logged: 0,
                });
            }
            None => {
                // Nowhere to stand (a sealed shell, or a mesh with no
                // floors): honest fallback to an orbit, never a stuck cam.
                self.tour = None;
                self.status = format!(
                    "level has no walkable floor ({} triangles) — orbiting",
                    level.triangles()
                );
                log!("VjMeshView: no interior start in {} triangles", level.triangles());
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

    /// One fixed step of the level tour. The camera IS the walker: the orbit
    /// rig is placed so its lens sits at the walker's eye looking along its
    /// heading (`preview_scene_state` puts the camera at
    /// `target - forward * distance`, with a 0.5 floor on distance).
    fn run_walk_tick(&mut self) {
        // ~0.12 s of white, the length of Doom's teleport fog flash.
        self.view_flash = (self.view_flash - TICK_DT * 8.0).max(0.0);
        let Some(w) = self.tour.as_mut() else { return };
        // player_nav: the player's mind runs first — rooms, look-around
        // pans, which exit leads somewhere new, door requests, cuts. It
        // steers the walker through the level.rs seam; every step below
        // (locomotion, the door dance, the flash) is unchanged and also
        // serves the built-in tour when no planner exists.
        if let (Some(p), Some(grid)) = (w.player.as_mut(), w.nav.as_deref()) {
            if let Some(moment) = p.steer(TICK_DT, &mut w.walker, grid, &w.level) {
                if w.trace {
                    log!("player: {moment:?}");
                }
            }
        }
        if w.walker.tick_in(TICK_DT, &w.level, w.nav.as_deref()) == WalkerEvent::Stranded {
            w.stranded_ticks += 1;
            // Half a second of genuinely nothing underfoot: cut back to the
            // start. `relocate` keeps everything the tour has learned —
            // rebuilding the walker threw away the whole map memory, and one
            // unlucky probe frame then reset the tour to zero.
            if w.stranded_ticks >= 30 {
                w.stranded_ticks = 0;
                let (home, yaw) = (w.home, w.walker.yaw());
                w.walker.relocate(home, yaw);
            }
        } else {
            w.stranded_ticks = 0;
        }
        // A door the route wants: drive the part, then tell the walker it
        // may walk through once the part has settled on "open".
        if let Some(d) = w.walker.wanted_door() {
            if let Some(part) = w.doors.get(d as usize).cloned() {
                let settled = self
                    .renderer
                    .model_part_state(w.model.as_str(), &part)
                    .is_some_and(|s| s.state_name == "open" && s.settled);
                if settled {
                    w.walker.set_door_open(d, true);
                } else {
                    self.renderer.set_model_state(w.model.as_str(), &part, "open", 0.6);
                }
            } else {
                // No such part (a grid marked from stale boxes): never let
                // the tour stand there waiting for a door that is not real.
                w.walker.set_door_open(d, true);
            }
        }
        self.renderer.tick_model_states(TICK_DT);
        // A cut (teleporter, or leaving a wing the tour has finished) reads
        // as a mistake without it: the camera would simply be somewhere else.
        if w.walker.take_flash() {
            self.view_flash = 1.0;
        }
        if w.trace {
            w.since_trace += TICK_DT;
            if w.since_trace >= 5.0 {
                w.since_trace = 0.0;
                // player_nav: the room-level picture, alongside the
                // walker's cell-level one below.
                if let Some(p) = w.player.as_ref() {
                    let ps = p.stats();
                    log!(
                        "player: room {:?}, visited {}/{} rooms ({} portals), legs {}, doors {}, jams {}, cuts {}, restarts {}, epoch {}, route left {}",
                        ps.room,
                        ps.visited,
                        ps.rooms,
                        ps.portals,
                        ps.legs,
                        ps.doors_opened,
                        ps.jammed,
                        ps.region_cuts,
                        ps.restarts,
                        ps.epoch,
                        ps.route_left
                    );
                }
                let s = w.walker.nav_stats();
                let fresh: Vec<u32> =
                    w.walker.nav_goal_log().iter().skip(w.goals_logged).copied().collect();
                w.goals_logged += fresh.len();
                log!(
                    "walker: distinct {}/{} cells, reachable {} ({} unseen), goal {:?} ({} waypoints left), replans {}, dropped edges {}, cuts {}, new goals {:?}",
                    s.distinct,
                    s.cells,
                    s.reachable,
                    s.unseen,
                    s.goal,
                    s.path_left,
                    s.replans,
                    s.invalidated,
                    s.cuts,
                    fresh
                );
            }
        }
        let pose = w.walker.camera();
        let (eye, yaw, roll) = (pose.eye, pose.yaw, pose.roll);
        let forward = makepad_render::level::yaw_forward(yaw);
        self.look.target = eye + forward * 0.5;
        self.look.distance = 0.5;
        self.look.fov = 75.0;
        self.orbit_yaw = yaw;
        self.orbit_pitch = 0.0;
        // Smooth the roll so a heading change eases in and out.
        self.view_roll += (roll - self.view_roll) * 0.12;
    }

    /// The preview camera's look-at uses a fixed world up, so a rolled view
    /// needs its own scene state: same eye/target, up vector tilted about
    /// the view direction.
    fn rolled_scene_state(&self, rect: Rect, time: f64) -> Option<SceneState3D> {
        let mut state = preview_scene_state(self.look, rect, time)?;
        if self.tour.is_none() {
            return Some(state);
        }
        // First person indoors: the shared preview near plane (0.15) is
        // half a Doom step, so a wall a body radius away can still clip.
        // 0.05 with a D32 depth buffer keeps the whole map z-fight free.
        let aspect = (rect.size.x / rect.size.y).max(0.001) as f32;
        state.projection = Mat4f::perspective(self.look.fov.clamp(20.0, 120.0), aspect, WALK_NEAR, WALK_FAR);
        if self.view_roll.abs() < 1e-4 {
            return Some(state);
        }
        let forward = (self.look.target - state.camera_pos).normalize();
        let right = vec3f(self.look.yaw.cos(), 0.0, self.look.yaw.sin());
        let up = vec3f(0.0, 1.0, 0.0) * self.view_roll.cos()
            + right * self.view_roll.sin();
        state.view = Mat4f::look_at(state.camera_pos, state.camera_pos + forward, up);
        Some(state)
    }

    fn draw_scene(&mut self, cx: &mut Cx3d, scene_state: SceneState3D) {
        let mut items = Vec::new();
        let mut textures: Vec<&Texture> = Vec::new();
        if let Some(d) = self.dancer.as_mut() {
            d.model.sample_clip(d.clip, d.clip_time, &mut d.pose);
            d.model.palette(&d.pose, &mut d.palette);
            items.push(
                SkinnedDraw::new(1, d.rig, trs_yaw(vec3f(0.0, d.lift, 0.0), 0.5, d.scale))
                    .with_texture(0)
                    .with_bounds(d.model.posed_bounds(&d.palette))
                    .with_palette(d.palette.clone()),
            );
            textures.push(&d.texture);
        }
        let batch = if items.is_empty() {
            None
        } else {
            Some(SkinnedBatch { skinned: &mut self.draw_skinned, textures, items })
        };
        // Statue turntable: retained renderer state, updated every frame.
        let statue = match &self.statue {
            Some(inst) => {
                let mut inst = inst.clone();
                let base = inst.transform;
                let scale = (base.v[0] * base.v[0] + base.v[1] * base.v[1] + base.v[2] * base.v[2])
                    .sqrt()
                    .max(0.0001);
                inst.transform = trs_yaw(
                    vec3f(base.v[12], base.v[13], base.v[14]),
                    self.statue_yaw,
                    scale,
                );
                vec![inst]
            }
            None => Vec::new(),
        };
        self.renderer.set_models(statue);
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
            // A level brings its own floor: a plinth slab would slice
            // through it at eye height. Keep the sky for open-air maps.
            let mut stage = PreviewStage::empty();
            stage.sky = self.stage;
            stage
        } else if self.stage {
            let mut stage = PreviewStage::statue();
            stage.ground_half = 9.0;
            stage.ground_color = vec4(0.10, 0.11, 0.14, 1.0);
            stage
        } else {
            PreviewStage::empty()
        };
        self.renderer.draw_preview(
            cx,
            &mut self.draw_list,
            &mut draws,
            self.look,
            stage,
            scene_state,
            batch,
            Some(&mut self.draw_models),
        );
    }
}

impl WidgetNode for VjMeshView {
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

impl Widget for VjMeshView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() && self.live {
            let time = cx.seconds_since_app_start();
            let last = self.last_time.replace(time).unwrap_or(time);
            self.time_accum += (time - last).min(0.25);
            let mut ticked = false;
            while self.time_accum >= TICK_DT as f64 {
                self.time_accum -= TICK_DT as f64;
                if self.paused {
                    continue;
                }
                if let Some(d) = self.dancer.as_mut() {
                    // sample_clip wraps by duration: this IS the loop.
                    d.clip_time += TICK_DT;
                }
                if self.tour.is_some() {
                    // A level tours itself; it must not also spin.
                    self.run_walk_tick();
                } else {
                    self.statue_yaw += TICK_DT * 0.5;
                }
                ticked = true;
            }
            // A tour that moved has to ask for its own frame: the pass is
            // only re-rendered when this view draws, and nothing else on the
            // console is obliged to redraw for it.
            if ticked && (self.dancer.is_some() || self.statue.is_some() || self.tour.is_some()) {
                self.area.redraw(cx);
            }
            self.next_frame = cx.new_next_frame();
        }
        // Orbit + zoom on the pane (raw events; the composited quad takes no
        // part in finger capture).
        match event {
            Event::MouseDown(me)
                if self.composite
                    && self.view_rect.contains(me.abs)
                    && me.button.is_primary() =>
            {
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
            Event::Scroll(se) if self.composite && self.view_rect.contains(se.abs) => {
                let axis =
                    if se.scroll.y.abs() > f64::EPSILON { se.scroll.y } else { se.scroll.x };
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
        let pass_rect = if self.composite && rect.size.x > 1.0 && rect.size.y > 1.0 {
            rect
        } else {
            Rect {
                pos: dvec2(0.0, 0.0),
                size: SLOT_PASS,
            }
        };
        if !self.has_mesh() {
            return DrawStep::done();
        }
        self.ensure_initialized(cx.cx);
        self.ensure_scene();
        self.load_pending(cx.cx);
        self.view_rect = rect;
        if self.tour.is_some() && self.look.distance != 0.5 {
            // A freshly loaded level draws before its first tick.
            self.run_walk_tick();
        }
        self.pass.set_size(cx, pass_rect.size);
        let flashing = self.view_flash > 0.0;
        let clear = match flashing {
            true => vec4(1.0, 1.0, 1.0, 1.0),
            false => self.pass_clear_color(),
        };
        self.pass
            .set_color_texture(cx, &self.color_texture, DrawPassClearColor::ClearWith(clear));
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        // begin_pass copies the PARENT pass rect into the child; the slot
        // resolution has to be (re)asserted after it or the texture takes
        // the window's size.
        self.pass.set_size(cx, pass_rect.size);
        self.look.yaw = self.orbit_yaw;
        self.look.pitch = self.orbit_pitch;
        if let Some(scene_state) = self.rolled_scene_state(pass_rect, cx.time()) {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            if !flashing {
                let cx3d = &mut Cx3d::new(cx.cx);
                self.draw_scene(cx3d, scene_state);
            }
        }
        cx.end_pass(&self.pass);
        if self.composite && rect.size.x > 1.0 && rect.size.y > 1.0 {
            self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
            self.draw_bg.draw_abs(cx, rect);
            self.area = self.draw_bg.area();
            // Only a composited pane tracks its own area: set_pass_area
            // REPLACES the explicit pass size, and a 4x4 slot placeholder
            // would shrink the program texture to 8x8.
            cx.set_pass_area(&self.pass, self.area);
        }
        DrawStep::done()
    }
}
