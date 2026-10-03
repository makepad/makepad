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
//!
//! With `--scene=rect` or `--scene=ibl` the lab instead draws a `World`
//! (`Renderer::draw_scene_full`) of generic items on resident geometry, in
//! the dark (the world's Sun and Sky at zero):
//! 0. a white cube lit by a rectangular area light before it (rect) or a smooth metal
//!    cube lit by the `sunset` environment (ibl);
//! 1. the same material with no light near it;
//! 2. an Unlit cyan cube;
//! 3. two small red cubes from one packed Instances item.
use makepad_draw::*;
use makepad_render::makepad_render_material::{Hook, HookMask, HookSet, MaterialDesc};
use makepad_render::{
    preview_scene_state, set_pass_camera, CustomMaterialInstance, DrawSceneAlpha, DrawSceneCube, DrawSceneCustom,
    DrawSceneSkinned, DrawSceneSky, DrawSceneTerrain, ModelInstance, PreviewLook, PreviewStage, Renderer, SceneDraws,
    TransformTint,
};
use makepad_render_graph::DrawSceneTexture;
use makepad_widgets::*;
use makepad_render::GeometryData;
use makepad_scene::{
    GeometryId, GeometryRef, InstanceSource, Item, ItemKind, Light, MaterialFrame, MaterialId, MaterialKind, PbrParams,
    UnlitParams, World,
};

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

/// Which scene the lab draws (`--scene=rect|ibl`, else the hook row).
#[derive(Clone, Copy, PartialEq)]
enum Scene {
    Hooks,
    Rect,
    Ibl,
}

fn scene() -> Scene {
    match std::env::args().find_map(|a| a.strip_prefix("--scene=").map(str::to_string)).as_deref() {
        Some("rect") => Scene::Rect,
        Some("ibl") => Scene::Ibl,
        _ => Scene::Hooks,
    }
}

/// The unit cube (y 0..1) as resident geometry, flat normals.
fn cube_geometry() -> GeometryData {
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
    ];
    let mut g = GeometryData::default();
    for (n, a, b) in faces {
        let base = g.positions.len() as u32;
        for (sa, sb) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            g.positions.push([
                0.5 * (n[0] + a[0] * sa + b[0] * sb),
                0.5 * (n[1] + a[1] * sa + b[1] * sb) + 0.5,
                0.5 * (n[2] + a[2] * sa + b[2] * sb),
            ]);
            g.normals.push(n);
        }
        // Winding outward (cross(b - a, c - a) along n).
        g.indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
    }
    g
}

fn column_x(c: usize) -> f32 {
    (c as f32 - (COLUMNS as f32 - 1.0) * 0.5) * SPACING
}

/// The dark world of generic items for `--scene=rect|ibl`.
fn items_world(scene: Scene) -> World {
    let mut w = World::new();
    let cube = GeometryRef::Resident(GeometryId(1));
    let at = |c: usize| {
        let mut m = Mat4f::identity();
        m.v[12] = column_x(c);
        m
    };
    // No key, no fill: only the lights under test.
    w.lights.push(Light::Sun { dir: vec3f(0.3, 1.0, 0.2), color: vec3f(1.0, 1.0, 1.0), lux: 0.0, shadow: Default::default() });
    w.lights.push(Light::Sky { top: vec3f(0.0, 0.0, 0.0), ground: vec3f(0.0, 0.0, 0.0), intensity: 0.0 });
    let lit = match scene {
        Scene::Ibl => MaterialKind::Pbr(PbrParams { base_color: vec4(1.0, 1.0, 1.0, 1.0), metallic: 1.0, roughness: 0.15, ..Default::default() }),
        _ => MaterialKind::Pbr(PbrParams { base_color: vec4(0.9, 0.9, 0.9, 1.0), metallic: 0.0, roughness: 0.8, ..Default::default() }),
    };
    w.set_material(MaterialFrame { id: MaterialId(1), kind: lit, ..Default::default() });
    w.set_material(MaterialFrame { id: MaterialId(2), kind: MaterialKind::Unlit(UnlitParams { color: vec4(0.0, 1.0, 1.0, 1.0), intensity: 1.0, map: None }), ..Default::default() });
    w.items.push(Item::new(ItemKind::Mesh { geometry: cube, material: MaterialId(1), transform: at(0) }));
    w.items.push(Item::new(ItemKind::Mesh { geometry: cube, material: MaterialId(1), transform: at(1) }));
    w.items.push(Item::new(ItemKind::Mesh { geometry: cube, material: MaterialId(2), transform: at(2) }));
    // Two half-size cubes, one Instances item, tinted red through an
    // Unlit material (so they show in the dark).
    let records = [0.0f32, 0.55].map(|dy| {
        let mut m = Mat4f::identity();
        m.v[0] = 0.45;
        m.v[5] = 0.45;
        m.v[10] = 0.45;
        m.v[12] = column_x(3);
        m.v[13] = dy;
        TransformTint { transform: m, tint: vec4(1.0, 0.0, 0.0, 1.0), glow: 0.0 }
    });
    w.items.push(Item::new(ItemKind::Instances {
        geometry: cube,
        material: MaterialId(4),
        source: InstanceSource::Packed { data: TransformTint::floats(&records).into(), layout: makepad_render::LAYOUT_TRANSFORM_TINT },
        count: 2,
    }));
    w.set_material(MaterialFrame { id: MaterialId(4), kind: MaterialKind::Unlit(UnlitParams { color: vec4(1.0, 1.0, 1.0, 1.0), intensity: 1.0, map: None }), ..Default::default() });
    match scene {
        // In front of the cube's camera-side face (z = 0.5; the camera
        // looks down -z) and aimed back at it: the emitter is one-sided,
        // so a light above the cube reached only its thin top face. Every
        // point of that face is within 0.9 m; the next cube's nearest point
        // is 1.006 m away, past the range, so it gets no light at all.
        Scene::Rect => w.lights.push(Light::Rect {
            pos: vec3f(column_x(0), 0.6, 0.95),
            normal: vec3f(0.0, -0.25, -1.0),
            tangent: vec3f(1.0, 0.0, 0.0),
            size: vec2f(0.8, 0.4),
            color: vec3f(1.0, 0.95, 0.85),
            intensity: 6.0,
            range: 0.95,
        }),
        Scene::Ibl => {
            w.environment.ibl = Some(makepad_scene::Ibl { source: makepad_scene::IblSource::Procedural(2), intensity: 1.0, rotation_deg: 0.0 });
        }
        Scene::Hooks => {}
    }
    w
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
    #[rust]
    frames: u64,
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
            self.pass.set_keep_camera_matrix(cx.cx, true);
            if let Err(e) = self.renderer.load_model(cx.cx, "lab/cube", &cube_glb(), None) {
                log!("material lab: cube did not load: {e}");
            }
            self.install_materials(cx.cx);
            if let Err(e) = self.renderer.register_geometry(GeometryId(1), cube_geometry()) {
                log!("material lab: cube geometry refused: {e}");
            }
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
            let scene = scene();
            self.renderer.set_models(if scene == Scene::Hooks { Self::instances() } else { Vec::new() });
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
            let stats = if scene == Scene::Hooks {
                self.renderer.draw_preview(cx3d, &mut self.draw_list, &mut draws, look, stage, scene_state, None, Some(&mut self.draw_models))
            } else {
                let world = items_world(scene);
                self.renderer.draw_scene_full(cx3d, &mut self.draw_list, &mut draws, &world, scene_state, None, Some(&mut self.draw_models))
            };
            self.frames += 1;
            if !self.announced { log!("material lab: {} model instances, {} draws, {} culled, {} tris", stats.model_instances, stats.model_draws, stats.model_culled, stats.model_triangles); }
        }
        cx.end_pass(&self.pass);
        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        // The pass keeps its own size: its texture is a fixed PASS_W x
        // PASS_H, so the viewport must be too (an area-placed pass would
        // follow the window and draw past the texture).
        self.area = self.draw_bg.area();
        // Metal compiles pipelines asynchronously: keep drawing until every
        // material's pipeline is ready (the lanes fall back to stock until
        // then), and say so once for the test.
        if !self.announced && scene() != Scene::Hooks {
            // Built-in item materials compile on first use; give the
            // pipelines time, then say so once.
            let names = ["__item_unlit"];
            let ready = names.iter().all(|n| self.renderer.custom_material_shader(n).is_some_and(|id| cx.cx.draw_shader_ready(id, false)));
            if ready && self.frames > 90 {
                self.announced = true;
                log!("material lab: ready, world scene, {} items skipped", self.renderer.skipped_items());
            }
        }
        if !self.announced && scene() == Scene::Hooks {
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
