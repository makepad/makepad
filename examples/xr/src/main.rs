pub use makepad_widgets;

use makepad_widgets::*;
use makepad_xr::XrScene as EngineXrScene;
use std::rc::Rc;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let xr_panel_pixels = vec2(960, 1200)

    mod.widgets.ExampleXrSceneBase = #(ExampleXrScene::register_widget(vm))
    mod.widgets.ExampleXrScene = set_type_default() do mod.widgets.ExampleXrSceneBase{
        draw_support_grid +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.18
            spec_power: 64.0
            spec_strength: 0.35
            env_intensity: 0.25
            grid_line_aa: fn(centered: vec2, px: vec2, scale: float) {
                let g = centered * scale;
                let cell = abs(fract(g - vec2(0.5, 0.5)) - vec2(0.5, 0.5));
                let fw = px * scale;
                let aa = min(
                    cell.x / max(fw.x, 0.000001),
                    cell.y / max(fw.y, 0.000001)
                );
                return 1.0 - min(aa, 1.0)
            }
            get_base_color: fn(uv: vec2, vertex_color: vec4) {
                let base = self.u_base_color_factor * vertex_color;
                let centered = uv - vec2(0.5, 0.5);
                let px = vec2(
                    max(length(vec2(dFdx(centered.x), dFdy(centered.x))), 0.000001),
                    max(length(vec2(dFdx(centered.y), dFdy(centered.y))), 0.000001)
                );

                let micro = self.grid_line_aa(centered, px, 96.0);
                let minor = self.grid_line_aa(centered, px, 24.0);
                let major = self.grid_line_aa(centered, px, 6.0);

                let axis_x = 1.0 - min(abs(centered.x) / px.x, 1.0);
                let axis_z = 1.0 - min(abs(centered.y) / px.y, 1.0);

                let floor_color = mix(base.xyz * 0.54, vec3(0.18, 0.185, 0.195), 0.40);
                let micro_color = vec3(0.26, 0.27, 0.285);
                let minor_color = vec3(0.36, 0.37, 0.40);
                let major_color = vec3(0.50, 0.52, 0.56);

                let mut color = floor_color;
                color = mix(color, micro_color, micro * 0.18);
                color = mix(color, minor_color, minor * 0.50);
                color = mix(color, major_color, major * 0.80);
                color = mix(color, vec3(0.47, 0.30, 0.30), axis_x * 0.60);
                color = mix(color, vec3(0.30, 0.37, 0.52), axis_z * 0.60);
                return vec4(color, base.w)
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            xr_scene := mod.widgets.ExampleXrScene{}

            main_window := Window{
                window.inner_size: vec2(1400, 900)
                pass.clear_color: #x0b1118
                xr_pixel_size: xr_panel_pixels
                xr_forward_offset: 0.78
                xr_position_offset: vec3(0.0, -0.26, 0.0)
                xr_toggle_with_menu: false
                body +: {
                    main := View{
                        width: Fill
                        height: Fill
                        flow: Overlay

                        shell := RoundedView{
                            width: 360
                            height: Fit
                            flow: Down
                            spacing: 12
                            padding: Inset{left: 20 right: 20 top: 22 bottom: 20}
                            draw_bg.color: #x0d1520
                            draw_bg.radius: 0.0

                            title := H1{
                                text: "Brick Wall"
                                draw_text.color: #xeff7ff
                            }

                            status := Label{
                                width: Fill
                                text: "Push it around."
                                draw_text.color: #xb8c8d8
                            }

                            controls := View{
                                width: Fit
                                height: Fit
                                flow: Right
                                spacing: 10

                                reset_button := Button{
                                    trigger_on_press: true
                                    width: Fit
                                    text: "Reset"
                                }

                                depth_mesh_button := Button{
                                    trigger_on_press: true
                                    width: Fit
                                    text: "Depth Mesh Off"
                                }
                            }
                        }

                        xr_permissions := mod.widgets.XrPermissionsFlow{}
                    }
                }
            }
        }
    }
}

const XR_UI_FORWARD_OFFSET: f32 = 0.78;
const XR_UI_VERTICAL_OFFSET: f32 = -0.26;
const XR_UI_PANEL_HALF_WIDTH_ESTIMATE: f32 = 0.20;
const XR_WALL_GAP_TO_UI: f32 = 0.10;
const XR_WALL_FORWARD_OFFSET: f32 = 0.04;
const XR_WALL_CHEST_OFFSET_FROM_HEAD: f32 = 0.28;
const XR_WALL_MIN_MID_HEIGHT: f32 = 1.05;
const XR_CUBE_HALF_EXTENT: f32 = 0.015;
const XR_PLATFORM_HALF_HEIGHT: f32 = 0.006;
const XR_PLATFORM_HALF_DEPTH: f32 = 0.06;
const XR_PLATFORM_SPAWN_GAP: f32 = 0.0;
const XR_WALL_BRICK_HALF_WIDTH: f32 = XR_CUBE_HALF_EXTENT * 2.0;
const XR_WALL_BRICK_HALF_HEIGHT: f32 = XR_CUBE_HALF_EXTENT;
const XR_WALL_BRICK_HALF_DEPTH: f32 = XR_CUBE_HALF_EXTENT;
const XR_WALL_FULL_ROW_BRICKS: usize = 12;
const XR_WALL_SHORT_ROW_BRICKS: usize = 11;
const XR_WALL_ROWS: usize = 12;
const XR_WALL_ROTATION_Y: f32 = std::f32::consts::FRAC_PI_2;
const XR_BRICK_VISUAL_SCALE: f32 = 0.98;
const XR_OCCLUSION_DEPTH_CLIP: f32 = 1.0;
const XR_SUPPORT_GRID_LIFT: f32 = 0.0015;
const XR_REFRACTION_CUBE_HALF_EXTENT: f32 = 0.06;
const XR_REFRACTION_CUBE_UI_SIDE_OFFSET: f32 = -0.36;
const XR_REFRACTION_CUBE_UI_FORWARD_OFFSET: f32 = 0.05;
const XR_REFRACTION_CUBE_UI_VERTICAL_OFFSET: f32 = -0.12;
const XR_REFRACTION_CUBE_CORNER_RADIUS: f32 = 0.010;
const XR_REFRACTION_CUBE_ROUGHNESS: f32 = 0.10;
const XR_REFRACTION_CUBE_SPEC_STRENGTH: f32 = 0.92;
const XR_REFRACTION_CUBE_ENV_INTENSITY: f32 = 1.18;
const XR_REFRACTION_CUBE_FOCUS_DISTANCE: f32 = 1.45;
const CUBE_COLORS: &[[f32; 3]] = &[
    [0.90, 0.30, 0.25],
    [0.25, 0.75, 0.45],
    [0.30, 0.50, 0.90],
    [0.95, 0.75, 0.20],
    [0.80, 0.40, 0.85],
    [0.20, 0.80, 0.80],
    [0.95, 0.55, 0.25],
    [0.60, 0.85, 0.35],
];

#[derive(Script, ScriptHook, Widget)]
pub struct ExampleXrScene {
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_support_grid: DrawPbr,
    #[deref]
    scene: EngineXrScene,
    #[rust]
    wall_center: Option<Vec3f>,
    #[rust]
    wall_reset_center: Option<Vec3f>,
    #[rust]
    ui_reset_center: Option<Vec3f>,
    #[rust]
    reset_forward: Vec3f,
    #[rust]
    reset_right: Vec3f,
    #[rust]
    last_xr_state: Option<Rc<XrState>>,
}

impl ExampleXrScene {
    fn scene_basis(state: &XrState) -> (Vec3f, Vec3f) {
        let mut forward = state.head_pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        forward.y = 0.0;
        if forward.length() <= 1.0e-4 {
            forward = vec3f(0.0, 0.0, -1.0);
        } else {
            forward = forward.normalize();
        }
        let right = vec3f(-forward.z, 0.0, forward.x);
        (forward, right)
    }

    fn ui_panel_center(state: &XrState, forward: Vec3f) -> Vec3f {
        vec3f(0.0, state.head_pose.position.y + XR_UI_VERTICAL_OFFSET, 0.0)
            + forward * XR_UI_FORWARD_OFFSET
    }

    fn wall_brick_width() -> f32 {
        XR_WALL_BRICK_HALF_WIDTH * 2.0 + XR_PLATFORM_SPAWN_GAP
    }

    fn wall_brick_height() -> f32 {
        XR_WALL_BRICK_HALF_HEIGHT * 2.0 + XR_PLATFORM_SPAWN_GAP
    }

    fn wall_half_width() -> f32 {
        Self::wall_brick_width() * XR_WALL_FULL_ROW_BRICKS as f32 * 0.5
    }

    fn wall_total_height() -> f32 {
        Self::wall_brick_height() * XR_WALL_ROWS as f32
    }

    fn wall_base_height(state: &XrState) -> f32 {
        let desired_mid_height =
            (state.head_pose.position.y - XR_WALL_CHEST_OFFSET_FROM_HEAD).max(XR_WALL_MIN_MID_HEIGHT);
        desired_mid_height - Self::wall_total_height() * 0.5
    }

    fn wall_center_from_state(state: &XrState, forward: Vec3f, right: Vec3f) -> Vec3f {
        let panel_center = Self::ui_panel_center(state, forward);
        vec3f(panel_center.x, Self::wall_base_height(state), panel_center.z)
            + right * (XR_UI_PANEL_HALF_WIDTH_ESTIMATE + Self::wall_half_width() + XR_WALL_GAP_TO_UI)
            + forward * XR_WALL_FORWARD_OFFSET
    }

    fn set_reset_anchor_from_state(&mut self, state: &XrState) {
        let (forward, right) = Self::scene_basis(state);
        self.reset_forward = forward;
        self.reset_right = right;
        self.ui_reset_center = Some(Self::ui_panel_center(state, forward));
        self.wall_reset_center = Some(Self::wall_center_from_state(state, forward, right));
    }

    fn should_reanchor_reset_center(update: &XrUpdateEvent) -> bool {
        let position_delta = update.state.head_pose.position - update.last.head_pose.position;
        if position_delta.length() > 0.35 {
            return true;
        }

        let (current_forward, _) = Self::scene_basis(&update.state);
        let (last_forward, _) = Self::scene_basis(&update.last);
        current_forward.dot(last_forward).clamp(-1.0, 1.0) < 0.75
    }

    fn reset_wall(&mut self, cx: &mut Cx) {
        let Some(state) = self.last_xr_state.clone() else {
            return;
        };
        self.wall_center = None;
        self.scene.reset_now(cx, state.as_ref());
        self.ensure_brick_wall(state.as_ref());
        cx.redraw_all();
    }

    fn ensure_brick_wall(&mut self, state: &XrState) {
        if self.scene.ensure_scene(state) || self.wall_reset_center.is_none() {
            self.set_reset_anchor_from_state(state);
        }
        if self.scene.dynamic_box_count() != 0 {
            return;
        }

        let center = if let Some(center) = self.wall_center {
            center
        } else if let Some(center) = self.wall_reset_center {
            self.wall_center = Some(center);
            center
        } else {
            self.set_reset_anchor_from_state(state);
            let center = self
                .wall_reset_center
                .unwrap_or_else(|| Self::wall_center_from_state(state, self.reset_forward, self.reset_right));
            self.wall_center = Some(center);
            center
        };

        let scene_rotation =
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), XR_WALL_ROTATION_Y);
        let platform_half_width = Self::wall_half_width() + 0.06;
        let support_pose = Pose::new(
            scene_rotation,
            center + vec3f(0.0, -XR_PLATFORM_HALF_HEIGHT, 0.0),
        );
        self.scene.spawn_fixed_box(
            support_pose,
            vec3f(
                platform_half_width,
                XR_PLATFORM_HALF_HEIGHT,
                XR_PLATFORM_HALF_DEPTH,
            ),
            0.9,
        );

        let brick_half_extents = vec3f(
            XR_WALL_BRICK_HALF_WIDTH,
            XR_WALL_BRICK_HALF_HEIGHT,
            XR_WALL_BRICK_HALF_DEPTH,
        );
        let brick_width = Self::wall_brick_width();
        let brick_height = Self::wall_brick_height();
        let row_axis = vec3f(0.0, 0.0, 1.0);
        for row in 0..XR_WALL_ROWS {
            let bricks_in_row = if row % 2 == 0 {
                XR_WALL_FULL_ROW_BRICKS
            } else {
                XR_WALL_SHORT_ROW_BRICKS
            };
            let row_center_offset = (bricks_in_row as f32 - 1.0) * 0.5;
            for brick in 0..bricks_in_row {
                let brick_center = center
                    + row_axis * ((brick as f32 - row_center_offset) * brick_width)
                    + vec3f(
                        0.0,
                        XR_WALL_BRICK_HALF_HEIGHT
                            + XR_PLATFORM_SPAWN_GAP
                            + row as f32 * brick_height,
                        0.0,
                    );
                self.scene
                    .spawn_dynamic_box(Pose::new(scene_rotation, brick_center), brick_half_extents);
            }
        }
    }

    fn draw_brick_wall(&mut self, cx: &mut Cx2d) {
        let mut light_dir =
            vec3f(0.0, 0.84, 0.0) - self.reset_right * 0.62 + self.reset_forward * 0.16;
        if light_dir.length() <= 1.0e-4 {
            light_dir = vec3f(-0.45, 0.84, 0.18);
        } else {
            light_dir = light_dir.normalize();
        }
        self.scene.set_cube_light_dir(light_dir);
        let bodies = self.scene.dynamic_box_states();
        self.scene.draw_pose_boxes(cx, bodies.into_iter().enumerate().map(|(index, (pose, half_extents))| {
            let color = CUBE_COLORS[index % CUBE_COLORS.len()];
            (
                pose,
                vec3f(
                    half_extents.x * 2.0 * XR_BRICK_VISUAL_SCALE,
                    half_extents.y * 2.0 * XR_BRICK_VISUAL_SCALE,
                    half_extents.z * 2.0 * XR_BRICK_VISUAL_SCALE,
                ),
                vec4(color[0], color[1], color[2], 1.0),
                XR_OCCLUSION_DEPTH_CLIP,
            )
        }));
    }

    fn draw_support_grid(&mut self, cx: &mut Cx2d) {
        let Some(center) = self.wall_center else {
            return;
        };
        let env_tex = self.draw_support_grid.default_env_texture(cx);
        self.draw_support_grid.begin();
        self.draw_support_grid.set_use_pass_camera(true);
        self.draw_support_grid.set_depth_clip(XR_OCCLUSION_DEPTH_CLIP);
        self.draw_support_grid.set_base_color_texture(None);
        self.draw_support_grid.set_metal_roughness_texture(None);
        self.draw_support_grid.set_normal_texture(None);
        self.draw_support_grid.set_occlusion_texture(None);
        self.draw_support_grid.set_emissive_texture(None);
        self.draw_support_grid.set_env_texture(Some(env_tex));
        self.draw_support_grid.set_metal_roughness(0.02, 0.96);
        self.draw_support_grid.fill(vec4(0.58, 0.60, 0.63, 1.0));
        self.draw_support_grid.push_matrix();
        self.draw_support_grid
            .translate_v(center + vec3f(0.0, XR_SUPPORT_GRID_LIFT, 0.0));
        let _ = self.draw_support_grid.draw_surface(
            cx,
            vec2f(
                XR_PLATFORM_HALF_DEPTH * 2.0,
                (Self::wall_half_width() + 0.06) * 2.0,
            ),
            1,
            1,
        );
        self.draw_support_grid.pop_matrix();
    }

    fn draw_refractive_cube(
        &mut self,
        cx: &mut Cx2d,
        state: &XrState,
        camera_env_atlas: Option<Texture>,
    ) {
        let Some(ui_center) = self.ui_reset_center else {
            return;
        };
        let pose = Pose::new(
            Quat::look_rotation(self.reset_forward, vec3f(0.0, 1.0, 0.0)),
            ui_center
                + vec3f(
                    0.0,
                    XR_REFRACTION_CUBE_UI_VERTICAL_OFFSET,
                    0.0,
                ),
        );
        let pose = Pose::new(
            pose.orientation,
            pose.position
                + self.reset_right * XR_REFRACTION_CUBE_UI_SIDE_OFFSET
                + self.reset_forward * XR_REFRACTION_CUBE_UI_FORWARD_OFFSET,
        );
        self.scene.draw_refractive_rounded_cube(
            cx,
            state,
            camera_env_atlas,
            pose,
            vec3f(
                XR_REFRACTION_CUBE_HALF_EXTENT,
                XR_REFRACTION_CUBE_HALF_EXTENT,
                XR_REFRACTION_CUBE_HALF_EXTENT,
            ),
            XR_REFRACTION_CUBE_CORNER_RADIUS,
            vec4(0.90, 0.97, 1.0, 1.0),
            XR_REFRACTION_CUBE_ROUGHNESS,
            XR_REFRACTION_CUBE_SPEC_STRENGTH,
            XR_REFRACTION_CUBE_ENV_INTENSITY,
            XR_REFRACTION_CUBE_FOCUS_DISTANCE,
        );
    }
}

impl Widget for ExampleXrScene {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        _args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(reset_wall) {
            vm.with_cx_mut(|cx| self.reset_wall(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(toggle_depth_mesh) {
            let mut visible = self.scene.depth_mesh_visible();
            vm.with_cx_mut(|cx| {
                visible = self.scene.toggle_depth_mesh_visible(cx);
                cx.redraw_all();
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(visible));
        }
        if method == live_id!(depth_mesh_visible) {
            return ScriptAsyncResult::Return(ScriptValue::from_bool(self.scene.depth_mesh_visible()));
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::XrUpdate(update) = event {
            self.last_xr_state = Some(update.state.clone());
            if self.wall_reset_center.is_none() || Self::should_reanchor_reset_center(update) {
                self.set_reset_anchor_from_state(&update.state);
            }
            if EngineXrScene::reset_requested(update) {
                self.wall_center = None;
            }
        }
        self.scene.handle_event(cx, event, scope);
        if let Event::XrUpdate(update) = event {
            self.ensure_brick_wall(&update.state);
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
        self.ensure_brick_wall(state);
        let cx = &mut Cx2d::new(cx.cx);
        let camera_env_atlas = self.scene.render_passthrough_env_atlas(cx, state);
        self.draw_support_grid(cx);
        self.draw_refractive_cube(cx, state, camera_env_atlas);
        self.draw_brick_wall(cx);
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl App {
    fn set_depth_mesh_button_label(&mut self, cx: &mut Cx, visible: bool) {
        let label = if visible {
            "Depth Mesh On"
        } else {
            "Depth Mesh Off"
        };
        self.ui.widget(cx, ids!(depth_mesh_button)).set_text(cx, label);
    }

    fn reset_wall(&mut self, cx: &mut Cx) {
        let xr_scene = self.ui.widget(cx, ids!(xr_scene));
        if xr_scene.is_empty() {
            return;
        }
        cx.with_vm(|vm| {
            let _ = xr_scene.script_call(vm, live_id!(reset_wall), NIL);
        });
    }

    fn depth_mesh_visible(&mut self, cx: &mut Cx) -> bool {
        let xr_scene = self.ui.widget(cx, ids!(xr_scene));
        if xr_scene.is_empty() {
            return false;
        }
        let mut visible = false;
        cx.with_vm(|vm| {
            if let ScriptAsyncResult::Return(value) =
                xr_scene.script_call(vm, live_id!(depth_mesh_visible), NIL)
            {
                visible = value.as_bool().unwrap_or(false);
            }
        });
        visible
    }

    fn toggle_depth_mesh(&mut self, cx: &mut Cx) -> bool {
        let xr_scene = self.ui.widget(cx, ids!(xr_scene));
        if xr_scene.is_empty() {
            return false;
        }
        let mut visible = false;
        cx.with_vm(|vm| {
            if let ScriptAsyncResult::Return(value) =
                xr_scene.script_call(vm, live_id!(toggle_depth_mesh), NIL)
            {
                visible = value.as_bool().unwrap_or(false);
            }
        });
        visible
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(reset_button)).pressed(actions) {
            self.reset_wall(cx);
        }
        if self.ui.button(cx, ids!(depth_mesh_button)).pressed(actions) {
            let visible = self.toggle_depth_mesh(cx);
            self.set_depth_mesh_button_label(cx, visible);
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
        if matches!(event, Event::Startup) {
            let visible = self.depth_mesh_visible(cx);
            self.set_depth_mesh_button_label(cx, visible);
        }
    }
}
