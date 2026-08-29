//! Offscreen GPU smoke for the 3D program slot's shading.
//!
//!   PBR_VIEW_GLB=<path.glb> PBR_VIEW_YAW=0,45,90 PBR_VIEW_OUT=<dir> \
//!       cargo run -p makepad-vj --release --example pbr_view
//!
//! Loads one GLB through exactly the path `mesh_view.rs` uses for an
//! unskinned prop — `Renderer::load_model` + `Renderer::draw_preview` into an
//! offscreen pass — and writes one PPM per camera yaw, then exits.
//!
//! It exists because the specular lane
//! ([`makepad_render::DrawScenePbr`]) is picked by MATERIAL at load, and the
//! only honest way to check a shader is to look at what it draws. Two knobs
//! matter: the model (matte vs shiny material = the before/after) and the
//! camera yaw (a specular highlight moves with the eye; a diffuse term does
//! not). Both are inputs here, so a regression is one command away.
//!
//! PPM rather than PNG on purpose: this crate has no encoder dependency and
//! P6 is four lines of code. `sips`/`magick`/python convert it.

use makepad_render::{
    preview_scene_state, set_pass_camera, DrawSceneAlpha, DrawSceneCube, DrawSceneShadow,
    DrawSceneSkinned, DrawSceneSkinnedGpu, DrawSceneSky, DrawSceneTerrain, DrawSceneTexture,
    ModelInstance, PreviewLook, PreviewStage, Renderer, SceneDraws,
};
use makepad_widgets::*;

app_main!(App);

const SIZE: f64 = 512.0;

script_mod! {
    use mod.prelude.widgets.*

    mod.widgets.PbrProbeBase = #(PbrProbe::register_widget(vm))
    mod.widgets.PbrProbe = set_type_default() do mod.widgets.PbrProbeBase{
        width: Fill
        height: Fill
        // The same key direction the VJ mesh slot lights its stage with, so
        // what this renders is what that slot renders.
        draw_cube +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_alpha +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_terrain +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_skinned +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_models +: { light_dir: vec3(0.35, 0.8, 0.45) }
    }

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(560, 560)
                body +: {
                    probe := PbrProbe{}
                }
            }
        }
    }
}

/// yaw-rotation * uniform scale, translated. Copied from `mesh_view.rs` so
/// the statue sits exactly where the VJ slot puts it.
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

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct PbrProbe {
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
    statue: Option<ModelInstance>,
    /// Camera yaws still to shoot, in radians.
    #[rust]
    yaws: Vec<(f32, String)>,
    /// Frames drawn at the current yaw. The readback is only meaningful once
    /// the GPU has actually delivered the pass.
    #[rust(0u32)]
    settle: u32,
    #[rust]
    next_frame: NextFrame,
}

impl PbrProbe {
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
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
        self.next_frame = cx.new_next_frame();

        let path = std::env::var("PBR_VIEW_GLB").expect("PBR_VIEW_GLB=<path.glb>");
        let out = std::env::var("PBR_VIEW_OUT").unwrap_or_else(|_| ".".into());
        let stem = std::path::Path::new(&path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("model")
            .to_string();
        self.yaws = std::env::var("PBR_VIEW_YAW")
            .unwrap_or_else(|_| "0".into())
            .split(',')
            .filter_map(|s| s.trim().parse::<f32>().ok())
            .map(|deg| {
                (
                    deg.to_radians(),
                    format!("{out}/{stem}_yaw{}.ppm", deg as i32),
                )
            })
            .collect();
        self.yaws.reverse();

        let glb = std::fs::read(&path).expect("read glb");
        let png = makepad_render::model::embedded_base_color_png(&glb);
        let tris = self
            .renderer
            .load_model(cx, "probe", &glb, png.as_deref())
            .expect("load model");
        let (min, max) = self
            .renderer
            .model_bounds("probe")
            .unwrap_or((vec3f(0.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0)));
        // Same framing as the VJ slot: 1.75 units tall, feet on the slab.
        let scale = 1.75 / (max.y - min.y).max(0.01);
        self.statue = Some(ModelInstance {
            model: "probe".into(),
            transform: trs_yaw(vec3f(0.0, -min.y * scale, 0.0), 0.0, scale),
            tint: vec4(1.0, 1.0, 1.0, 1.0),
            color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
            dynamic: false,
            depth_order: 0.0,
            part_poses: Vec::new(),
        });
        self.look.target = vec3f(0.0, 0.9, 0.0);
        self.look.distance = 4.6;
        self.look.fov = 45.0;
        log!("pbr_view: {path} — {tris} triangles, {} shots", self.yaws.len());
    }

    /// Read the pass back and write a P6 PPM. Returns false while the GPU has
    /// not delivered yet, so the caller shoots the same frame again.
    fn capture(&mut self, cx: &mut Cx, file: &str) -> bool {
        let Some((w, h, bgra)) = cx.debug_read_render_texture(&self.color_texture) else {
            return false;
        };
        if w == 0 || h == 0 || bgra.len() < w * h * 4 {
            return false;
        }
        let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
        for px in bgra.chunks_exact(4).take(w * h) {
            out.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        match std::fs::write(file, &out) {
            Ok(()) => log!("pbr_view: wrote {file} ({w}x{h})"),
            Err(e) => log!("pbr_view: cannot write {file}: {e}"),
        }
        true
    }
}

impl Widget for PbrProbe {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        self.ensure_initialized(cx.cx);
        let Some(yaw) = self.yaws.last().map(|(y, _)| *y) else {
            return DrawStep::done();
        };
        self.look.yaw = yaw;
        let pass_rect = Rect { pos: dvec2(0.0, 0.0), size: dvec2(SIZE, SIZE) };
        self.pass.set_size(cx, pass_rect.size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(vec4(0.015, 0.02, 0.04, 1.0)),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        self.pass.set_size(cx, pass_rect.size);
        self.pass.set_dpi_factor(cx, 1.0);
        if let Some(scene_state) = preview_scene_state(self.look, pass_rect, cx.time()) {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_list.begin_always(cx3d);
            self.renderer
                .set_models(self.statue.iter().cloned().collect());
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
            let mut stage = PreviewStage::statue();
            stage.ground_half = 9.0;
            stage.ground_color = vec4(0.10, 0.11, 0.14, 1.0);
            self.renderer.draw_preview(
                cx3d,
                &mut self.draw_list,
                &mut draws,
                self.look,
                stage,
                scene_state,
                None,
                Some(&mut self.draw_models),
            );
            self.draw_list.end(cx3d);
        }
        cx.end_pass(&self.pass);
        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_none() {
            return;
        }
        self.next_frame = cx.new_next_frame();
        if !self.initialized {
            self.area.redraw(cx);
            return;
        }
        // Eight frames of settling before the first readback: the pass is
        // pipelined and an early read returns the clear colour.
        self.settle += 1;
        if self.settle < 8 {
            self.area.redraw(cx);
            return;
        }
        let Some((_, file)) = self.yaws.last().cloned() else {
            cx.quit();
            return;
        };
        if self.capture(cx, &file) {
            self.yaws.pop();
            self.settle = 0;
        }
        if self.yaws.is_empty() {
            cx.quit();
            return;
        }
        self.area.redraw(cx);
    }
}

impl WidgetNode for PbrProbe {
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

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_render::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
