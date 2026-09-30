//! Material pixel tests on the real renderer: a row of cubes, one per
//! Splash material feature (the hook set of makepad-render-material), drawn
//! through `Renderer::draw_preview` into an offscreen pass. `tests/pixels.rs`
//! grabs the hidden window and checks each column by what it must show.
//!
//! Columns, left to right:
//! 0. stock lane (no material): lit grey;
//! 1. `finish` returns red: red whatever the light;
//! 2. Unlit, `surface` green: flat green;
//! 3. the error material: magenta hatching;
//! 4. `vertex` lifts the cube 0.9 m: its column is empty at the row's
//!    height and filled above it;
//! 5. `lighting` returns blue: blue;
//! 6. `light` returns yellow for every light, plus the stock ambient:
//!    yellow over the sky fill.
use makepad_draw::*;
use makepad_render::makepad_render_material::{Hook, HookMask, HookSet, MaterialDesc};
use makepad_render::{
    preview_scene_state, set_pass_camera, CustomMaterialInstance, DrawSceneAlpha, DrawSceneCube, DrawSceneCustom,
    DrawSceneSkinned, DrawSceneSky, DrawSceneTerrain, ModelInstance, PreviewLook, PreviewStage, Renderer, SceneDraws,
};
use makepad_render_graph::DrawSceneTexture;
use makepad_widgets::*;

app_main!(App);

pub const COLUMNS: usize = 7;
pub const SPACING: f32 = 1.4;
/// The offscreen pass, in pixels (drawn stretched to the widget).
pub const PASS_W: usize = 800;
pub const PASS_H: usize = 300;

script_mod! {
    use mod.prelude.widgets.*

    mod.widgets.lab_finish_red = fn(c: vec4) -> vec4 { return vec4(1.0, 0.0, 0.0, c.w) }
    mod.widgets.lab_surface_green = fn(base: vec4) -> vec4 { return vec4(0.0, 1.0, 0.0, 1.0) }
    mod.widgets.lab_vertex_up = fn(p: vec3, n: vec3, uv: vec2) -> vec3 { return p + vec3(0.0, 0.9, 0.0) }
    mod.widgets.lab_lighting_blue = fn(direct: vec3, ambient: vec3) -> vec3 { return vec3(0.0, 0.0, 1.0) }
    mod.widgets.lab_light_yellow = fn(radiance: vec3, l: vec3, n: vec3, v: vec3, brdf: vec3) -> vec3 { return vec3(1.0, 1.0, 0.0) }

    mod.widgets.MaterialLabBase = #(MaterialLab::register_widget(vm))
    mod.widgets.MaterialLab = set_type_default() do mod.widgets.MaterialLabBase{
        width: Fill
        height: Fill
    }

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(800, 300)
                body +: {
                    lab := mod.widgets.MaterialLab{}
                }
            }
        }
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

/// A unit cube with a face per normal (flat shading), white.
fn cube_glb() -> Vec<u8> {
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
    ];
    let (mut positions, mut indices) = (Vec::new(), Vec::new());
    for (n, a, b) in faces {
        let base = positions.len() as u32;
        for (sa, sb) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            positions.push([
                0.5 * (n[0] + a[0] * sa + b[0] * sb),
                0.5 * (n[1] + a[1] * sa + b[1] * sb) + 0.5,
                0.5 * (n[2] + a[2] * sa + b[2] * sb),
            ]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let colors = vec![[1.0f32, 1.0, 1.0]; positions.len()];
    makepad_gltf::write_glb_mesh_colored(&positions, &indices, Some(&colors))
}

#[derive(Script, ScriptHook, Widget)]
pub struct MaterialLab {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_cube: DrawSceneCube,
    #[live]
    draw_alpha: DrawSceneAlpha,
    #[live]
    draw_sky: DrawSceneSky,
    #[live]
    draw_terrain: DrawSceneTerrain,
    #[live]
    draw_models: DrawSceneSkinned,
    #[redraw]
    #[live]
    draw_bg: DrawSceneTexture,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    renderer: Renderer,
    #[rust]
    area: Area,
    #[rust(false)]
    initialized: bool,
    #[rust(false)]
    announced: bool,
}

impl MaterialLab {
    /// Build the materials in the app's VM and install them by column name.
    fn install_materials(&mut self, cx: &mut Cx) {
        let built = cx.with_vm(|vm| {
            let f = |vm: &mut ScriptVm, v: ScriptValue| v.as_object().filter(|o| vm.bx.heap.is_fn(*o)).expect("lab hook");
            let red = script_eval!(vm, { mod.widgets.lab_finish_red });
            let green = script_eval!(vm, { mod.widgets.lab_surface_green });
            let up = script_eval!(vm, { mod.widgets.lab_vertex_up });
            let blue = script_eval!(vm, { mod.widgets.lab_lighting_blue });
            let yellow = script_eval!(vm, { mod.widgets.lab_light_yellow });
            let (red, green, up, blue, yellow) = (f(vm, red), f(vm, green), f(vm, up), f(vm, blue), f(vm, yellow));
            let pbr = MaterialDesc::default();
            let one = |h: Hook, o: ScriptObject| HookSet::new().with(h, o);
            let mut out = Vec::new();
            let mut add = |name: &str, m: Result<makepad_render::custom_material::CustomMaterial, _>| match m {
                Ok(m) => out.push((name.to_string(), m)),
                Err(e) => log!("material lab: {name} did not build: {e}"),
            };
            add("finish", DrawSceneCustom::build(vm, &pbr, &one(Hook::Finish, red), HookMask::ALL, Vec4f::default()));
            add("unlit", DrawSceneCustom::unlit(vm, &one(Hook::Surface, green), HookMask::ALL, 0.0));
            add("error", DrawSceneCustom::error_material(vm));
            let mut vertex = one(Hook::Vertex, up);
            vertex.bounds_pad = 1.0;
            add("vertex", DrawSceneCustom::build(vm, &pbr, &vertex, HookMask::ALL, Vec4f::default()));
            add("lighting", DrawSceneCustom::build(vm, &pbr, &one(Hook::Lighting, blue), HookMask::ALL, Vec4f::default()));
            add("light", DrawSceneCustom::build(vm, &pbr, &one(Hook::Light, yellow), HookMask::ALL, Vec4f::default()));
            out
        });
        for (name, material) in built {
            if !self.renderer.install_custom_material(name.clone(), material) {
                log!("material lab: {name} was not installed");
            }
        }
    }

    fn instances() -> Vec<ModelInstance> {
        let names = [None, Some("finish"), Some("unlit"), Some("error"), Some("vertex"), Some("lighting"), Some("light")];
        names.iter().enumerate().map(|(i, name)| {
            let mut transform = Mat4f::identity();
            transform.v[12] = (i as f32 - (COLUMNS as f32 - 1.0) * 0.5) * SPACING;
            ModelInstance {
                model: "lab/cube".into(),
                custom_material: name.map(|n| CustomMaterialInstance { name: n.into(), params: Vec4f::default() }),
                transform,
                tint: vec4(0.8, 0.8, 0.8, 1.0),
                color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
                dynamic: true,
                depth_order: 0.0,
                part_poses: Vec::new(),
            }
        }).collect()
    }
}

impl Widget for MaterialLab {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return DrawStep::done();
        }
        if !self.initialized {
            self.initialized = true;
            self.color_texture = Texture::new_with_format(cx.cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Fixed { width: PASS_W, height: PASS_H }, initial: true });
            self.depth_texture = Texture::new_with_format(cx.cx, TextureFormat::DepthD32 { size: TextureSize::Fixed { width: PASS_W, height: PASS_H }, initial: true });
            self.pass.set_color_texture(cx.cx, &self.color_texture, DrawPassClearColor::ClearWith(vec4(0.05, 0.06, 0.09, 1.0)));
            self.pass.set_depth_texture(cx.cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
            // The 3D camera is ours (set_pass_camera), not the 2D pass's.
            cx.cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
            if let Err(e) = self.renderer.load_model(cx.cx, "lab/cube", &cube_glb(), None) {
                log!("material lab: cube did not load: {e}");
            }
            self.install_materials(cx.cx);
        }
        let size = dvec2(PASS_W as f64, PASS_H as f64);
        self.pass.set_size(cx, size);
        self.pass.set_dpi_factor(cx, 1.0);
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        // begin_pass copies the parent pass rect: re-assert the size, at one
        // texel per point.
        self.pass.set_size(cx, size);
        self.pass.set_dpi_factor(cx, 1.0);
        let look = PreviewLook { target: vec3f(0.0, 0.9, 0.0), distance: 9.0, fov: 30.0, yaw: 0.0, pitch: -0.12 };
        // The pass draws in its own coordinates: the scene's viewport starts at 0.
        let local = Rect { pos: dvec2(0.0, 0.0), size };
        if let Some(scene_state) = preview_scene_state(look, local, cx.time()) {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.renderer.set_models(Self::instances());
            let mut draws = SceneDraws {
                cube: &mut self.draw_cube,
                alpha: &mut self.draw_alpha,
                sky: &mut self.draw_sky,
                sky_analytic: None,
                terrain: &mut self.draw_terrain,
                shadow: None,
                shadow_sdf: None,
                firework: None,
                flare: None,
                water: None,
                screen: None,
                screen_instances: &[],
                view_model: None,
            };
            let stage = PreviewStage { ground: false, sky: false, ground_half: 8.0, ground_color: vec4(0.0, 0.0, 0.0, 1.0), dark: false };
            let stats = self.renderer.draw_preview(cx3d, &mut self.draw_list, &mut draws, look, stage, scene_state, None, Some(&mut self.draw_models));
            if !self.announced { log!("material lab: {} model instances, {} draws, {} culled, {} tris", stats.model_instances, stats.model_draws, stats.model_culled, stats.model_triangles); }
        }
        cx.end_pass(&self.pass);
        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        cx.set_pass_area(&self.pass, self.area);
        // Metal compiles pipelines asynchronously: keep drawing until every
        // material's pipeline is ready (the lanes fall back to stock until
        // then), and say so once for the test.
        if !self.announced {
            let names = ["finish", "unlit", "error", "vertex", "lighting", "light"];
            let ready = names.iter().filter(|n| self.renderer.custom_material_shader(n).is_some_and(|id| cx.cx.draw_shader_ready(id, false))).count();
            if ready == names.len() {
                self.announced = true;
                log!("material lab: ready, {ready} materials");
            }
        }
        cx.new_next_frame();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::NextFrame(_) = event {
            self.draw_bg.redraw(cx);
        }
    }
}
