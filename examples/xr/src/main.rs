pub use makepad_widgets;

use makepad_widgets::*;
use makepad_xr::{
    apply_draw_call_reorder_for_draw_list, SceneScope3D, SceneState3D, XrScene as EngineXrScene,
};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let active_scene = "gltf"
    let xr_panel_pixels = vec2(960, 1200)
    let ActiveSceneContent = Node3D{
        on_render: ||{
            if active_scene == "gltf" {
                ground := Grid3D{
                    size: 16.0
                    position: vec3(0.0, -1.25, 0.0)
                    color: vec4(0.56, 0.58, 0.61, 1.0)
                }
                model := Gltf3D{
                    src: crate_resource("self://resources/DamagedHelmet.glb")
                    env_src: crate_resource("self://resources/royal_esplanade_4k.jpg")
                    scale: vec3(0.82, 0.82, 0.82)
                    rotation: vec3(0.0, 0.45, 0.0)
                }
            }
            else if active_scene == "splat" {
                ground := Grid3D{
                    size: 18.0
                    position: vec3(0.0, -1.25, 0.0)
                    color: vec4(0.56, 0.58, 0.61, 1.0)
                }
                helmet := Gltf3D{
                    src: crate_resource("self://resources/DamagedHelmet.glb")
                    env_src: crate_resource("self://resources/royal_esplanade_4k.jpg")
                    position: vec3(0.0, -0.1, 0.0)
                    rotation: vec3(0.0, 1.2, 0.0)
                    scale: vec3(0.38, 0.38, 0.38)
                }
                sog := ViewSplat{
                    src: crate_resource("self://../../local/toy-cat.sog")
                    position: vec3(-1.8, 0.0, 0.0)
                    scale: vec3(1.0, -1.0, 1.0)
                    normalize_fit: 2.3
                    max_splats: 0
                    radius_scale: 1.1
                    min_radius: 0.0012
                }
                ply := ViewSplat{
                    src: crate_resource("self://../../local/biker.ply")
                    position: vec3(1.8, -0.1, 0.0)
                    scale: vec3(1.0, -1.0, 1.0)
                    normalize_fit: 2.0
                    max_splats: 0
                    radius_scale: 1.1
                    min_radius: 0.0012
                }
            }
            else {
                simulation := PhysicsWorld3D{}
            }
        }
    }

    mod.widgets.ExampleXrSceneBase = #(ExampleXrScene::register_widget(vm))
    mod.widgets.ExampleXrScene = set_type_default() do mod.widgets.ExampleXrSceneBase{
        xr_viewport_size: xr_panel_pixels
        world_scene: ActiveSceneContent{}
    }

    fn select_scene(scene_id, label) {
        active_scene = scene_id
        ui.scene_label.set_text(label)
        ui.desktop_scene.render()
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            on_startup: ||{
                ui.desktop_scene.render()
            }

            xr_scene := mod.widgets.ExampleXrScene{}

            main_window := Window{
                window.inner_size: vec2(1400, 900)
                pass.clear_color: #x0b1118
                xr_pixel_size: xr_panel_pixels
                xr_forward_offset: 0.78
                xr_toggle_with_menu: true
                body +: {
                    main := View{
                        width: Fill
                        height: Fill
                        flow: Overlay

                        desktop_shell := View{
                            width: Fill
                            height: Fill
                            flow: Right

                            sidebar := RoundedView{
                                width: 320
                                height: Fill
                                flow: Down
                                spacing: 14
                                padding: Inset{left: 20 right: 20 top: 22 bottom: 20}
                                draw_bg.color: #x0d1520
                                draw_bg.radius: 0.0

                                title := H1{
                                    text: "XR Demo"
                                    draw_text.color: #xeff7ff
                                }

                                intro := Label{
                                    width: Fill
                                    text: "Desktop mode stays in 2D UI with the scene viewport on the right. Drag to orbit, scroll to zoom, and click the physics stack to kick cubes."
                                    draw_text.color: #xb8c8d8
                                }

                                scene_label := Label{
                                    width: Fill
                                    text: "Single GLTF object"
                                    draw_text.color: #x8fe4d6
                                }

                                show_gltf := Button{
                                    trigger_on_press: true
                                    width: Fill
                                    text: "GLTF"
                                    on_click: || select_scene("gltf", "Single GLTF object")
                                }

                                show_splat := Button{
                                    trigger_on_press: true
                                    width: Fill
                                    text: "Splat Trio"
                                    on_click: || select_scene("splat", "Helmet plus two splat assets")
                                }

                                show_physics := Button{
                                    trigger_on_press: true
                                    width: Fill
                                    text: "Physics Cubes"
                                    on_click: || select_scene("physics", "Interactive physics cube stack")
                                }

                                filler := Filler{}

                                xr_note := Label{
                                    width: Fill
                                    text: "XR presentation stays on the separate XR scene path when headset support is available."
                                    draw_text.color: #x6f8191
                                }
                            }

                            gutter := SolidView{
                                visible: true
                                width: 1
                                height: Fill
                                draw_bg.color: #x1a2633
                            }

                            viewport := View{
                                visible: true
                                width: Fill
                                height: Fill

                                desktop_scene := Scene3D{
                                    width: Fill
                                    height: Fill
                                    animating: true
                                    spin_speed: 0.0
                                    camera_fov_y: 46.0
                                    camera_distance: 5.8
                                    camera_near: 0.02
                                    camera_far: 400.0
                                    camera_target: vec3(0.0, 0.35, 0.0)
                                    draw_bg +: {
                                        color: #x111823
                                        draw_depth: -400.0
                                    }

                                    scene_root := ActiveSceneContent{}
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

const XR_SCENE_FORWARD_OFFSET: f32 = 1.15;
const XR_SCENE_SIDE_OFFSET: f32 = 0.85;
const XR_SCENE_VIEW_OFFSET_Y: f32 = -0.1;

#[derive(Script, ScriptHook, Widget)]
pub struct ExampleXrScene {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    scene: EngineXrScene,
    #[live]
    draw_list_3d: DrawList2d,
    #[live(vec2(960.0, 1200.0))]
    xr_viewport_size: Vec2d,
    #[find]
    #[live]
    world_scene: WidgetRef,
    #[rust]
    scene_pose: Pose,
    #[rust]
    scene_pose_valid: bool,
    #[rust]
    debug_logged_first_draw: bool,
    #[rust]
    debug_logged_empty_draw: bool,
    #[rust]
    debug_logged_missing_world_scene: bool,
    #[rust]
    debug_logged_first_xr_update: bool,
    #[rust]
    debug_logged_draw_without_xr_state: bool,
    #[rust]
    debug_logged_draw_with_xr_state: bool,
    #[rust]
    debug_logged_missing_scene_state: bool,
    #[rust]
    debug_logged_fallback_viewport: bool,
    #[rust]
    debug_xr_update_log_count: u32,
    #[rust]
    debug_xr_draw_log_count: u32,
}

impl ExampleXrScene {
    fn scene_forward(state: &XrState) -> Vec3f {
        let mut forward = state.head_pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        forward.y = 0.0;
        if forward.length() <= 1.0e-4 {
            vec3f(0.0, 0.0, -1.0)
        } else {
            forward.normalize()
        }
    }

    fn compute_scene_pose(state: &XrState) -> Pose {
        let forward = Self::scene_forward(state);
        let right = Vec3f::cross(forward, vec3f(0.0, 1.0, 0.0)).normalize();
        let orientation = Quat::look_rotation(forward, vec3f(0.0, 1.0, 0.0));
        let position =
            vec3f(0.0, state.head_pose.position.y + XR_SCENE_VIEW_OFFSET_Y, 0.0)
                + forward * XR_SCENE_FORWARD_OFFSET
                + right * XR_SCENE_SIDE_OFFSET;
        Pose::new(orientation, position)
    }

    fn reset_scene_anchor(&mut self, state: &XrState) {
        self.scene_pose = Self::compute_scene_pose(state);
        self.scene_pose_valid = true;
    }

    fn ensure_scene_anchor(&mut self, state: &XrState) {
        if self.scene.ensure_scene(state) || !self.scene_pose_valid {
            self.reset_scene_anchor(state);
        }
    }

    fn scene_state(&self, cx: &Cx3d) -> Option<SceneState3D> {
        let draw_list_id = cx.get_current_draw_list_id()?;
        let pass_id = cx.draw_lists[draw_list_id].draw_pass_id?;
        let pass_uniforms = cx.passes[pass_id].pass_uniforms.clone();
        let mut pass_size = cx.current_pass_size();
        if pass_size.x <= 1.0 || pass_size.y <= 1.0 {
            pass_size = self.xr_viewport_size;
        }
        if pass_size.x <= 1.0 || pass_size.y <= 1.0 {
            return None;
        }

        let camera_world = pass_uniforms.camera_inv.transform_vec4(vec4(0.0, 0.0, 0.0, 1.0));
        let camera_pos = if camera_world.w.abs() > 1.0e-6 {
            vec3(
                camera_world.x / camera_world.w,
                camera_world.y / camera_world.w,
                camera_world.z / camera_world.w,
            )
        } else {
            vec3(0.0, 0.0, 0.0)
        };

        Some(SceneState3D {
            time: pass_uniforms.time as f64,
            camera_pos,
            view: pass_uniforms.camera_view,
            projection: pass_uniforms.camera_projection,
            projection_viewport: pass_uniforms.camera_projection,
            clip_ndc: vec4(-1.0, -1.0, 1.0, 1.0),
            depth_range: vec2(0.0, 1.0),
            depth_forward_bias: 0.0,
            use_pass_camera: true,
            viewport_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: pass_size,
            },
        })
    }
}

impl Widget for ExampleXrScene {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(render) || method == live_id!(render_scene) {
            log!(
                "xr scene render request method={:?} world_scene_empty={}",
                method,
                self.world_scene.is_empty()
            );
            let result = self.world_scene.script_call(vm, live_id!(render), args);
            match result {
                ScriptAsyncResult::Pending => {
                    log!("xr scene render request accepted: pending");
                }
                ScriptAsyncResult::MethodNotFound => {
                    log!("xr scene render request failed: method not found");
                }
                ScriptAsyncResult::Return(_) => {
                    log!("xr scene render request returned immediately");
                }
            }
            return result;
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !cx.in_xr_mode() {
            self.scene_pose_valid = false;
        }
        self.scene.handle_event(cx, event, scope);
        self.world_scene.handle_event(cx, event, scope);
        if let Event::XrUpdate(update) = event {
            if !self.debug_logged_first_xr_update {
                self.debug_logged_first_xr_update = true;
                log!(
                    "xr scene first XrUpdate head_pos={:?} head_orientation={:?}",
                    update.state.head_pose.position,
                    update.state.head_pose.orientation
                );
            }
            if self.debug_xr_update_log_count < 12 {
                self.debug_xr_update_log_count += 1;
                log!(
                    "xr scene XrUpdate #{} head_pos={:?} head_orientation={:?} scene_pose_valid={}",
                    self.debug_xr_update_log_count,
                    update.state.head_pose.position,
                    update.state.head_pose.orientation,
                    self.scene_pose_valid
                );
                self.ensure_scene_anchor(&update.state);
                log!(
                    "xr scene anchor after update #{} pose_pos={:?}",
                    self.debug_xr_update_log_count,
                    self.scene_pose.position
                );
            } else {
                self.ensure_scene_anchor(&update.state);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.scene.draw_walk(cx, scope, walk)
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let _ = self.scene.draw_3d(cx, scope);
        let draw_list_id = cx.get_current_draw_list_id();
        let pass_size = cx.current_pass_size();
        let Some(state) = cx.draw_event.xr_state.as_ref() else {
            if !self.debug_logged_draw_without_xr_state {
                self.debug_logged_draw_without_xr_state = true;
                log!("xr scene draw skipped: no xr_state on draw event");
            }
            return DrawStep::done();
        };
        if !self.debug_logged_draw_with_xr_state {
            self.debug_logged_draw_with_xr_state = true;
            log!(
                "xr scene draw entered head_pos={:?} head_orientation={:?}",
                state.head_pose.position,
                state.head_pose.orientation
            );
        }
        let log_this_draw = self.debug_xr_draw_log_count < 20;
        if log_this_draw {
            self.debug_xr_draw_log_count += 1;
            let pass_id = draw_list_id.and_then(|draw_list_id| cx.cx.draw_lists[draw_list_id].draw_pass_id);
            log!(
                "xr scene draw #{} draw_list_id={:?} pass_id={:?} pass_size={:?} world_scene_empty={} scene_pose={:?}",
                self.debug_xr_draw_log_count,
                draw_list_id,
                pass_id,
                pass_size,
                self.world_scene.is_empty(),
                self.scene_pose.position
            );
        }

        self.ensure_scene_anchor(state);
        if (pass_size.x <= 1.0 || pass_size.y <= 1.0) && !self.debug_logged_fallback_viewport {
            self.debug_logged_fallback_viewport = true;
            log!(
                "xr scene draw using fallback viewport size={:?} because xr pass has no 2d pass rect",
                self.xr_viewport_size
            );
        }
        let Some(scene_state) = self.scene_state(cx) else {
            if !self.debug_logged_missing_scene_state {
                self.debug_logged_missing_scene_state = true;
                log!(
                    "xr scene draw skipped: scene_state unavailable draw_list_id={:?} pass_size={:?}",
                    cx.get_current_draw_list_id(),
                    cx.current_pass_size()
                );
            }
            return DrawStep::done();
        };
        let viewport_size = scene_state.viewport_rect.size;
        if self.world_scene.is_empty() {
            if !self.debug_logged_missing_world_scene {
                self.debug_logged_missing_world_scene = true;
                log!("xr scene draw: world_scene missing");
            }
            return DrawStep::done();
        }

        let mut scene_scope_data = SceneScope3D {
            scene: scene_state,
            chart_data: None,
            world_transform: self.scene_pose.to_mat4(),
            draw_call_anchors: Vec::new(),
        };
        let mut scene_scope = Scope::with_data(&mut scene_scope_data);
        {
            let cx2d = &mut Cx2d::new(cx.cx);
            self.draw_list_3d.begin_always(cx2d);
            self.draw_list_3d
                .set_view_transform_self_only(cx2d, &Mat4f::identity());
        }
        {
            let cx3d = &mut Cx3d::new(cx.cx);
            self.world_scene.draw_3d_all(cx3d, &mut scene_scope);
        }
        {
            let cx2d = &mut Cx2d::new(cx.cx);
            let draw_list_id = self.draw_list_3d.draw_list_id();
            let draw_count = cx2d.cx.draw_lists[draw_list_id].draw_items.len();
            if log_this_draw {
                log!(
                    "xr scene world draw_count={} scene_pose={:?} viewport_size={:?}",
                    draw_count,
                    self.scene_pose.position,
                    viewport_size
                );
            }
            if draw_count == 0 {
                if !self.debug_logged_empty_draw {
                    self.debug_logged_empty_draw = true;
                    log!(
                        "xr scene draw emitted 0 draw items pose={:?} pass_size={:?}",
                        self.scene_pose.position,
                        viewport_size
                    );
                }
            } else if !self.debug_logged_first_draw {
                self.debug_logged_first_draw = true;
                log!(
                    "xr scene draw emitted {} draw items pose={:?} pass_size={:?}",
                    draw_count,
                    self.scene_pose.position,
                    viewport_size
                );
            }
            apply_draw_call_reorder_for_draw_list(cx2d, &mut scene_scope, draw_list_id, true);
            self.draw_list_3d.end(cx2d);
        }
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    ui_in_xr: bool,
    #[rust]
    debug_logged_mode_switch: bool,
}

impl App {
    fn sync_mode_ui(&mut self, cx: &mut Cx) -> bool {
        let in_xr = cx.in_xr_mode();
        if self.ui_in_xr == in_xr {
            return false;
        }
        self.debug_logged_mode_switch = true;
        log!("xr app mode switch in_xr={}", in_xr);
        self.ui_in_xr = in_xr;
        self.ui.view(cx, ids!(gutter)).set_visible(cx, !in_xr);
        self.ui.view(cx, ids!(viewport)).set_visible(cx, !in_xr);
        in_xr
    }

    fn render_xr_world_scene(&mut self, cx: &mut Cx) {
        let xr_scene = self.ui.widget(cx, ids!(xr_scene));
        if xr_scene.is_empty() {
            log!("xr scene render request skipped: xr_scene widget missing");
            return;
        }
        log!("xr scene render request dispatched");
        cx.with_vm(|vm| {
            match xr_scene.script_call(vm, live_id!(render_scene), NIL) {
                ScriptAsyncResult::Pending => log!("xr scene widget call pending"),
                ScriptAsyncResult::MethodNotFound => {
                    log!("xr scene widget call failed: method not found")
                }
                ScriptAsyncResult::Return(_) => {
                    log!("xr scene widget call returned immediately")
                }
            }
        });
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_xr::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let entered_xr = self.sync_mode_ui(cx);
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if entered_xr {
            log!("xr scene rerender requested on xr mode entry");
            self.render_xr_world_scene(cx);
        }
        match event {
            Event::Startup => {
                self.render_xr_world_scene(cx);
            }
            Event::Actions(actions)
                if self.ui.button(cx, ids!(show_gltf)).pressed(actions)
                    || self.ui.button(cx, ids!(show_splat)).pressed(actions)
                    || self.ui.button(cx, ids!(show_physics)).pressed(actions) =>
            {
                self.render_xr_world_scene(cx);
            }
            _ => {}
        }
    }
}
