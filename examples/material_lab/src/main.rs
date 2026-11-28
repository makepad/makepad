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
//!
//! Above every cube sits a sphere with the same material (and in the rect
//! scene its own light): a sphere has every normal, so a light, the
//! environment, a rim or a highlight always shows on it, where a cube can
//! turn its faces away.
//!
//! The traced scenes use an 800 × 300 orthographic target. T/M/W/F/O select
//! analytic, mesh, held, rejected, or omitted. C/A/G/S toggle caster, AO,
//! fog and SSR. 1/2 bracket a known hit in depth; 0 removes the bracket.
//! In the early-depth scene E changes fragment work and B moves the prefill.
//! --trace-series measures the four combinations at natural presentation.
//! With `--scene=traced-volume`, --trace-series times the camera outside and inside the room,
//! pairing frames from two prewarmed programs while a separate pass holds the GPU clock.
//! In `--scene=traced-volume`, X opens or restores a hole through a pillar. I moves inside,
//! N slices the wall with the near plane, and V returns to the outside view.
use makepad_draw::*;
use makepad_render::makepad_render_material::{Hook, HookMask, HookSet, MaterialDesc};
use makepad_render::{
    preview_scene_state, set_pass_camera, CustomMaterialInstance, DrawSceneAlpha, DrawSceneCube, DrawSceneCustom,
    DrawSceneSkinned, DrawSceneSky, DrawSceneTerrain, ModelInstance, PreviewLook, PreviewStage, Renderer, SceneDraws,
    TransformTint,
};
use makepad_render_graph::DrawSceneTexture;
use makepad_widgets::*;
use makepad_render::{GeometryData, GpuLightmapMode, PbrMaterial, PreparedStaticPreview, StaticModel};
use makepad_render::custom_material::{TracedLayerRole, TracedSurfacePasses, TracedSurfaceRecipe, TracedSurfaceState};
use makepad_scene::{
    GeometryId, GeometryRef, InstanceSource, Item, ItemKind, Light, MaterialFrame, MaterialId, MaterialKind, PbrParams,
    UnlitParams, World,
};

mod traced_volume;
use traced_volume::VolumeCamera;
app_main!(App);

pub const COLUMNS: usize = 7;
pub const SPACING: f32 = 1.4;
/// The sphere row's base height: spheres span y 1.35..2.35 above the cubes'
/// 0..1, with a gap the tests find the rows by.
pub const SPHERE_ROW: f32 = 1.35;
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

    mod.widgets.lab_expensive_hit = fn(p: vec3, d: vec3) -> vec4 {
        let hit = self.trace_sphere_hit(p, d)
        var phase = dot(p, vec3(1.7, 2.3, 0.9))
        for i in 0..192 { phase = sin(phase * 1.91 + d.z) }
        return vec4(hit.xyz, phase)
    }
    mod.widgets.LabClockHold = mod.draw.DrawQuad{
        iters: uniform(380.0)
        scene: texture_2d(float)
        pixel: fn() -> vec4 {
            var phase = self.pos.x + self.scene.sample_nearest(self.pos).x * 1.0e-6
            for i in 0..u32(self.iters) {
                for j in 0..8 { phase = sin(phase * 1.91 + self.pos.y) }
            }
            return vec4(0.5 + 0.5 * phase, 0.0, 0.0, 1.0)
        }
    }
    mod.widgets.lab_hit_color = fn(material: float, p: vec3) -> vec4 {
        return vec4(vec3(1.0, 1.0, 1.0) * (1.0 + material * 0.0001), 1.0)
    }
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
        if scene().traced() { vm.cx_mut().enable_pick_variants(); }
        makepad_widgets::script_mod(vm);
        makepad_render::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

/// Which scene the lab draws, with the hook row as the default.
#[derive(Clone, Copy, PartialEq)]
enum Scene {
    Hooks,
    Rect,
    Ibl,
    Traced,
    EarlyDepth,
    Volume,
}

impl Scene {
    fn traced(self) -> bool { matches!(self, Self::Traced | Self::EarlyDepth | Self::Volume) }
}

fn scene() -> Scene {
    static SELECTED: std::sync::OnceLock<Scene> = std::sync::OnceLock::new();
    *SELECTED.get_or_init(|| match std::env::args().find_map(|a| a.strip_prefix("--scene=").map(str::to_string)).as_deref() {
        Some("rect") => Scene::Rect,
        Some("ibl") => Scene::Ibl,
        Some("traced") => Scene::Traced,
        Some("traced-early-depth") => Scene::EarlyDepth,
        Some("traced-volume") => Scene::Volume,
        _ => Scene::Hooks,
    })
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
        // Counter-clockwise seen from outside: cross(b - a, c - a) along n.
        g.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    g
}

/// A unit-diameter sphere on the cube's footprint (y 0..1): rings of shared
/// vertices, so every normal is smooth (the direction from the centre).
/// Quads come counter-clockwise seen from outside.
fn sphere_mesh() -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[u32; 4]>) {
    const RINGS: u32 = 24;
    const SEGS: u32 = 48;
    let mut positions = vec![[0.0, 1.0, 0.0]];
    let mut normals = vec![[0.0, 1.0, 0.0]];
    for r in 1..RINGS {
        let theta = std::f32::consts::PI * r as f32 / RINGS as f32;
        for s in 0..SEGS {
            let phi = std::f32::consts::TAU * s as f32 / SEGS as f32;
            let n = [theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()];
            positions.push([0.5 * n[0], 0.5 + 0.5 * n[1], 0.5 * n[2]]);
            normals.push(n);
        }
    }
    let bottom = positions.len() as u32;
    positions.push([0.0, 0.0, 0.0]);
    normals.push([0.0, -1.0, 0.0]);
    // Ring r (1..RINGS-1), segment s; the poles are single vertices.
    let at = |r: u32, s: u32| match r {
        0 => 0,
        r if r == RINGS => bottom,
        r => 1 + (r - 1) * SEGS + s % SEGS,
    };
    let mut quads = Vec::new();
    for r in 0..RINGS {
        for s in 0..SEGS {
            // Along the segment, then down the ring: counter-clockwise.
            quads.push([at(r, s), at(r, s + 1), at(r + 1, s + 1), at(r + 1, s)]);
        }
    }
    (positions, normals, quads)
}

/// The sphere as resident geometry, wound as `cube_geometry`.
fn sphere_geometry() -> GeometryData {
    let (positions, normals, quads) = sphere_mesh();
    let mut g = GeometryData { positions, normals, ..Default::default() };
    for [a, b, c, d] in quads {
        // A pole quad has two corners on the pole: one triangle.
        if a != b {
            g.indices.extend_from_slice(&[a, b, c]);
        }
        if c != d {
            g.indices.extend_from_slice(&[a, c, d]);
        }
    }
    g
}

/// The sphere for the hook row's model lane, wound as `cube_glb`.
fn sphere_glb() -> Vec<u8> {
    let (positions, _, quads) = sphere_mesh();
    let mut indices = Vec::new();
    for [a, b, c, d] in quads {
        if a != b {
            indices.extend_from_slice(&[a, b, c]);
        }
        if c != d {
            indices.extend_from_slice(&[a, c, d]);
        }
    }
    let colors = vec![[1.0f32, 1.0, 1.0]; positions.len()];
    makepad_gltf::write_glb_mesh_colored(&positions, &indices, Some(&colors))
}

fn column_x(c: usize) -> f32 {
    (c as f32 - (COLUMNS as f32 - 1.0) * 0.5) * SPACING
}

/// The dark world of generic items for `--scene=rect|ibl`.
fn items_world(scene: Scene) -> World {
    let mut w = World::new();
    let cube = GeometryRef::Resident(GeometryId(1));
    let sphere = GeometryRef::Resident(GeometryId(2));
    let at = |c: usize, y: f32| {
        let mut m = Mat4f::identity();
        m.v[12] = column_x(c);
        m.v[13] = y;
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
    for (geometry, y) in [(cube, 0.0), (sphere, SPHERE_ROW)] {
        w.items.push(Item::new(ItemKind::Mesh { geometry, material: MaterialId(1), transform: at(0, y) }));
        w.items.push(Item::new(ItemKind::Mesh { geometry, material: MaterialId(1), transform: at(1, y) }));
        w.items.push(Item::new(ItemKind::Mesh { geometry, material: MaterialId(2), transform: at(2, y) }));
        // Two half-size copies, one Instances item, tinted red through an
        // Unlit material (so they show in the dark).
        let records = [0.0f32, 0.55].map(|dy| {
            let mut m = Mat4f::identity();
            m.v[0] = 0.45;
            m.v[5] = 0.45;
            m.v[10] = 0.45;
            m.v[12] = column_x(3);
            m.v[13] = y + dy;
            TransformTint { transform: m, tint: vec4(1.0, 0.0, 0.0, 1.0), glow: 0.0 }
        });
        w.items.push(Item::new(ItemKind::Instances {
            geometry,
            material: MaterialId(4),
            source: InstanceSource::Packed { data: TransformTint::floats(&records).into(), layout: makepad_render::LAYOUT_TRANSFORM_TINT },
            count: 2,
        }));
    }
    w.set_material(MaterialFrame { id: MaterialId(4), kind: MaterialKind::Unlit(UnlitParams { color: vec4(1.0, 1.0, 1.0, 1.0), intensity: 1.0, map: None }), ..Default::default() });
    match scene {
        // In front of the cube's camera-side face (z = 0.5; the camera
        // looks down -z) and aimed back at it: the emitter is one-sided,
        // so a light above the cube reached only its thin top face. Every
        // point of that face is within 0.9 m; the next cube's nearest point
        // is 1.006 m away, past the range, so it gets no light at all. The
        // sphere above has the same light at the same place; each light is
        // out of the other row's reach (0.96 m to the cube, 1.07 m to the
        // sphere).
        Scene::Rect => {
            for y in [0.6, SPHERE_ROW + 0.5] {
                w.lights.push(Light::Rect {
                    pos: vec3f(column_x(0), y, 0.95),
                    normal: vec3f(0.0, -0.25, -1.0),
                    tangent: vec3f(1.0, 0.0, 0.0),
                    size: vec2f(0.8, 0.4),
                    color: vec3f(1.0, 0.95, 0.85),
                    intensity: 6.0,
                    range: 0.95,
                });
            }
        }
        Scene::Ibl => {
            w.environment.ibl = Some(makepad_scene::Ibl { key_reflection: makepad_scene::KeyReflection::Light, source: makepad_scene::IblSource::Procedural(2), intensity: 1.0, rotation_deg: 0.0 });
        }
        Scene::Hooks | Scene::Traced | Scene::EarlyDepth | Scene::Volume => {}
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
    #[rust]
    trace: Option<TraceLab>,
    #[rust(false)]
    initialized: bool,
    #[rust(false)]
    announced: bool,
}

impl MaterialLab {
    /// Build the materials in the app's VM and install them by column name.
    fn install_materials(&mut self, cx: &mut Cx) {
        let built = cx.with_vm(|vm| {
            let f = |vm: &mut ScriptVm, v: ScriptValue| v.as_object().filter(|o| vm.is_fn(*o)).expect("lab hook");
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
        let rows = [("lab/cube", 0.0), ("lab/sphere", SPHERE_ROW)];
        rows.iter().flat_map(|&(model, y)| names.iter().enumerate().map(move |(i, name)| (model, y, i, name))).map(|(model, y, i, name)| {
            let mut transform = Mat4f::identity();
            transform.v[12] = column_x(i);
            transform.v[13] = y;
            ModelInstance {
                model: model.into(),
                custom_material: name.map(|n| CustomMaterialInstance { name: n.into(), params: Vec4f::default() }),
                layer_materials: Vec::new(),
                transform,
                tint: vec4(0.8, 0.8, 0.8, 1.0),
                color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
                dynamic: true,
                depth_order: 0.0,
                part_poses: Vec::new(),
                lamps: Default::default(),
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
        let selected = scene();
        if selected.traced() {
            let mut trace = self.trace.take().unwrap_or_else(|| TraceLab::new(self, cx.cx, selected));
            trace.draw(self, cx, rect);
            self.trace = Some(trace);
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
            if let Err(e) = self.renderer.load_model(cx.cx, "lab/sphere", &sphere_glb(), None) {
                log!("material lab: sphere did not load: {e}");
            }
            self.install_materials(cx.cx);
            if let Err(e) = self.renderer.register_geometry(GeometryId(1), cube_geometry()) {
                log!("material lab: cube geometry refused: {e}");
            }
            if let Err(e) = self.renderer.register_geometry(GeometryId(2), sphere_geometry()) {
                log!("material lab: sphere geometry refused: {e}");
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
        // Framed on both rows (cubes y 0..1, spheres 1.35..2.35).
        let look = PreviewLook { target: vec3f(0.0, 1.15, 0.0), distance: 9.0, fov: 30.0, yaw: 0.0, pitch: -0.12 };
        // The pass draws in its own coordinates: the scene's viewport starts at 0.
        let local = Rect { pos: dvec2(0.0, 0.0), size };
        if let Some(mut scene_state) = preview_scene_state(look, local, cx.time()) {
            // The texture is drawn stretched over the widget: project at the
            // widget's aspect, so a sphere stays round however the window
            // (or a title bar inside it) shapes the widget.
            if let Some(shown) = preview_scene_state(look, Rect { pos: dvec2(0.0, 0.0), size: rect.size }, cx.time()) {
                scene_state.projection = shown.projection;
            }
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            let scene = scene();
            self.renderer.set_models(if scene == Scene::Hooks { Self::instances() } else { Vec::new() });
            let mut draws = SceneDraws {
                cube: Some(&mut self.draw_cube),
                alpha: Some(&mut self.draw_alpha),
                sky: &mut self.draw_sky,
                sky_analytic: None,
                terrain: Some(&mut self.draw_terrain),
                shadow: None,
                shadow_sdf: None,
                firework: None,
                flare: None,
                water: None,
                screen: None,
                screen_instances: &[],
                view_model: None,
            };
            let stage = PreviewStage::empty();
            let stats = if scene == Scene::Hooks {
                self.renderer.draw_preview(cx3d, &mut self.draw_list, &mut draws, look, stage, scene_state, None, Some(&mut self.draw_models))
            } else {
                let world = items_world(scene);
                self.renderer.draw_scene_full(cx3d, &mut self.draw_list, &mut draws, &world, scene_state, None, Some(&mut self.draw_models))
            };
            if !self.announced { log!("material lab: {} model instances, {} draws, {} culled, {} tris", stats.model_instances, stats.model_draws, stats.model_culled, stats.model_triangles); }
        }
        cx.end_pass(&self.pass);
        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        // The pass keeps its own size: its texture is a fixed PASS_W x
        // PASS_H, so the viewport must be too (an area-placed pass would
        // follow the window and draw past the texture).
        self.area = self.draw_bg.area();
        // Metal compiles pipelines asynchronously, and a material draws through
        // a stock lane until its pipeline exists. Ready is the first frame the
        // renderer waited on nothing (`items_ready`: the environment, the item
        // materials, the PBR and stock IBL pipelines) and drew nothing through a
        // fallback lane (`drew_fallback`), so the grab sees every material however
        // fast the frames come or the pipelines compile. Said once for the test.
        if !self.announced {
            let hooks = ["finish", "unlit", "error", "vertex", "lighting", "light"];
            let installed = scene() != Scene::Hooks || hooks.iter().all(|n| self.renderer.custom_material(n).is_some());
            if installed && self.renderer.items_ready(cx.cx) && !self.renderer.drew_fallback() {
                self.announced = true;
                if scene() == Scene::Hooks {
                    log!("material lab: ready, {} materials", hooks.len());
                } else {
                    log!("material lab: ready, world scene, {} items skipped", self.renderer.skipped_items());
                }
            }
        }
        cx.new_next_frame();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let (Some(trace), Event::KeyDown(key)) = (&mut self.trace, event) {
            if !key.is_repeat {
                let revision = trace.revision;
                trace.key(key.key_code);
                if trace.revision != revision { self.draw_bg.redraw(cx); }
            }
        }
        if let Event::NextFrame(_) = event {
            self.draw_bg.redraw(cx);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SurfaceView { Traced, Mesh, Waiting, Failed, Omitted }

fn trace_volumes_available(cx: &Cx) -> bool {
    let caps = cx.gpu_info().volume_caps();
    caps.max_dimension > 0 && (caps.r8_unorm || caps.rgba8_unorm)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct TraceControls {
    camera: VolumeCamera,
    view: SurfaceView,
    caster: bool,
    ao: bool,
    fog: bool,
    ssr: bool,
    expensive: bool,
    front: bool,
    depth_probe: i8,
    cleared: bool,
}

struct TraceLab {
    controls: TraceControls,
    early: bool,
    volume_series: Option<traced_volume::VolumeSeries>,
    volume: Option<traced_volume::VolumeFixture>,
    outside_name: &'static str,
    series_block: Option<usize>,
    captures: Option<std::path::PathBuf>,
    revision: u64,
    installed: Option<(u64, TraceControls)>,
    availability: SurfaceView,
    inside_draws: u64,
    held: std::rc::Rc<std::cell::Cell<bool>>,
    recipe: TracedSurfaceRecipe,
    world: World,
    surface: Texture,
    envspec: Texture,
    // Two completed scene recordings prime the previous-depth AO input.
    recording: Option<u64>,
    primed: bool,
    rendered: bool,
    announced: bool,
    color_read: Option<ReadbackTicket>,
    surface_read: Option<ReadbackTicket>,
    pick_read: Option<PickTicket>,
    files: Vec<(std::path::PathBuf, HostBuf)>,
    writer: Option<makepad_platform::thread::TaskHandle<std::io::Result<()>>>,
    ssr_frames: Option<u8>,
    timed_samples: usize,
}

fn trace_instance(model: &str, pos: [f32; 3], scale: [f32; 3], rgb: [f32; 3]) -> ModelInstance {
    let mut transform = Mat4f::identity();
    transform.v[0] = scale[0];
    transform.v[5] = scale[1];
    transform.v[10] = scale[2];
    transform.v[12..15].copy_from_slice(&pos);
    ModelInstance { model: model.into(), transform, tint: vec4(rgb[0], rgb[1], rgb[2], 1.0),
        color_adjust: vec4(0.0, 1.0, 1.0, 0.0), dynamic: true, depth_order: 0.0,
        custom_material: None, layer_materials: vec![], part_poses: vec![], lamps: Default::default() }
}

fn trace_asset(renderer: &mut Renderer, cx: &mut Cx, name: &str, g: GeometryData, mr: [f32; 2], pick: u32, programs: &[&str]) {
    let mut vertices = Vec::new();
    for (p, n) in g.positions.iter().zip(&g.normals) {
        let (nx, ny) = makepad_render::skin::oct_encode(vec3f(n[0], n[1], n[2]));
        vertices.extend([p[0], p[1], p[2], pack_pair_f16(nx, ny), 0.0, f32::from_bits(u32::MAX), 0.0]);
    }
    trace_packed_asset(renderer, cx, name, vertices, g.indices, mr, pick, programs);
}

fn trace_packed_asset(renderer: &mut Renderer, cx: &mut Cx, name: &str, vertices: Vec<f32>, indices: Vec<u32>, mr: [f32; 2], pick: u32, programs: &[&str]) {
    let mut min = Vec3f::all(f32::INFINITY);
    let mut max = Vec3f::all(f32::NEG_INFINITY);
    for p in vertices.chunks_exact(7) {
        min = vec3f(min.x.min(p[0]), min.y.min(p[1]), min.z.min(p[2]));
        max = vec3f(max.x.max(p[0]), max.y.max(p[1]), max.z.max(p[2]));
    }
    let model = StaticModel { vertices, indices, min, max, material_name: name.into(),
        pbr: PbrMaterial { metallic: mr[0], roughness: mr[1], ..Default::default() },
        texture_uri: None, texture_png: None, detail_png: None, detail_scale: [1.0; 2],
        parts: vec![], ground_ao: None, draw_layers: vec![], prelit: false, anim_parts: vec![],
        driven_parts: vec![], sky: None, liquids: vec![], liquid_ranges: vec![] };
    let asset = renderer.upload_static_preview(cx, PreparedStaticPreview::prepare_with_sidecars(model, None, None).unwrap());
    asset.set_layer_pick_id(cx, name, pick);
    if programs.is_empty() {
        renderer.install_uploaded_static(name, asset);
    } else {
        for &program in programs {
            let mut proxy = asset.clone();
            assert_eq!(proxy.require_traced_layer(cx, name, program, TracedLayerRole::Outside), 1);
            renderer.install_uploaded_static(program, proxy);
        }
    }
}

impl TraceLab {
    fn install_subject(r: &mut Renderer, name: &str, recipe: TracedSurfaceRecipe, inside: bool) {
        if name != "analytic" {
            r.install_traced_surface_for_passes(name.into(), recipe, TracedSurfacePasses { inside: name == "volume" && inside, sun: true, ..TracedSurfacePasses::OUTSIDE });
        } else {
            r.install_traced_outside_preview("analytic".into(), recipe);
        }
    }

    fn new(lab: &mut MaterialLab, cx: &mut Cx, scene: Scene) -> Self {
        let args: Vec<_> = std::env::args().collect();
        let arg = |prefix: &str| args.iter().find_map(|s| s.strip_prefix(prefix));
        let early = scene == Scene::EarlyDepth;
        let trace_series = args.iter().any(|s| s == "--trace-series");
        let volume_series = scene == Scene::Volume && trace_series;
        let series = (early || volume_series) && trace_series;
        let controls = TraceControls {
            camera: VolumeCamera::Outside,
            view: match arg("--trace-view=").unwrap_or("traced") {
                "traced" => SurfaceView::Traced, "mesh" => SurfaceView::Mesh, "waiting" => SurfaceView::Waiting,
                "failed" => SurfaceView::Failed, "omit" => SurfaceView::Omitted, v => panic!("unknown trace view {v}"),
            },
            caster: args.iter().any(|s| s == "--trace-caster"), ao: args.iter().any(|s| s == "--trace-ssao"),
            fog: args.iter().any(|s| s == "--trace-fog"), ssr: args.iter().any(|s| s == "--trace-ssr"),
            depth_probe: 0, cleared: false,
            expensive: early && !series && arg("--trace-work=") == Some("expensive"),
            front: !series && arg("--trace-prefill=") == Some("front"),
        };
        let size = TextureSize::Fixed { width: PASS_W, height: PASS_H };
        let hdr = TextureFormat::RenderRGBAf16 { size: size.clone(), initial: true };
        lab.color_texture = Texture::new_with_format(cx, hdr.clone());
        lab.depth_texture = Texture::new_with_format(cx, TextureFormat::DepthD32Sampled { size: size.clone(), initial: true });
        lab.pass.set_depth_texture(cx, &lab.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        lab.pass.set_keep_camera_matrix(cx, true);
        lab.pass.set_gpu_timing_enabled(cx, early || volume_series);
        cx.set_pick_ids(true);
        let r = &mut lab.renderer;
        r.set_hdr_output(true);
        r.set_auto_exposure(false);
        r.set_bloom(0.0);
        r.set_fxaa(false);
        r.set_output_dither(false);
        r.set_pbr_materials_enabled(true);
        r.set_scene_time(cx, Some(0.0));
        let mut ball = sphere_geometry();
        let mut box_mesh = cube_geometry();
        for g in [&mut ball, &mut box_mesh] {
            for p in &mut g.positions { *p = [2.0 * p[0], 2.0 * p[1] - 1.0, 2.0 * p[2]]; }
        }
        trace_asset(r, cx, "ball", ball, [0.0, 0.5], 2, &[]);
        if scene == Scene::Volume {
            trace_asset(r, cx, "receiver", box_mesh.clone(), [0.0, 0.8], 4, &[]);
            r.set_model_casts_shadow("receiver", false);
        } else {
            trace_asset(r, cx, "box", box_mesh.clone(), [0.0, 0.5], 2, if early { &["analytic", "costly"] } else { &["analytic"] });
        }
        trace_asset(r, cx, "bar", box_mesh, [0.0, 0.8], 3, &[]);
        let (plane_z, slope) = if scene == Scene::Volume { (-1.5, 0.0) } else { (-0.45, 1.4) };
        let normal = vec3f(0.0, slope, 1.0).normalize();
        let mut plane = GeometryData { indices: vec![0, 1, 2, 0, 2, 3], ..Default::default() };
        for (x, y) in [(-4.2,-1.6), (4.2,-1.6), (4.2,1.6), (-4.2,1.6)] {
            plane.positions.push([x, y, plane_z - slope*y]);
            plane.normals.push([normal.x, normal.y, normal.z]);
        }
        let plane_mr = if scene == Scene::Volume { [0.0, 0.8] } else { [0.65, 0.12] };
        trace_asset(r, cx, "plane", plane, plane_mr, 1, &[]);
        // Isolate receiving the separate bar's shadow. Neither reference sphere casts.
        for name in ["ball", "plane"] { r.set_model_casts_shadow(name, false); }
        r.set_csm_config(1024, 20.0);
        r.set_gpu_lightmap_mode(if early { GpuLightmapMode::Off } else { GpuLightmapMode::Realtime });
        let (volume, mut cheap) = if scene == Scene::Volume {
            let (fixture, recipe) = traced_volume::VolumeFixture::install(r, cx, volume_series);
            (Some(fixture), recipe)
        } else {
            let recipe = cx.with_vm(|vm| {
                let mut recipe = TracedSurfaceRecipe::default();
                let color = script_eval!(vm, { mod.widgets.lab_hit_color }).as_object().unwrap();
                recipe.set_hook(vm, live_id!(traced_base), color).unwrap();
                recipe
            });
            (None, recipe)
        };
        if early {
            let mut expensive = cheap.clone();
            cx.with_vm(|vm| {
                let hit = script_eval!(vm, { mod.widgets.lab_expensive_hit }).as_object().unwrap();
                expensive.set_hook(vm, live_id!(traced_hit), hit).unwrap();
            });
            r.install_traced_outside_preview("costly".into(), expensive);
        }
        let held = std::rc::Rc::new(std::cell::Cell::new(false));
        let gate = held.clone();
        cheap.host_vm = Some(std::rc::Rc::new(move |cx, build| !gate.get() && cx.try_with_vm(|vm| build(vm)).is_some()));
        let name = if volume.is_some() { "volume" } else { "analytic" };
        Self::install_subject(r, name, cheap.clone(), true);
        let volume_series = volume_series.then(|| {
            Self::install_subject(r, "volume_no_inside", cheap.clone(), false);
            let iters = arg("--trace-gpu-load-iters=").map_or(380, |v| v.parse().expect("trace GPU load iterations"));
            traced_volume::VolumeSeries::new(cx, iters, &volume.as_ref().unwrap().subject_no_inside)
        });
        let mut world = World::new();
        world.sky = None;
        let sun = if scene == Scene::Volume { vec3f(0.0, 0.5, 1.0) } else { vec3f(0.0, 0.82, 0.57) };
        world.lights.push(Light::Sun { dir: sun.normalize(), color: Vec3f::all(1.0), lux: 1.6, shadow: Default::default() });
        world.lights.push(Light::Sky { top: Vec3f::all(0.75), ground: Vec3f::all(0.45), intensity: 0.8 });
        Self { controls, early, volume_series, volume, outside_name: name, series_block: (early && series).then_some(0),
            captures: arg("--trace-captures=").map(Into::into), revision: 1, installed: None, world,
            inside_draws: 0, availability: SurfaceView::Traced, recipe: cheap, held,
            surface: Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size, initial: true }),
            envspec: Texture::new_with_format(cx, hdr), recording: None, primed: false, rendered: false,
            announced: false, color_read: None, surface_read: None, pick_read: None, files: Vec::new(), writer: None, ssr_frames: None, timed_samples: 0 }
    }

    fn key(&mut self, key: KeyCode) {
        // Finish the requested readbacks before accepting another picture.
        if self.volume_series.is_some() || (self.captures.is_some() && !self.announced) { return; }
        let old = self.controls;
        let c = &mut self.controls;
        match key {
            KeyCode::KeyT => c.view = SurfaceView::Traced, KeyCode::KeyM => c.view = SurfaceView::Mesh,
            KeyCode::KeyW => c.view = SurfaceView::Waiting, KeyCode::KeyF => c.view = SurfaceView::Failed,
            KeyCode::KeyO => c.view = SurfaceView::Omitted, KeyCode::KeyA => c.ao = !c.ao,
            KeyCode::KeyG => c.fog = !c.fog, KeyCode::KeyS => c.ssr = !c.ssr,
            KeyCode::Key1 => c.depth_probe = -1, KeyCode::Key2 => c.depth_probe = 1, KeyCode::Key0 => c.depth_probe = 0,
            KeyCode::KeyC => c.caster = !c.caster, KeyCode::KeyE if self.early => c.expensive = !c.expensive,
            KeyCode::KeyX if self.volume.is_some() => c.cleared = !c.cleared,
            KeyCode::KeyI if self.volume.is_some() => c.camera = VolumeCamera::Inside,
            KeyCode::KeyN if self.volume.is_some() => c.camera = VolumeCamera::Slice,
            KeyCode::KeyV if self.volume.is_some() => c.camera = VolumeCamera::Outside,
            KeyCode::KeyB if self.early => c.front = !c.front, _ => return,
        }
        if self.controls != old { self.revision += 1; }
    }

    fn ball(name: &str, x: f32) -> ModelInstance {
        trace_instance(name, [x, 0.0, 0.0], [0.9; 3], [1.0, 0.22, 0.055])
    }

    fn place_models(&self, r: &mut Renderer) {
        let c = self.controls;
        let plane_color = if self.volume.is_some() { [0.7; 3] } else { [0.12, 0.56, 0.82] };
        let mut placed = vec![
            trace_instance("plane", [0.0; 3], [1.0; 3], plane_color),
            Self::ball("ball", -1.45),
        ];
        if c.caster {
            placed.push(trace_instance("bar", [0.0, 1.85, 1.85], [3.0, 0.085, 0.04], [0.3; 3]));
        }
        if self.early {
            // Identical XY coverage and opaque submission before the attachment in both cases.
            placed.push(trace_instance("bar", [1.15, 0.0, if c.front { 1.15 } else { -2.9 }],
                [0.8, 0.94, 0.015], [0.16, 0.8, 0.12]));
        }
        if c.depth_probe != 0 {
            // At target pixel (centre + 40, 150), the ray hits z = sqrt(r² - x² - y²).
            let z = (0.9_f32.powi(2) - 0.405_f32.powi(2) - 0.005_f32.powi(2)).sqrt();
            for x in [-1.45, 1.45] {
                placed.push(trace_instance("bar", [x + 0.405, -0.005, z + 0.006 * c.depth_probe as f32 - 0.001],
                    [0.035, 0.035, 0.001], [0.16, 0.8, 0.12]));
            }
        }
        if let Some(cube) = self.volume.as_ref().and_then(|v| v.interior_cube(c.camera)) {
            placed.push(cube.clone());
        }
        r.set_models(placed);
    }

    fn configure(&mut self, lab: &mut MaterialLab, cx: &mut Cx) {
        let c = self.controls;
        let r = &mut lab.renderer;
        let previous = self.installed.replace((self.revision, c));
        self.recording = None;
        self.primed = false;
        self.rendered = false;
        self.announced = false;
        self.timed_samples = 0;
        self.ssr_frames = None;
        if let Some(fixture) = &mut self.volume { fixture.set_cleared(cx, c.cleared); }
        if self.volume.is_some() {
            // A cell edit only uploads the changed texels. Keep the same placements,
            // attachments and cascade state for its next picture.
            if previous.is_some_and(|(_, old)| c == TraceControls { cleared: c.cleared, ..old }) { return; }
        }
        r.set_ssr(makepad_render::SsrSettings { strength: if c.ssr { 1.0 } else { 0.0 }, max_distance: 4.0 });
        self.world.environment.fog = if c.fog {
            makepad_scene::Fog::Exp2 { color: vec3f(0.12, 0.45, 0.85), density: 0.12 }
        } else { makepad_scene::Fog::None };
        self.place_models(r);
        // Hold a fresh install before its VM is entered. T releases that very
        // generation; after a terminal failure it starts a fresh one.
        if c.view != self.availability {
            match c.view {
                SurfaceView::Waiting => {
                    self.held.set(true);
                    Self::install_subject(r, self.outside_name, self.recipe.clone(), true);
                }
                SurfaceView::Failed => {
                    self.held.set(false);
                    let mut invalid = self.recipe.clone();
                    let mut bindings = invalid.textures.to_vec();
                    bindings.push((live_id!(missing_lab_input), self.surface.clone()));
                    invalid.textures = bindings.into();
                    Self::install_subject(r, self.outside_name, invalid, true);
                }
                SurfaceView::Traced => {
                    self.held.set(false);
                    if self.availability == SurfaceView::Failed {
                        Self::install_subject(r, self.outside_name, self.recipe.clone(), true);
                    }
                }
                _ => {}
            }
            if matches!(c.view, SurfaceView::Traced | SurfaceView::Waiting | SurfaceView::Failed) {
                self.availability = c.view;
            }
        }
        let subject = match c.view {
            SurfaceView::Mesh => Some("ball"),
            _ => self.program(),
        };
        r.set_world_attachments(subject.into_iter().map(|name| {
            if name == "volume" {
                self.volume.as_ref().unwrap().subject.clone()
            } else { Self::ball(name, 1.45) }
        }).collect());
        lab.pass.clear_color_textures(cx);
        lab.pass.set_color_texture(cx, &lab.color_texture, DrawPassClearColor::ClearWith(vec4(0.025, 0.03, 0.05, 1.0)));
        if c.ssr {
            assert!(r.ssr_available(cx), "traced SSR fixture needs mixed-output passes");
            for tex in [&self.surface, &self.envspec] {
                lab.pass.add_color_texture(cx, tex, DrawPassClearColor::ClearWith(Vec4f::default()));
            }
            log!("material lab: SSR revision {} waiting for the plane's reflection outputs and history target", self.revision);
        }
    }

    fn program(&self) -> Option<&'static str> {
        match self.controls.view {
            SurfaceView::Traced if self.controls.expensive => Some("costly"),
            SurfaceView::Traced | SurfaceView::Waiting | SurfaceView::Failed => Some(self.outside_name),
            _ => None,
        }
    }

    fn draw(&mut self, lab: &mut MaterialLab, cx: &mut Cx2d, rect: Rect) {
        if self.installed.map(|(revision, _)| revision) != Some(self.revision) { self.configure(lab, cx.cx); }
        let mut tag = if self.announced { self.revision } else { 0 };
        if let Some(series) = &mut self.volume_series {
            let camera;
            (camera, tag) = series.next_frame(cx.cx, &mut lab.renderer, self.announced);
            if self.controls.camera != camera {
                self.controls.camera = camera;
                self.place_models(&mut lab.renderer);
            }
        }
        let c = self.controls;
        let program = self.program();
        // One history step is a completed trace, resolve and apply. Re-recording
        // on every UI draw flips the history textures even when Metal coalesces
        // those draws or one of the post pipelines is still compiling.
        if let (Some(frames), Some(recorded)) = (self.ssr_frames, self.recording) {
            let parent = lab.pass.parent(cx.cx);
            cx.make_child_pass(&lab.pass);
            let CxDrawPassParent::DrawPass(first) = parent else { panic!("missing SSR chain") };
            lab.pass.set_pass_parent(cx.cx, first);
            let host = cx.current_pass_id();
            assert_ne!(first, host, "scene bypassed the SSR passes");
            if !self.rendered && Self::chain_painted(cx.cx, lab.pass.draw_pass_id(), host, recorded) {
                self.ssr_frames = Some(frames + 1);
                self.rendered = frames + 1 == 8;
                if self.rendered {
                    self.request_capture(lab, cx.cx);
                } else {
                    self.recording = None;
                }
            }
            if self.recording.is_some() {
                // Keep the final applied texture and its inputs together until both
                // the colour copy and the scene's independent pick replay have landed.
                lab.draw_bg.draw_abs(cx, rect);
                lab.area = lab.draw_bg.area();
                self.finish_picture(lab, cx, program);
                if self.announced { self.recording = None; }
                return;
            }
        }
        let size = dvec2(PASS_W as f64, PASS_H as f64);
        let local = Rect { pos: dvec2(0.0, 0.0), size };
        let (eye, target, projection) = if let Some(room) = &self.volume {
            let (eye, target, fov) = room.camera(c.camera);
            (eye, target, Mat4f::perspective(fov, PASS_W as f32 / PASS_H as f32, 0.1, 20.0))
        } else {
            // The orthographic depth fixtures use 100 texels per metre.
            let mut projection = Mat4f::identity();
            projection.v[0] = 0.25;
            projection.v[5] = 2.0 / 3.0;
            projection.v[10] = -1.0 / 16.0;
            (vec3f(0.0, 0.0, 8.0), Vec3f::default(), projection)
        };
        let scene = SceneState3D { time: 0.0, camera_pos: eye, view: Mat4f::look_at(eye, target, vec3f(0.0, 1.0, 0.0)),
            projection, viewport_rect: local };
        lab.pass.set_size(cx, size);
        lab.pass.set_dpi_factor(cx, 1.0);
        cx.make_child_pass(&lab.pass);
        lab.renderer.set_pass_camera(cx.cx, &lab.pass, &scene);
        if self.early || self.volume_series.is_some() { lab.pass.set_gpu_time_tag(cx.cx, tag); }
        // Post and AO receive logical points and multiply by the host's DPI.
        let post_size = size / cx.current_dpi_factor();
        lab.renderer.run_screen_ao(cx, &lab.pass, &lab.depth_texture, post_size, makepad_render::ScreenAoParams {
            strength: if c.ao { 1.0 } else { 0.0 }, radius: 0.6, tier: makepad_render::cortao::CortaoTier::High,
            resolution: makepad_render::cortao::CortaoResolution::Full, history: false, frame: 0, reset: false,
        });
        lab.renderer.run_ssr(cx, &lab.pass, &lab.depth_texture, post_size);
        cx.begin_pass(&lab.pass, Some(1.0));
        lab.pass.set_size(cx, size);
        lab.pass.set_dpi_factor(cx, 1.0);
        let mut draws = SceneDraws { cube: Some(&mut lab.draw_cube), alpha: Some(&mut lab.draw_alpha), sky: &mut lab.draw_sky,
            sky_analytic: None, terrain: Some(&mut lab.draw_terrain), shadow: None, shadow_sdf: None, firework: None,
            flare: None, water: None, screen: None, screen_instances: &[], view_model: None };
        let stats = lab.renderer.draw_scene_full(&mut Cx3d::new(cx.cx), &mut lab.draw_list, &mut draws, &self.world, scene, None, Some(&mut lab.draw_models));
        self.inside_draws = stats.traced_inside_draws;
        cx.end_pass(&lab.pass);
        if let CxDrawPassParent::DrawPass(host) = lab.pass.parent(cx.cx) {
            if let Some(first) = lab.renderer.run_post(cx, post_size, &lab.color_texture, host) {
                lab.pass.set_pass_parent(cx.cx, first);
            }
        }
        if let Some(series) = &mut self.volume_series {
            series.draw_clock_hold(cx, &lab.color_texture);
            if tag != 0 {
                assert_eq!(stats.world_attachment_draws, 1, "missing volume at tag {tag}");
                series.record_submission(tag, self.inside_draws);
            }
        }
        lab.draw_bg.draw_vars.set_texture(0, &lab.color_texture);
        lab.renderer.bind_composite(&mut lab.draw_bg, vec2f(PASS_W as f32, PASS_H as f32));
        lab.draw_bg.draw_abs(cx, rect);
        lab.area = lab.draw_bg.area();
        let rejected = program.is_some_and(|name| lab.renderer.traced_surface_failure(name).is_some());
        let submitted = match c.view {
            _ if rejected => stats.world_attachment_draws == 0,
            SurfaceView::Traced | SurfaceView::Mesh => stats.world_attachment_draws == 1,
            SurfaceView::Waiting => lab.renderer.traced_surface_state(self.outside_name) == TracedSurfaceState::Waiting && stats.world_attachment_draws == 0,
            SurfaceView::Omitted => stats.world_attachment_draws == 0,
            SurfaceView::Failed => false,
        };
        // Waiting and failed go through this gate too. Only the ordinary scene and
        // the requested outside submission need to have painted, never all four passes.
        if !self.rendered && submitted && lab.renderer.items_ready(cx.cx) && lab.renderer.scene_settled() {
            if self.ssr_frames.is_some() {
                self.recording = Some(cx.repaint_id());
            } else {
                let recorded = *self.recording.get_or_insert_with(|| cx.repaint_id());
                if lab.pass.painted_whole_once_since(cx.cx, recorded) {
                    self.recording = None;
                    if self.primed && c.ssr {
                        // During first-use compilation the plane can still be drawn
                        // by the stock colour-only variant. Keep recording warmup
                        // frames until its draw writes both reflection attachments.
                        if Self::ssr_warmed(lab, cx.cx) {
                            lab.renderer.set_ssr(Default::default());
                            lab.renderer.set_ssr(makepad_render::SsrSettings { strength: 1.0, max_distance: 4.0 });
                            self.ssr_frames = Some(0);
                            log!("material lab: SSR receiver ready, starting history for revision {}", self.revision);
                        }
                    } else {
                        self.rendered = self.primed;
                    }
                    self.primed = true;
                }
            }
            if self.rendered { self.request_capture(lab, cx.cx); }
        } else if !self.rendered { self.recording = None; self.primed = false; self.ssr_frames = None; }
        self.finish_picture(lab, cx, program);
    }

    fn ssr_warmed(lab: &MaterialLab, cx: &Cx) -> bool {
        // This frame's applied target becomes the next frame's sampled history.
        let applied = lab.draw_bg.draw_vars.texture_slots[0].as_ref().unwrap();
        if applied.texture_id() == lab.color_texture.texture_id() || applied.allocated_size(cx).is_none() {
            return false;
        }
        let items = &cx.draw_lists[lab.draw_list.id()].draw_items;
        (0..items.len()).filter_map(|i| items[i].kind.draw_call()).any(|call| {
            let plane = call.geometry_id.is_some_and(|id| matches!(cx.geometries[id].pick_ranges.as_slice(), [[0, 1]]));
            plane && cx.draw_shaders[call.draw_shader_id.index].mapping.fragment_outputs & 0b111 == 0b111
        })
    }

    fn chain_painted(cx: &Cx, mut id: DrawPassId, host: DrawPassId, recorded: u64) -> bool {
        while id != host {
            if !id.holds_whole_paint_since(cx, recorded, u64::MAX) { return false; }
            let CxDrawPassParent::DrawPass(next) = cx.passes[id].parent else { panic!("detached SSR pass") };
            id = next;
        }
        true
    }

    fn request_capture(&mut self, lab: &MaterialLab, cx: &mut Cx) {
        let Some(dir) = &self.captures else { return };
        if let Some(volume) = &self.volume {
            let uniforms = &cx.passes[lab.pass.draw_pass_id()].pass_uniforms;
            let path = dir.join(format!("{}.volume", self.revision));
            self.files.push((path, volume.metadata(uniforms.camera_view, uniforms.camera_projection, self.controls.camera)));
        }
        let tex = lab.draw_bg.draw_vars.texture_slots[0].as_ref().unwrap();
        let ssr = self.controls.ssr;
        if ssr {
            assert_ne!(tex.texture_id(), lab.color_texture.texture_id(), "SSR must capture the applied HDR target");
            self.surface_read = Some(self.surface.read_back(cx, ReadbackRequest::default()).unwrap());
        }
        self.color_read = Some(tex.read_back(cx, ReadbackRequest { next_render: !ssr }).unwrap());
        self.pick_read = Some(cx.request_pick(&lab.color_texture));
    }

    fn finish_picture(&mut self, lab: &mut MaterialLab, cx: &mut Cx2d, program: Option<&str>) {
        let c = self.controls;
        if let Some(dir) = &self.captures {
            if let Some(ticket) = self.surface_read {
                if let Some(read) = cx.try_take_texture_readbacks_for(&[ticket]).pop() {
                    assert_eq!((read.width, read.height), (PASS_W, PASS_H));
                    self.files.push((dir.join(format!("{}.surface", self.revision)), read.data.unwrap()));
                    self.surface_read = None;
                }
            }
            if let Some(ticket) = self.color_read {
                if let Some(read) = cx.try_take_texture_readbacks_for(&[ticket]).pop() {
                    assert_eq!((read.width, read.height), (PASS_W, PASS_H));
                    self.files.push((dir.join(format!("{}.rgba16f", self.revision)), read.data.unwrap()));
                    self.color_read = None;
                }
            }
            if let Some(ticket) = self.pick_read {
                if let Some(read) = cx.take_pick(ticket) {
                    let read = read.expect("scene pick replay");
                    assert_eq!((read.width, read.height), (PASS_W, PASS_H));
                    self.files.push((dir.join(format!("{}.pick", self.revision)), read.bytes));
                    self.pick_read = None;
                }
            }
        }
        if self.files.len() == 2 + usize::from(c.ssr) + usize::from(self.volume.is_some()) {
            let files = self.files.clone();
            if let Ok(task) = cx.task_pool().submit(makepad_platform::thread::Class::Background, move || {
                for (path, bytes) in files { std::fs::write(path, bytes.as_slice())?; }
                Ok(())
            }) {
                self.writer = Some(task);
                self.files.clear();
            }
        }
        if let Some(result) = self.writer.as_mut().and_then(|task| task.try_take()) {
            result.expect("capture worker").expect("write trace capture");
            self.writer = None;
        }
        if self.rendered && !self.announced && self.color_read.is_none() && self.surface_read.is_none() && self.pick_read.is_none() && self.files.is_empty() && self.writer.is_none() {
            self.announced = true;
            let state = program.map(|name| lab.renderer.traced_surface_state(name));
            let (view, reason) = if let Some(TracedSurfaceState::Failed(reason)) = &state {
                if self.captures.is_some() && c.view == SurfaceView::Failed {
                    assert!(reason.contains(&live_id!(missing_lab_input).to_string()), "unexpected terminal reason: {reason}");
                }
                (SurfaceView::Failed, reason.as_str())
            } else { (c.view, "") };
            let volumes = trace_volumes_available(cx.cx);
            log!("material lab: traced rendered {} {:?} caster={} ssao={} fog={} ssr={} work={} prefill={} inside={} backend={} volumes={} reason={}",
                self.revision, view, c.caster, c.ao, c.fog, c.ssr,
                if c.expensive { "expensive" } else { "cheap" }, if c.front { "front" } else { "behind" },
                self.inside_draws, cx.gpu_backend().name(), volumes, reason);
        }
        const SERIES_BLOCKS: usize = 6 * 4;
        let rejected = program.is_some_and(|name| lab.renderer.traced_surface_failure(name).is_some());
        if let Some(series) = &mut self.volume_series {
            series.finish_frame(cx, &lab.pass);
            return;
        }
        let measuring = self.early && self.series_block != Some(SERIES_BLOCKS)
            && c.view == SurfaceView::Traced && !rejected;
        if !self.announced || measuring { cx.new_next_frame(); }
        if measuring {
            for (sample_tag, ms) in lab.pass.take_gpu_time_samples(cx.cx) {
                if sample_tag != self.revision || !self.announced { continue; }
                self.timed_samples += 1;
                if self.timed_samples > 8 && (self.series_block.is_none() || self.timed_samples <= 24) {
                    log!("traced-early-depth backend={} revision={} work={} prefill={} sample={} scene_gpu_ms={:.6}",
                        cx.gpu_backend().name(), self.revision, if c.expensive { "expensive" } else { "cheap" },
                        if c.front { "front" } else { "behind" }, self.timed_samples - 8, ms);
                }
            }
            if let Some(block) = self.series_block {
                if self.timed_samples >= 24 {
                    // Six interleaved rounds; reverse order on every other round.
                    let phases = [(false,false),(true,false),(true,true),(false,true),
                                  (false,true),(true,true),(true,false),(false,false)];
                    let next = block + 1;
                    self.series_block = Some(next);
                    if next == SERIES_BLOCKS {
                        lab.pass.set_gpu_timing_enabled(cx.cx, false);
                        log!("traced-early-depth complete rounds=6 samples_per_condition_per_round=16");
                    } else {
                        (self.controls.expensive, self.controls.front) = phases[next % phases.len()];
                        self.revision += 1;
                    }
                }
            }
        }
    }
}
