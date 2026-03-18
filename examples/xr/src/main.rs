pub use makepad_widgets;

use makepad_gltf::load_gltf_from_bytes;
use makepad_xr::XrScene as EngineXrScene;
use makepad_widgets::makepad_platform::permission::{Permission, PermissionStatus};
use makepad_widgets::*;
use std::{path::PathBuf, rc::Rc};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.ExampleXrSceneBase = #(ExampleXrScene::register_widget(vm))
    mod.widgets.ExampleXrScene = set_type_default() do mod.widgets.ExampleXrSceneBase{
        scene := mod.widgets.XrScene{}
        src: crate_resource("self://resources/DamagedHelmet.glb")
        env_src: crate_resource("self://resources/royal_esplanade_4k.jpg")
        draw_pbr +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.22
            spec_power: 128.0
            spec_strength: 0.9
            env_intensity: 1.8
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 820)
                body +: {
                    phase_view := AdaptiveView{
                        width: Fill
                        height: Fill
                        retain_unused_variants: false

                        Preflight := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            align: Align{x: 0.5 y: 0.5}
                            padding: Inset{left: 36 right: 36 top: 36 bottom: 36}
                            spacing: 14
                            show_bg: true
                            draw_bg +: {
                                color_top: uniform(#x0b1422)
                                color_bottom: uniform(#x051018)
                                color_glow: uniform(#x1b4663)
                                pixel: fn() {
                                    let uv = self.pos;
                                    let base = mix(self.color_top, self.color_bottom, uv.y);
                                    let glow = smoothstep(0.72, 0.0, length(uv - vec2(0.18, 0.24)));
                                    return mix(base, self.color_glow, glow * 0.24);
                                }
                            }

                            panel := RoundedView{
                                width: 560
                                height: Fit
                                flow: Down
                                spacing: 10
                                padding: Inset{left: 22 right: 22 top: 20 bottom: 20}
                                draw_bg.color: #x09131cdd
                                draw_bg.radius: 16.0

                                title := H1{
                                    text: "XR Preflight"
                                    draw_text.color: #xeff7ff
                                }

                                detail_label := Label{
                                    width: Fill
                                    text: "Allow Quest scene access here before starting XR. The passthrough depth path uses Meta's scene permission for environment depth and occlusion."
                                    draw_text.color: #xb8c8d8
                                }

                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 10

                                    allow_button := Button{
                                        width: Fill
                                        text: "Allow Quest Scene Access"
                                    }

                                    start_xr_button := Button{
                                        width: Fill
                                        text: "Start XR"
                                    }
                                }

                                status_label := Label{
                                    width: Fill
                                    text: "Checking startup requirements."
                                    draw_text.color: #x8fe4d6
                                }
                            }
                        }

                        XrRuntime := View{
                            width: 0
                            height: 0
                        }
                    }
                }
            }

            xr_scene := mod.widgets.ExampleXrScene{}
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AppPhase {
    #[default]
    Preflight,
    XrRuntime,
}

const EXAMPLE_HELMET_TARGET_HEIGHT: f32 = 0.40;
const EXAMPLE_HELMET_HEIGHT_OFFSET: f32 = 0.16;
const EXAMPLE_HELMET_SIDE_OFFSET: f32 = 0.10;
const EXAMPLE_HELMET_FORWARD_OFFSET: f32 = -0.14;
const EXAMPLE_HELMET_ROTATION_Y: f32 = 1.2;
const EXAMPLE_CUBE_COLORS: &[[f32; 3]] = &[
    [0.90, 0.30, 0.25],
    [0.25, 0.75, 0.45],
    [0.30, 0.50, 0.90],
    [0.95, 0.75, 0.20],
    [0.80, 0.40, 0.85],
    [0.20, 0.80, 0.80],
    [0.95, 0.55, 0.25],
    [0.60, 0.85, 0.35],
];
const EXAMPLE_PLATFORM_COLOR: [f32; 3] = [0.10, 0.14, 0.18];
const EXAMPLE_CUBE_HALF_EXTENT: f32 = 0.020;
const EXAMPLE_PLATFORM_HALF_WIDTH: f32 = 0.64;
const EXAMPLE_PLATFORM_HALF_HEIGHT: f32 = 0.006;
const EXAMPLE_PLATFORM_HALF_DEPTH: f32 = 0.16;
const EXAMPLE_PLATFORM_SPAWN_GAP: f32 = 0.0;
const EXAMPLE_WALL_BRICK_HALF_WIDTH: f32 = EXAMPLE_CUBE_HALF_EXTENT * 2.0;
const EXAMPLE_WALL_BRICK_HALF_HEIGHT: f32 = EXAMPLE_CUBE_HALF_EXTENT;
const EXAMPLE_WALL_BRICK_HALF_DEPTH: f32 = EXAMPLE_CUBE_HALF_EXTENT;
const EXAMPLE_WALL_FULL_ROW_BRICKS: usize = 5;
const EXAMPLE_WALL_SHORT_ROW_BRICKS: usize = 4;
const EXAMPLE_WALL_ROWS: usize = 4;
const EXAMPLE_WALL_ROTATION_Y: f32 = std::f32::consts::FRAC_PI_2;
const EXAMPLE_PLATFORM_ROUND_RADIUS: f32 = 0.005;
const EXAMPLE_BRICK_VISUAL_SCALE: f32 = 0.98;

#[derive(Script, ScriptHook, Widget)]
pub struct ExampleXrScene {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    scene: EngineXrScene,
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,
    #[live]
    src: Option<ScriptHandleRef>,
    #[live]
    env_src: Option<ScriptHandleRef>,
    #[rust]
    scene_center: Option<Vec3f>,
    #[rust]
    scene_forward: Vec3f,
    #[rust]
    scene_right: Vec3f,
    #[rust]
    helmet_renderer: Option<GltfRenderer>,
    #[rust]
    helmet_load_logged: bool,
    #[rust]
    helmet_draw_logged: bool,
    #[rust]
    helmet_draw_failed_logged: bool,
    #[rust]
    helmet_model_center: Vec3f,
    #[rust(1.0)]
    helmet_fit_scale: f32,
    #[rust]
    loaded_src_handle: Option<ScriptHandle>,
    #[rust]
    loaded_env_handle: Option<ScriptHandle>,
}

enum ResourceResolve {
    Ready {
        handle: ScriptHandle,
        abs_path: PathBuf,
        data: Rc<Vec<u8>>,
    },
    Pending {
        handle: ScriptHandle,
    },
    Error {
        handle: ScriptHandle,
    },
    Missing,
}

impl ExampleXrScene {
    fn resource_metadata_by_handle(cx: &mut Cx, handle: ScriptHandle) -> Option<(PathBuf, bool)> {
        let resources = cx.script_data.resources.resources.borrow();
        let resource = resources
            .iter()
            .find(|resource| resource.handle == handle)?;
        Some((PathBuf::from(&resource.abs_path), resource.is_error()))
    }

    fn resolve_resource(cx: &mut Cx, handle_ref: &ScriptHandleRef) -> ResourceResolve {
        let handle = handle_ref.as_handle();

        if let Some(data) = cx.get_resource(handle) {
            let abs_path = Self::resource_metadata_by_handle(cx, handle)
                .map(|metadata| metadata.0)
                .unwrap_or_else(|| PathBuf::from("resource"));
            return ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            };
        }

        cx.load_script_resource(handle);

        if let Some(data) = cx.get_resource(handle) {
            let abs_path = Self::resource_metadata_by_handle(cx, handle)
                .map(|metadata| metadata.0)
                .unwrap_or_else(|| PathBuf::from("resource"));
            return ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            };
        }

        if let Some((_, is_error)) = Self::resource_metadata_by_handle(cx, handle) {
            if is_error {
                return ResourceResolve::Error { handle };
            }
            return ResourceResolve::Pending { handle };
        }

        ResourceResolve::Missing
    }

    fn scene_basis(state: &XrState) -> (Vec3f, Vec3f) {
        let mut forward = state.vec_in_head_space(vec3(0.0, 0.0, -1.0)) - state.head_pose.position;
        forward.y = 0.0;
        if forward.length() <= 1.0e-4 {
            forward = vec3f(0.0, 0.0, -1.0);
        } else {
            forward = forward.normalize();
        }
        let right = vec3f(-forward.z, 0.0, forward.x);
        (forward, right)
    }

    fn ensure_demo_scene(&mut self, state: &XrState) {
        if self.scene_center.is_none() {
            let (forward, right) = Self::scene_basis(state);
            self.scene_forward = forward;
            self.scene_right = right;
        }
        if self.scene.ensure_scene(state) || self.scene_center.is_none() {
            self.scene_center = Some(EngineXrScene::default_scene_center(state));
        }
        if self.scene.dynamic_box_count() != 0 {
            return;
        }

        let center = if let Some(center) = self.scene_center {
            center
        } else {
            let center = EngineXrScene::default_scene_center(state);
            self.scene_center = Some(center);
            center
        };

        let scene_rotation =
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), EXAMPLE_WALL_ROTATION_Y);
        let platform_pose = Pose::new(
            scene_rotation,
            center + vec3f(0.0, -EXAMPLE_PLATFORM_HALF_HEIGHT, 0.0),
        );
        self.scene.spawn_fixed_box(
            platform_pose,
            vec3f(
                EXAMPLE_PLATFORM_HALF_WIDTH,
                EXAMPLE_PLATFORM_HALF_HEIGHT,
                EXAMPLE_PLATFORM_HALF_DEPTH,
            ),
            0.9,
        );

        let brick_half_extents = vec3f(
            EXAMPLE_WALL_BRICK_HALF_WIDTH,
            EXAMPLE_WALL_BRICK_HALF_HEIGHT,
            EXAMPLE_WALL_BRICK_HALF_DEPTH,
        );
        let brick_width = EXAMPLE_WALL_BRICK_HALF_WIDTH * 2.0 + EXAMPLE_PLATFORM_SPAWN_GAP;
        let brick_height = EXAMPLE_WALL_BRICK_HALF_HEIGHT * 2.0 + EXAMPLE_PLATFORM_SPAWN_GAP;
        let row_axis = vec3f(0.0, 0.0, 1.0);
        for row in 0..EXAMPLE_WALL_ROWS {
            let bricks_in_row = if row % 2 == 0 {
                EXAMPLE_WALL_FULL_ROW_BRICKS
            } else {
                EXAMPLE_WALL_SHORT_ROW_BRICKS
            };
            let row_center_offset = (bricks_in_row as f32 - 1.0) * 0.5;
            for brick in 0..bricks_in_row {
                let brick_center = center
                    + row_axis * ((brick as f32 - row_center_offset) * brick_width)
                    + vec3f(
                        0.0,
                        EXAMPLE_WALL_BRICK_HALF_HEIGHT
                            + EXAMPLE_PLATFORM_SPAWN_GAP
                            + row as f32 * brick_height,
                        0.0,
                    );
                self.scene
                    .spawn_dynamic_box(Pose::new(scene_rotation, brick_center), brick_half_extents);
            }
        }
    }

    fn helmet_bounds(renderer: &GltfRenderer) -> Option<(Vec3f, Vec3f)> {
        let mut min = vec3f(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = vec3f(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        let mut found = false;

        for object in &renderer.draw_objects {
            for x in [object.local_bounds_min.x, object.local_bounds_max.x] {
                for y in [object.local_bounds_min.y, object.local_bounds_max.y] {
                    for z in [object.local_bounds_min.z, object.local_bounds_max.z] {
                        let corner = object
                            .world_transform
                            .transform_vec4(vec4(x, y, z, 1.0))
                            .to_vec3f();
                        min.x = min.x.min(corner.x);
                        min.y = min.y.min(corner.y);
                        min.z = min.z.min(corner.z);
                        max.x = max.x.max(corner.x);
                        max.y = max.y.max(corner.y);
                        max.z = max.z.max(corner.z);
                        found = true;
                    }
                }
            }
        }

        found.then_some((min, max))
    }

    fn transformed_bounds(
        renderer: &GltfRenderer,
        transform: Mat4f,
    ) -> Option<(Vec3f, Vec3f)> {
        let mut min = vec3f(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = vec3f(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        let mut found = false;

        for object in &renderer.draw_objects {
            let object_transform = Mat4f::mul(&transform, &object.world_transform);
            for x in [object.local_bounds_min.x, object.local_bounds_max.x] {
                for y in [object.local_bounds_min.y, object.local_bounds_max.y] {
                    for z in [object.local_bounds_min.z, object.local_bounds_max.z] {
                        let corner = object_transform
                            .transform_vec4(vec4(x, y, z, 1.0))
                            .to_vec3f();
                        min.x = min.x.min(corner.x);
                        min.y = min.y.min(corner.y);
                        min.z = min.z.min(corner.z);
                        max.x = max.x.max(corner.x);
                        max.y = max.y.max(corner.y);
                        max.z = max.z.max(corner.z);
                        found = true;
                    }
                }
            }
        }

        found.then_some((min, max))
    }

    fn update_helmet_fit_from_renderer(&mut self, renderer: &GltfRenderer) {
        if let Some((min, max)) = Self::helmet_bounds(renderer) {
            self.helmet_model_center = vec3f(
                (min.x + max.x) * 0.5,
                (min.y + max.y) * 0.5,
                (min.z + max.z) * 0.5,
            );
            let model_height = (max.y - min.y).max(0.0001);
            self.helmet_fit_scale = EXAMPLE_HELMET_TARGET_HEIGHT / model_height;
        } else {
            self.helmet_model_center = renderer.scene_center;
            self.helmet_fit_scale = 1.0;
        }
    }

    fn helmet_world_center(&self) -> Vec3f {
        self.scene_center.unwrap_or(vec3f(0.0, 0.0, 0.0))
            + self.scene_right * EXAMPLE_HELMET_SIDE_OFFSET
            + self.scene_forward * EXAMPLE_HELMET_FORWARD_OFFSET
            + vec3f(0.0, EXAMPLE_HELMET_HEIGHT_OFFSET, 0.0)
    }

    fn ensure_env_loaded(&mut self, cx: &mut Cx2d) {
        let Some(handle_ref) = self.env_src.as_ref() else {
            return;
        };
        let handle = handle_ref.as_handle();
        if self.loaded_env_handle == Some(handle) {
            return;
        }

        match Self::resolve_resource(cx, handle_ref) {
            ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            } => {
                if let Err(err) =
                    self.draw_pbr
                        .load_default_env_equirect_from_bytes(cx, &data, Some(&abs_path))
                {
                    log!("XR helmet env load failed: {err}");
                }
                self.loaded_env_handle = Some(handle);
            }
            ResourceResolve::Error { handle } => {
                self.loaded_env_handle = Some(handle);
            }
            ResourceResolve::Pending { handle } => {
                let _ = handle;
            }
            ResourceResolve::Missing => {}
        }
    }

    fn ensure_helmet_loaded(&mut self, cx: &mut Cx2d) {
        let Some(handle_ref) = self.src.as_ref() else {
            return;
        };
        let handle = handle_ref.as_handle();
        if self.loaded_src_handle == Some(handle) {
            return;
        }

        match Self::resolve_resource(cx, handle_ref) {
            ResourceResolve::Ready {
                handle,
                abs_path,
                data,
            } => match load_gltf_from_bytes(&data, abs_path.parent()) {
                Ok(mut loaded) => {
                    loaded.source_path = Some(abs_path.clone());
                    loaded.base_dir = abs_path.parent().map(PathBuf::from);
                    let decoded_meshes = match GltfDecodedMeshes::decode_all(&loaded) {
                        Ok(decoded_meshes) => decoded_meshes,
                        Err(err) => {
                            log!("XR helmet decode failed: {err}");
                            self.helmet_renderer = None;
                            self.loaded_src_handle = Some(handle);
                            return;
                        }
                    };
                    let renderer = match GltfRenderer::from_loaded_predecoded(
                        &mut self.draw_pbr,
                        cx,
                        &loaded,
                        &decoded_meshes,
                    ) {
                        Ok(renderer) => renderer,
                        Err(err) => {
                            log!("XR helmet upload failed: {err}");
                            self.helmet_renderer = None;
                            self.loaded_src_handle = Some(handle);
                            return;
                        }
                    };
                    self.update_helmet_fit_from_renderer(&renderer);
                    if !self.helmet_load_logged {
                        log!(
                            "XR helmet load ok: draw_objects={} materials={} textures={} fit_scale={:.3} model_center=({:.3}, {:.3}, {:.3})",
                            renderer.draw_objects.len(),
                            renderer.materials.len(),
                            renderer.textures.len(),
                            self.helmet_fit_scale,
                            self.helmet_model_center.x,
                            self.helmet_model_center.y,
                            self.helmet_model_center.z,
                        );
                        self.helmet_load_logged = true;
                    }
                    self.helmet_renderer = Some(renderer);
                    self.loaded_src_handle = Some(handle);
                }
                Err(err) => {
                    log!("XR helmet load failed: {err}");
                    self.helmet_renderer = None;
                    self.loaded_src_handle = Some(handle);
                }
            },
            ResourceResolve::Error { handle } => {
                self.helmet_renderer = None;
                self.loaded_src_handle = Some(handle);
            }
            ResourceResolve::Pending { handle } => {
                let _ = handle;
            }
            ResourceResolve::Missing => {}
        }
    }

    fn helmet_base_transform(&self) -> Mat4f {
        let world_center = self.helmet_world_center();
        let placement = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), EXAMPLE_HELMET_ROTATION_Y),
            world_center,
        )
        .to_mat4();
        let centered_scale = Mat4f::scaled_translation(
            self.helmet_fit_scale,
            vec3f(
                -self.helmet_model_center.x * self.helmet_fit_scale,
                -self.helmet_model_center.y * self.helmet_fit_scale,
                -self.helmet_model_center.z * self.helmet_fit_scale,
            ),
        );
        Mat4f::mul(&placement, &centered_scale)
    }

    fn helmet_transform(&self) -> Mat4f {
        let base_transform = self.helmet_base_transform();
        let Some(renderer) = self.helmet_renderer.as_ref() else {
            return base_transform;
        };
        let Some((min, max)) = Self::transformed_bounds(renderer, base_transform) else {
            return base_transform;
        };
        let bounds_center = vec3f(
            (min.x + max.x) * 0.5,
            (min.y + max.y) * 0.5,
            (min.z + max.z) * 0.5,
        );
        let world_center = self.helmet_world_center();
        let correction = world_center - bounds_center;
        Mat4f::mul(&Mat4f::translation(correction), &base_transform)
    }

    fn prepare_helmet_draw(
        &mut self,
        cx: &mut Cx2d,
        state: &XrState,
        camera_env_atlas: Option<Texture>,
    ) {
        self.draw_pbr.begin();
        self.draw_pbr.set_use_pass_camera(true);
        self.draw_pbr.set_depth_clip(1.0);
        self.draw_pbr.set_base_color_texture(None);
        self.draw_pbr.set_metal_roughness_texture(None);
        self.draw_pbr.set_normal_texture(None);
        self.draw_pbr.set_occlusion_texture(None);
        self.draw_pbr.set_emissive_texture(None);
        if let Some(env_atlas) = camera_env_atlas {
            self.draw_pbr.set_env_texture(None);
            self.draw_pbr.set_env_atlas_texture(Some(env_atlas));
        } else {
            self.draw_pbr.set_env_atlas_texture(None);
            let env_texture = self.draw_pbr.default_env_texture(cx);
            self.draw_pbr.set_env_texture(Some(env_texture));
        }
        self.draw_pbr.camera_pos = state.head_pose.position;
    }

    fn draw_demo_platform(&mut self, cx: &mut Cx2d) {
        let Some(center) = self.scene_center else {
            return;
        };
        let scene_rotation =
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), EXAMPLE_WALL_ROTATION_Y);
        self.scene.draw_rounded_cube(
            cx,
            Pose::new(
                scene_rotation,
                center + vec3f(0.0, -EXAMPLE_PLATFORM_HALF_HEIGHT, 0.0),
            ),
            vec3f(
                EXAMPLE_PLATFORM_HALF_WIDTH,
                EXAMPLE_PLATFORM_HALF_HEIGHT,
                EXAMPLE_PLATFORM_HALF_DEPTH,
            ),
            EXAMPLE_PLATFORM_ROUND_RADIUS,
            vec4(
                EXAMPLE_PLATFORM_COLOR[0],
                EXAMPLE_PLATFORM_COLOR[1],
                EXAMPLE_PLATFORM_COLOR[2],
                1.0,
            ),
            0.85,
        );
    }

    fn draw_demo_bodies(&mut self, cx: &mut Cx2d) {
        let bodies = self.scene.dynamic_box_states();
        self.scene.draw_pose_boxes(cx, bodies.into_iter().enumerate().map(|(index, (pose, half_extents))| {
            let color = EXAMPLE_CUBE_COLORS[index % EXAMPLE_CUBE_COLORS.len()];
            (
                pose,
                vec3f(
                    half_extents.x * 2.0 * EXAMPLE_BRICK_VISUAL_SCALE,
                    half_extents.y * 2.0 * EXAMPLE_BRICK_VISUAL_SCALE,
                    half_extents.z * 2.0 * EXAMPLE_BRICK_VISUAL_SCALE,
                ),
                vec4(color[0], color[1], color[2], 1.0),
                0.0,
            )
        }));
    }

    fn draw_helmet_fallback(&mut self, cx: &mut Cx2d) {
        let center = self.helmet_world_center();
        self.scene.draw_rounded_cube(
            cx,
            Pose::new(Quat::default(), center),
            vec3f(0.06, 0.08, 0.06),
            0.01,
            vec4(1.0, 0.68, 0.12, 1.0),
            0.3,
        );
    }
}

impl Widget for ExampleXrScene {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::XrUpdate(update) = event {
            if update.clicked_menu() || update.menu_pressed() {
                self.scene_center = None;
            }
        }
        self.scene.handle_event(cx, event, scope);
        if let Some(renderer) = self.helmet_renderer.as_mut() {
            renderer.handle_event(cx, event);
        }
        if let Event::XrUpdate(update) = event {
            self.ensure_demo_scene(&update.state);
            if self.scene_center.is_none() {
                self.scene_center = Some(EngineXrScene::default_scene_center(&update.state));
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.scene.draw_walk(cx, scope, walk)
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let _ = self.scene.draw_3d(cx, scope);
        let Some(state) = cx.draw_event.xr_state.as_ref() else {
            return DrawStep::done();
        };

        let cx = &mut Cx2d::new(cx.cx);
        self.ensure_demo_scene(state);
        self.draw_demo_platform(cx);
        self.draw_demo_bodies(cx);
        let camera_env_atlas = self.scene.render_passthrough_env_atlas(cx, state);
        self.ensure_env_loaded(cx);
        self.ensure_helmet_loaded(cx);
        if self.helmet_renderer.is_some() {
            self.prepare_helmet_draw(cx, state, camera_env_atlas);
            let world_center = self.helmet_world_center();
            let helmet_transform = self.helmet_transform();
            let renderer = self.helmet_renderer.as_mut().unwrap();
            match renderer.draw_with_transform(&mut self.draw_pbr, cx, helmet_transform) {
                Ok(()) => {
                    if !self.helmet_draw_logged {
                        log!(
                            "XR helmet draw ok: world_center=({:.3}, {:.3}, {:.3}) fit_scale={:.3}",
                            world_center.x,
                            world_center.y,
                            world_center.z,
                            self.helmet_fit_scale,
                        );
                        self.helmet_draw_logged = true;
                    }
                }
                Err(err) => {
                    if !self.helmet_draw_failed_logged {
                        log!(
                            "XR helmet draw failed: {err}; world_center=({:.3}, {:.3}, {:.3}) fit_scale={:.3}",
                            world_center.x,
                            world_center.y,
                            world_center.z,
                            self.helmet_fit_scale,
                        );
                        self.helmet_draw_failed_logged = true;
                    }
                    self.draw_helmet_fallback(cx);
                }
            }
        } else {
            self.draw_helmet_fallback(cx);
        }
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    phase: AppPhase,
    #[rust]
    scene_access: Option<PermissionStatus>,
    #[rust]
    headset_camera: Option<PermissionStatus>,
    #[rust]
    pending_scene_access_check: Option<i32>,
    #[rust]
    pending_headset_camera_check: Option<i32>,
    #[rust]
    pending_scene_access_request: Option<i32>,
    #[rust]
    pending_headset_camera_request: Option<i32>,
    #[rust]
    ui_refresh_next_frame: Option<NextFrame>,
    #[rust]
    xr_start_next_frame: Option<NextFrame>,
}

impl App {
    fn is_android_preflight() -> bool {
        cfg!(target_os = "android")
    }

    fn scene_access_granted(&self) -> bool {
        !Self::is_android_preflight()
            || matches!(self.scene_access, Some(PermissionStatus::Granted))
    }

    fn headset_camera_granted(&self) -> bool {
        !Self::is_android_preflight()
            || matches!(self.headset_camera, Some(PermissionStatus::Granted))
    }

    fn xr_permissions_ready(&self) -> bool {
        self.scene_access_granted() && self.headset_camera_granted()
    }

    fn permission_checks_pending(&self) -> bool {
        self.pending_scene_access_check.is_some() || self.pending_headset_camera_check.is_some()
    }

    fn permission_requests_pending(&self) -> bool {
        self.pending_scene_access_request.is_some() || self.pending_headset_camera_request.is_some()
    }

    fn phase_variant(&self) -> LiveId {
        match self.phase {
            AppPhase::Preflight => live_id!(Preflight),
            AppPhase::XrRuntime => live_id!(XrRuntime),
        }
    }

    fn apply_phase(&mut self, cx: &mut Cx) {
        let phase_variant = self.phase_variant();
        self.ui
            .adaptive_view(cx, ids!(phase_view))
            .set_variant_selector(move |_cx, _parent_size| phase_variant);
        cx.redraw_all();
    }

    fn schedule_ui_refresh(&mut self, cx: &mut Cx) {
        self.ui_refresh_next_frame = Some(cx.new_next_frame());
        cx.redraw_all();
    }

    fn allow_button_text(&self) -> &'static str {
        if self.permission_checks_pending() {
            "Checking Quest Permissions..."
        } else if self.permission_requests_pending() {
            "Waiting for Quest Permissions..."
        } else if self.xr_permissions_ready() {
            "Re-check Quest Permissions"
        } else {
            "Allow Quest Permissions"
        }
    }

    fn detail_text(&self) -> &'static str {
        if !Self::is_android_preflight() {
            "This build can start XR directly from the splash screen."
        } else if self.xr_permissions_ready() {
            "Quest scene access and headset camera are granted. Start XR when you are ready."
        } else if !self.scene_access_granted() {
            "Allow Quest scene access before starting XR. This unlocks environment depth and passthrough occlusion."
        } else if !self.headset_camera_granted() {
            "Allow Quest headset camera access before starting XR. This unlocks the passthrough texture overlay."
        } else {
            "Allow Quest permissions before starting XR."
        }
    }

    fn status_text(&self) -> &'static str {
        if self.permission_checks_pending() {
            "Checking current Quest permission status."
        } else if self.permission_requests_pending() {
            "Approve the Quest permission dialog to continue."
        } else if !Self::is_android_preflight() {
            "XR is ready to launch from this splash screen."
        } else if self.xr_permissions_ready() {
            "Quest scene access and headset camera granted."
        } else if !self.scene_access_granted() {
            "Quest scene access has not been granted yet."
        } else if !self.headset_camera_granted() {
            "Quest headset camera permission has not been granted yet."
        } else {
            "Quest permissions are incomplete."
        }
    }

    fn refresh_preflight_ui(&mut self, cx: &mut Cx) {
        if self.phase != AppPhase::Preflight {
            return;
        }
        self.ui
            .label(cx, ids!(detail_label))
            .set_text(cx, self.detail_text());
        self.ui
            .label(cx, ids!(status_label))
            .set_text(cx, self.status_text());

        let allow_button = self.ui.button(cx, ids!(allow_button));
        allow_button.set_visible(cx, Self::is_android_preflight());
        allow_button.set_enabled(
            cx,
            Self::is_android_preflight()
                && !self.permission_checks_pending()
                && !self.permission_requests_pending(),
        );
        self.ui
            .widget(cx, ids!(allow_button))
            .set_text(cx, self.allow_button_text());

        self.ui
            .button(cx, ids!(start_xr_button))
            .set_enabled(cx, self.xr_permissions_ready());
    }

    fn begin_scene_access_check(&mut self, cx: &mut Cx) {
        if self.phase != AppPhase::Preflight
            || !Self::is_android_preflight()
            || self.pending_scene_access_check.is_some()
        {
            return;
        }
        self.pending_scene_access_check = Some(cx.check_permission(Permission::SceneAccess));
        self.schedule_ui_refresh(cx);
    }

    fn begin_headset_camera_check(&mut self, cx: &mut Cx) {
        if self.phase != AppPhase::Preflight
            || !Self::is_android_preflight()
            || self.pending_headset_camera_check.is_some()
        {
            return;
        }
        self.pending_headset_camera_check = Some(cx.check_permission(Permission::HeadsetCamera));
        self.schedule_ui_refresh(cx);
    }

    fn request_scene_access(&mut self, cx: &mut Cx) {
        if self.phase != AppPhase::Preflight
            || !Self::is_android_preflight()
            || self.pending_scene_access_check.is_some()
            || self.pending_scene_access_request.is_some()
        {
            return;
        }
        self.pending_scene_access_request = Some(cx.request_permission(Permission::SceneAccess));
        self.schedule_ui_refresh(cx);
    }

    fn request_headset_camera(&mut self, cx: &mut Cx) {
        if self.phase != AppPhase::Preflight
            || !Self::is_android_preflight()
            || self.pending_headset_camera_check.is_some()
            || self.pending_headset_camera_request.is_some()
        {
            return;
        }
        self.pending_headset_camera_request = Some(cx.request_permission(Permission::HeadsetCamera));
        self.schedule_ui_refresh(cx);
    }

    fn begin_preflight_permission_checks(&mut self, cx: &mut Cx) {
        self.begin_scene_access_check(cx);
        self.begin_headset_camera_check(cx);
    }

    fn request_next_missing_permission(&mut self, cx: &mut Cx) {
        if !self.scene_access_granted() {
            self.request_scene_access(cx);
        } else if !self.headset_camera_granted() {
            self.request_headset_camera(cx);
        } else {
            self.begin_preflight_permission_checks(cx);
        }
    }

    fn begin_xr_runtime(&mut self, cx: &mut Cx) {
        if self.phase == AppPhase::XrRuntime {
            return;
        }
        self.phase = AppPhase::XrRuntime;
        self.apply_phase(cx);
        self.xr_start_next_frame = Some(cx.new_next_frame());
    }

    fn maybe_start_xr_on_ready(&mut self, cx: &mut Cx) -> bool {
        if self.phase != AppPhase::Preflight || !self.xr_permissions_ready() {
            return false;
        }
        self.begin_xr_runtime(cx);
        true
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.phase = AppPhase::Preflight;
        if !Self::is_android_preflight() {
            self.scene_access = Some(PermissionStatus::Granted);
            self.headset_camera = Some(PermissionStatus::Granted);
            self.maybe_start_xr_on_ready(cx);
            return;
        }
        self.apply_phase(cx);
        self.schedule_ui_refresh(cx);
        self.begin_preflight_permission_checks(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(allow_button)).clicked(actions) {
            self.request_next_missing_permission(cx);
        }

        if self.ui.button(cx, ids!(start_xr_button)).clicked(actions) && self.xr_permissions_ready()
        {
            self.begin_xr_runtime(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_xr::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        match event {
            Event::NextFrame(ne) => {
                if self
                    .ui_refresh_next_frame
                    .is_some_and(|next_frame| ne.set.contains(&next_frame))
                {
                    self.ui_refresh_next_frame = None;
                    self.refresh_preflight_ui(cx);
                }

                if self
                    .xr_start_next_frame
                    .is_some_and(|next_frame| ne.set.contains(&next_frame))
                {
                    self.xr_start_next_frame = None;
                    cx.xr_start_presenting();
                }
            }
            Event::PermissionResult(result) if result.permission == Permission::SceneAccess => {
                let was_request = self.pending_scene_access_request == Some(result.request_id);
                if self.pending_scene_access_check == Some(result.request_id) {
                    self.pending_scene_access_check = None;
                } else if self.pending_scene_access_request == Some(result.request_id) {
                    self.pending_scene_access_request = None;
                } else {
                    return;
                }
                self.scene_access = Some(result.status);
                if was_request
                    && result.status == PermissionStatus::Granted
                    && !self.headset_camera_granted()
                {
                    self.request_next_missing_permission(cx);
                } else if !self.maybe_start_xr_on_ready(cx) {
                    self.schedule_ui_refresh(cx);
                }
            }
            Event::PermissionResult(result) if result.permission == Permission::HeadsetCamera => {
                let was_request = self.pending_headset_camera_request == Some(result.request_id);
                if self.pending_headset_camera_check == Some(result.request_id) {
                    self.pending_headset_camera_check = None;
                } else if self.pending_headset_camera_request == Some(result.request_id) {
                    self.pending_headset_camera_request = None;
                } else {
                    return;
                }
                self.headset_camera = Some(result.status);
                if was_request
                    && result.status == PermissionStatus::Granted
                    && !self.scene_access_granted()
                {
                    self.request_next_missing_permission(cx);
                } else if !self.maybe_start_xr_on_ready(cx) {
                    self.schedule_ui_refresh(cx);
                }
            }
            Event::Resume => {
                if self.phase == AppPhase::Preflight
                    && Self::is_android_preflight()
                    && !self.permission_requests_pending()
                {
                    self.begin_preflight_permission_checks(cx);
                }
            }
            _ => {}
        }
    }
}
