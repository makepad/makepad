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
}

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
        self.status.clear();
        self.area.redraw(cx);
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
            PreparedMesh::Statue { glb, base_color } => {
                // Unskinned fallback: the renderer's own loader still parses
                // on this thread, over worker-capped bytes.
                let name = format!("vj/statue-{}", self.load_count);
                let png = base_color;
                match self.renderer.load_model(cx, &name, &glb, png.as_deref()) {
                    Ok(_tris) => {
                        let (min, max) = self
                            .renderer
                            .model_bounds(&name)
                            .unwrap_or((vec3f(0.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0)));
                        let height = (max.y - min.y).max(0.01);
                        let scale = 1.75 / height;
                        self.dancer = None;
                        self.statue_yaw = 0.0;
                        self.statue = Some(ModelInstance {
                            model: name,
                            transform: trs_yaw(vec3f(0.0, -min.y * scale, 0.0), 0.0, scale),
                            dynamic: false,
                            depth_order: 0.0,
                        });
                        self.status =
                            "static mesh (no animation clips) — turntable".to_string();
                    }
                    Err(e) => {
                        self.status = format!("mesh load failed: {e}");
                    }
                }
            }
        }
        self.area.redraw(cx);
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
        let stage = if self.stage {
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
        if self.next_frame.is_event(event).is_some() {
            let time = cx.seconds_since_app_start();
            let last = self.last_time.replace(time).unwrap_or(time);
            self.time_accum += (time - last).min(0.25);
            let mut ticked = false;
            while self.time_accum >= TICK_DT as f64 {
                self.time_accum -= TICK_DT as f64;
                if let Some(d) = self.dancer.as_mut() {
                    // sample_clip wraps by duration: this IS the loop.
                    d.clip_time += TICK_DT;
                }
                self.statue_yaw += TICK_DT * 0.5;
                ticked = true;
            }
            if ticked && (self.dancer.is_some() || self.statue.is_some()) {
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
        self.pass.set_size(cx, pass_rect.size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.pass_clear_color()),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        self.look.yaw = self.orbit_yaw;
        self.look.pitch = self.orbit_pitch;
        if let Some(scene_state) = preview_scene_state(self.look, pass_rect, cx.time()) {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_scene(cx3d, scene_state);
        }
        cx.end_pass(&self.pass);
        if self.composite && rect.size.x > 1.0 && rect.size.y > 1.0 {
            self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
            self.draw_bg.draw_abs(cx, rect);
            self.area = self.draw_bg.area();
        }
        cx.set_pass_area(&self.pass, self.area);
        DrawStep::done()
    }
}
