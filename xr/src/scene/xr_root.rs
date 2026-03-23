use crate::*;
use crate::xr_env::XrEnv;
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.XrRootBase = #(XrRoot::register_widget(vm))
    mod.widgets.XrRoot = set_type_default() do mod.widgets.XrRootBase{
        width: Fill
        height: Fill
        flow: Overlay
        xr_panel_pixels: vec2(960.0, 1200.0)
        desktop_control_width: 360.0
        desktop_padding: 0.0
        desktop_spacing: 0.0

        window +: {
            inner_size: vec2(1400 900)
        }

        pass +: {
            clear_color: #x0b1118
        }

        draw_control_bg.color: #x0d1520
        draw_scene_bg.color: #x10161f
        env: mod.widgets.XrEnv{}
        xr_permissions := mod.widgets.XrPermissionsFlow{}
    }
}

#[derive(Clone, Copy, Default)]
pub struct XrRootOptions {
    pub depth_mesh: bool,
    pub env_cube: bool,
}

pub fn xr_root_options_from_scope(scope: &mut Scope) -> XrRootOptions {
    scope
        .props
        .get::<XrRootOptions>()
        .copied()
        .unwrap_or_default()
}

#[derive(Clone)]
enum DrawState {
    Drawing,
}

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct XrRoot {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[rust]
    area: Area,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    control_2d: LiveId,
    #[live]
    control_xr: LiveId,
    #[live]
    scene: LiveId,
    #[live(false)]
    depth_mesh: bool,
    #[live(false)]
    env_cube: bool,
    #[live]
    env: XrEnv,
    #[live]
    window: ScriptWindowHandle,
    #[live]
    pass: ScriptDrawPass,
    #[new]
    depth_texture: Texture,
    #[new]
    main_draw_list: DrawList2d,
    #[live]
    draw_control_bg: DrawColor,
    #[live]
    draw_scene_bg: DrawColor,
    #[rust]
    template_widgets: HashMap<LiveId, WidgetRef>,
    #[rust]
    scene_order: Vec<LiveId>,
    #[rust]
    active_scene: LiveId,
    #[rust]
    permissions_widget: WidgetRef,
    #[new]
    xr_draw_list: DrawList,
    #[new]
    xr_control_draw_list: DrawList2d,
    #[rust(Mat4f::nonuniform_scaled_translation(vec3(0.0004,-0.0004,0.12),vec3(-0.25,0.25,-0.5)))]
    xr_view_matrix: Mat4f,
    #[rust(Mat4f::nonuniform_scaled_translation(vec3(0.0004,-0.0004,0.0004),vec3(-0.25,0.25,-0.5)))]
    xr_hit_matrix: Mat4f,
    #[rust]
    xr_view_matrix_initialized: bool,
    #[rust]
    xr_runtime_active: bool,
    #[rust]
    xr_draw_logged: bool,
    #[rust]
    xr_panel_log_count: u32,
    #[rust(0u32)]
    desktop_draw_log_count: u32,
    #[live(vec2(960.0, 1200.0))]
    xr_panel_pixels: Vec2d,
    #[live(3.0)]
    xr_dpi_factor: f64,
    #[live(0.0004)]
    xr_pixel_scale: f32,
    #[live(300.0)]
    xr_depth_scale: f32,
    #[live(0.78)]
    xr_forward_offset: f32,
    #[live(vec3(0.0, -0.26, 0.0))]
    xr_position_offset: Vec3f,
    #[live(false)]
    xr_toggle_with_menu: bool,
    #[rust(true)]
    xr_visible: bool,
    #[live(360.0)]
    desktop_control_width: f64,
    #[live(0.0)]
    desktop_padding: f64,
    #[live(20.0)]
    desktop_spacing: f64,
    #[rust]
    initialized: bool,
    #[rust]
    draw_state: DrawStateWrap<DrawState>,
}

impl XrRoot {
    fn xr_is_active(&self, cx: &Cx) -> bool {
        cx.in_xr_mode() && self.xr_runtime_active
    }

    fn options(&self) -> XrRootOptions {
        XrRootOptions {
            depth_mesh: self.depth_mesh,
            env_cube: self.env_cube,
        }
    }

    pub fn depth_mesh_visible(&self) -> bool {
        self.depth_mesh
    }

    pub fn set_depth_mesh_visible(&mut self, cx: &mut Cx, visible: bool) {
        self.depth_mesh = visible;
        cx.redraw_all();
    }

    pub fn toggle_depth_mesh_visible(&mut self, cx: &mut Cx) -> bool {
        let visible = !self.depth_mesh_visible();
        self.set_depth_mesh_visible(cx, visible);
        visible
    }

    fn xr_flat_forward(orientation: Quat) -> Vec3f {
        let mut forward = orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        forward.y = 0.0;
        if forward.length() <= 1.0e-4 {
            vec3f(0.0, 0.0, -1.0)
        } else {
            forward.normalize()
        }
    }

    fn xr_should_reanchor_panel(update: &XrUpdateEvent) -> bool {
        let position_delta = update.state.head_pose.position - update.last.head_pose.position;
        if position_delta.length() > 0.35 {
            return true;
        }

        let current_forward = Self::xr_flat_forward(update.state.head_pose.orientation);
        let last_forward = Self::xr_flat_forward(update.last.head_pose.orientation);
        current_forward.dot(last_forward).clamp(-1.0, 1.0) < 0.75
    }

    fn xr_window_logical_size(&self) -> DVec2 {
        let dpi_factor = self.xr_dpi_factor.max(1.0);
        dvec2(
            self.xr_panel_pixels.x.max(1.0) / dpi_factor,
            self.xr_panel_pixels.y.max(1.0) / dpi_factor,
        )
    }

    fn xr_window_logical_scale(&self) -> f32 {
        self.xr_pixel_scale.max(0.00001) * self.xr_dpi_factor.max(1.0) as f32
    }

    fn compute_xr_panel_matrix(&self, state: &XrState, depth_scale: f32) -> Mat4f {
        let mut forward = state.head_pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        forward.y = 0.0;
        if forward.length() <= 1.0e-4 {
            forward = vec3f(0.0, 0.0, -1.0);
        } else {
            forward = forward.normalize();
        }

        let up = vec3f(0.0, 1.0, 0.0);
        let yaw_rotation = Quat::look_rotation(forward, up);
        let center = vec3f(0.0, state.head_pose.position.y, 0.0)
            + forward * self.xr_forward_offset.max(0.0)
            + yaw_rotation.rotate_vec3(&self.xr_position_offset);
        let pose = Pose::new(Quat::look_rotation(forward.scale(-1.0), up), center);
        let size = self.xr_window_logical_size();
        let scale = self.xr_window_logical_scale();
        let local_depth_transform = Mat4f::nonuniform_scaled_translation(
            vec3(1.0, 1.0, depth_scale.max(0.00001)),
            vec3(0.0, 0.0, 0.0),
        );
        let local_panel = Mat4f::nonuniform_scaled_translation(
            vec3(scale, -scale, scale),
            vec3(
                -(size.x as f32) * scale * 0.5,
                (size.y as f32) * scale * 0.5,
                0.0,
            ),
        );
        let object_to_world = Mat4f::mul(&local_panel, &local_depth_transform);
        Mat4f::mul(&pose.to_mat4(), &object_to_world)
    }

    fn log_xr_panel_pose(&mut self, state: &XrState, panel_matrix: &Mat4f, hit_matrix: &Mat4f) {
        if self.xr_panel_log_count >= 8 {
            return;
        }
        let mut forward = state.head_pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
        forward.y = 0.0;
        forward = if forward.length() <= 1.0e-4 {
            vec3f(0.0, 0.0, -1.0)
        } else {
            forward.normalize()
        };
        let up = vec3f(0.0, 1.0, 0.0);
        let yaw_rotation = Quat::look_rotation(forward, up);
        let center = vec3f(0.0, state.head_pose.position.y, 0.0)
            + forward * self.xr_forward_offset.max(0.0)
            + yaw_rotation.rotate_vec3(&self.xr_position_offset);
        crate::log!(
            "XrRoot panel[{}] head=({:.3},{:.3},{:.3}) forward=({:.3},{:.3},{:.3}) center=({:.3},{:.3},{:.3}) panel_t=({:.3},{:.3},{:.3}) hit_t=({:.3},{:.3},{:.3}) logical_size=({:.1},{:.1}) scale={:.6} depth_scale={:.3}",
            self.xr_panel_log_count,
            state.head_pose.position.x,
            state.head_pose.position.y,
            state.head_pose.position.z,
            forward.x,
            forward.y,
            forward.z,
            center.x,
            center.y,
            center.z,
            panel_matrix.v[12],
            panel_matrix.v[13],
            panel_matrix.v[14],
            hit_matrix.v[12],
            hit_matrix.v[13],
            hit_matrix.v[14],
            self.xr_window_logical_size().x,
            self.xr_window_logical_size().y,
            self.xr_window_logical_scale(),
            self.xr_depth_scale
        );
        self.xr_panel_log_count += 1;
    }

    fn current_control_is_xr(&self) -> bool {
        self.xr_runtime_active && self.control_xr != LiveId(0)
    }

    fn active_control_id(&self) -> LiveId {
        if self.current_control_is_xr() && self.template_widget(self.control_xr).is_some() {
            self.control_xr
        } else {
            self.control_2d
        }
    }

    fn active_control_widget(&self) -> WidgetRef {
        self.template_widget(self.active_control_id())
            .unwrap_or_else(WidgetRef::empty)
    }

    fn template_widget(&self, id: LiveId) -> Option<WidgetRef> {
        self.template_widgets
            .get(&id)
            .cloned()
            .filter(|widget| !widget.is_empty())
    }

    fn active_scene_id(&self) -> LiveId {
        if self.active_scene != LiveId(0) {
            self.active_scene
        } else {
            self.scene
        }
    }

    fn active_scene_widget(&self) -> WidgetRef {
        self.template_widget(self.active_scene_id())
            .unwrap_or_else(WidgetRef::empty)
    }

    fn activate_scene_id(&mut self, cx: &mut Cx, scene_id: LiveId) -> bool {
        if scene_id == LiveId(0) || !self.scene_order.contains(&scene_id) {
            return false;
        }
        if self.active_scene == scene_id {
            return true;
        }
        self.active_scene = scene_id;
        cx.widget_tree_mark_dirty(self.uid);
        cx.redraw_all();
        true
    }

    fn switch_scene_internal(&mut self, cx: &mut Cx) -> LiveId {
        if self.scene_order.is_empty() {
            return LiveId(0);
        }
        let current = self.active_scene_id();
        let current_index = self
            .scene_order
            .iter()
            .position(|scene_id| *scene_id == current)
            .unwrap_or(0);
        let next_scene = self.scene_order[(current_index + 1) % self.scene_order.len()];
        let _ = self.activate_scene_id(cx, next_scene);
        next_scene
    }

    fn log_desktop_state(&mut self, label: &str) {
        if self.desktop_draw_log_count >= 8 {
            return;
        }
        let control_widget = self.active_control_widget();
        let scene_widget = self.active_scene_widget();
        crate::log!(
            "XrRoot {label}[{}]: xr_runtime_active={} active_scene={:?} template_widgets={} scene_order={} control_widget_empty={} scene_widget_empty={} permissions_empty={}",
            self.desktop_draw_log_count,
            self.xr_runtime_active,
            self.active_scene_id(),
            self.template_widgets.len(),
            self.scene_order.len(),
            control_widget.is_empty(),
            scene_widget.is_empty(),
            self.permissions_widget.is_empty()
        );
        self.desktop_draw_log_count += 1;
    }

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.window.handle.set_pass(cx, &self.pass.handle);
        self.pass.handle.set_pass_name(cx, "xr_root_window");
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.pass.handle.set_depth_texture(
            cx,
            &self.depth_texture,
            DrawPassClearDepth::ClearWith(1.0),
        );
    }

    fn begin_preview(&mut self, cx: &mut Cx2d) -> Redrawing {
        self.ensure_initialized(cx.cx);
        let will_redraw = cx.will_redraw(&mut self.main_draw_list, Walk::default());
        if !will_redraw {
            return Redrawing::no();
        }
        cx.begin_pass(&self.pass.handle, None);
        self.main_draw_list.begin_always(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());
        Redrawing::yes()
    }

    fn end_preview(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        self.main_draw_list.end(cx);
        cx.end_pass(&self.pass.handle);
    }

    fn draw_xr_controls(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        if !self.xr_visible {
            return;
        }
        let control_widget = self.active_control_widget();
        if control_widget.is_empty() {
            return;
        }

        let previous_dpi = cx.current_dpi_factor();
        cx.set_current_pass_dpi_factor(self.xr_dpi_factor.max(1.0));
        self.xr_control_draw_list.begin_always(cx);
        self.xr_control_draw_list
            .set_view_transform(cx, &self.xr_view_matrix);
        let size = self.xr_window_logical_size();
        cx.begin_root_turtle(size, Layout::flow_down());
        control_widget.draw_walk_all(cx, scope, Walk::default());
        cx.end_pass_sized_turtle();
        self.xr_control_draw_list.end(cx);
        cx.set_current_pass_dpi_factor(previous_dpi);
    }

    fn draw_xr_content(&mut self, cx: &mut Cx3d, scope: &mut Scope) {
        if !self.xr_draw_logged {
            self.xr_draw_logged = true;
            crate::log!(
                "XrRoot XR draw active scene={:?} control_2d={:?} control_xr={:?}",
                self.active_scene_id(),
                self.control_2d,
                self.control_xr
            );
        }

        self.xr_draw_list.begin_always(cx);
        let scene_widget = self.active_scene_widget();
        if !scene_widget.is_empty() {
            let options = self.options();
            let mut draw_scope = scene_widget
                .borrow_mut::<XrScene>()
                .and_then(|mut scene| {
                    let cx2d = &mut Cx2d::new(cx.cx);
                    self.env.prepare_draw_scope(cx2d, &mut scene, options)
                });
            if let Some(draw_scope) = draw_scope.as_mut() {
                let mut scene_scope = Scope::with_data_props(draw_scope, &options);
                scene_widget.draw_3d_all(cx, &mut scene_scope);
            } else {
                let mut scene_scope = Scope::with_props(&options);
                scene_widget.draw_3d_all(cx, &mut scene_scope);
            }
        }
        self.xr_draw_list.end(cx);

        let cx2d = &mut Cx2d::new(cx.cx);
        self.draw_xr_controls(cx2d, scope);
    }
}

impl ScriptHook for XrRoot {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.template_widgets.clear();
            self.scene_order.clear();
            self.active_scene = LiveId(0);
            self.permissions_widget = WidgetRef::empty();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        let mut template_ids = Vec::new();
        let mut scene_order = Vec::new();
        self.permissions_widget = WidgetRef::empty();

        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    let Some(id) = kv.key.as_id() else {
                        continue;
                    };
                    if !WidgetRef::value_is_newable_widget(vm, kv.value) {
                        continue;
                    }
                    let widget = if let Some(widget) = self.template_widgets.get_mut(&id) {
                        widget.script_apply(vm, apply, scope, kv.value);
                        widget.clone()
                    } else {
                        let widget = WidgetRef::script_from_value_scoped(vm, scope, kv.value);
                        self.template_widgets.insert(id, widget.clone());
                        widget
                    };

                    if id == live_id!(xr_permissions)
                        || widget.borrow::<XrPermissionsFlow>().is_some()
                    {
                        self.permissions_widget = widget;
                        continue;
                    }

                    let is_scene = widget.borrow::<XrScene>().is_some();
                    let is_control = id == self.control_2d || id == self.control_xr;
                    if !is_control && !is_scene {
                        continue;
                    }
                    if is_scene {
                        scene_order.push(id);
                    }
                    template_ids.push(id);
                }
            });
        }

        self.template_widgets
            .retain(|id, _| template_ids.contains(id));
        self.scene_order = scene_order;
        if !self.scene_order.contains(&self.active_scene) {
            self.active_scene = if self.scene_order.contains(&self.scene) {
                self.scene
            } else {
                self.scene_order.first().copied().unwrap_or(LiveId(0))
            };
        }

        vm.cx_mut().widget_tree_mark_dirty(self.uid);
        crate::log!(
            "XrRoot on_after_apply template_widgets={} scene_order={:?} active_scene={:?} permissions_empty={}",
            self.template_widgets.len(),
            self.scene_order,
            self.active_scene_id(),
            self.permissions_widget.is_empty()
        );
        let scene_widget = self.active_scene_widget();
        if !scene_widget.is_empty() {
            let _ = scene_widget.script_call(vm, live_id!(render), NIL);
        }
    }
}

impl WidgetNode for XrRoot {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        if !self.permissions_widget.is_empty() {
            visit(live_id!(xr_permissions), self.permissions_widget.clone());
        }
        let control_id = self.active_control_id();
        if let Some(control) = self.template_widget(control_id) {
            visit(control_id, control);
        }
        let scene_id = self.active_scene_id();
        if let Some(scene) = self.template_widget(scene_id) {
            visit(scene_id, scene);
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        cx.redraw_all();
    }
}

impl Widget for XrRoot {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(toggle_depth_mesh) {
            let mut visible = self.depth_mesh_visible();
            vm.with_cx_mut(|cx| {
                visible = self.toggle_depth_mesh_visible(cx);
            });
            return ScriptAsyncResult::Return(ScriptValue::from_bool(visible));
        }
        if method == live_id!(depth_mesh_visible) {
            return ScriptAsyncResult::Return(ScriptValue::from_bool(self.depth_mesh_visible()));
        }
        if method == live_id!(render_scene) {
            let scene_widget = self.active_scene_widget();
            if scene_widget.is_empty() {
                return ScriptAsyncResult::Return(NIL);
            }
            return scene_widget.script_call(vm, live_id!(render), args);
        }
        if method == live_id!(switch_scene) {
            let mut next_scene = LiveId(0);
            vm.with_cx_mut(|cx| {
                next_scene = self.switch_scene_internal(cx);
            });
            if next_scene == LiveId(0) {
                return ScriptAsyncResult::MethodNotFound;
            }
            let scene_widget = self.active_scene_widget();
            let _ = scene_widget.script_call(vm, live_id!(render), NIL);
            return ScriptAsyncResult::Return(ScriptValue::from_id(next_scene));
        }
        if method == live_id!(select_scene) {
            let Some(scene_id) = args.as_id() else {
                return ScriptAsyncResult::MethodNotFound;
            };
            let mut activated = false;
            vm.with_cx_mut(|cx| {
                activated = self.activate_scene_id(cx, scene_id);
            });
            if !activated {
                return ScriptAsyncResult::MethodNotFound;
            }
            let scene_widget = self.active_scene_widget();
            let _ = scene_widget.script_call(vm, live_id!(render), NIL);
            return ScriptAsyncResult::Return(ScriptValue::from_id(scene_id));
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_initialized(cx);

        if !cx.in_xr_mode() {
            if self.xr_runtime_active {
                self.xr_runtime_active = false;
                cx.widget_tree_mark_dirty(self.uid);
            }
            self.xr_view_matrix_initialized = false;
            self.xr_visible = true;
            self.xr_draw_logged = false;
            self.xr_panel_log_count = 0;
        }

        if matches!(event, Event::XrUpdate(_)) && !self.xr_runtime_active {
            self.xr_runtime_active = true;
            cx.widget_tree_mark_dirty(self.uid);
            cx.redraw_all();
        }

        if !self.permissions_widget.is_empty() {
            self.permissions_widget.handle_event(cx, event, scope);
        }

        let control_widget = self.active_control_widget();
        if !control_widget.is_empty() {
            control_widget.handle_event(cx, event, scope);
        }

        let scene_widget = self.active_scene_widget();
        if !scene_widget.is_empty() {
            scene_widget.handle_event(cx, event, scope);
        }

        let handled_scene_env = {
            let scene_widget = self.active_scene_widget();
            let handled = if let Some(mut scene) = scene_widget.borrow_mut::<XrScene>() {
                self.env.handle_event(cx, event, Some(&mut scene));
                true
            } else {
                false
            };
            handled
        };
        if !handled_scene_env {
            self.env.handle_event(cx, event, None);
        }

        if !self.xr_is_active(cx) {
            return;
        }

        if let Event::XrUpdate(update) = event {
            if self.xr_toggle_with_menu && update.menu_pressed() {
                self.xr_visible = !self.xr_visible;
                cx.redraw_all();
            }
            if !self.xr_view_matrix_initialized || Self::xr_should_reanchor_panel(update) {
                self.xr_view_matrix =
                    self.compute_xr_panel_matrix(update.state.as_ref(), self.xr_depth_scale);
                self.xr_hit_matrix = self.compute_xr_panel_matrix(update.state.as_ref(), 1.0);
                let panel_matrix = self.xr_view_matrix;
                let hit_matrix = self.xr_hit_matrix;
                self.log_xr_panel_pose(update.state.as_ref(), &panel_matrix, &hit_matrix);
                self.xr_view_matrix_initialized = true;
                self.xr_visible = true;
            }

            if !control_widget.is_empty() {
                let xr_event = XrLocalEvent::from_update_event(update, &self.xr_hit_matrix);
                if self.xr_visible {
                    control_widget.handle_event(cx, &Event::XrLocal(xr_event.clone()), scope);
                }
                xr_event.process_end(cx);
            }
        }

        if matches!(event, Event::Startup) {
            cx.with_vm(|vm| {
                let scene_widget = self.active_scene_widget();
                if !scene_widget.is_empty() {
                    let _ = scene_widget.script_call(vm, live_id!(render), NIL);
                }
            });
            cx.redraw_all();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        if self.xr_is_active(cx.cx) && cx.draw_event.xr_state.is_some() {
            return DrawStep::done();
        }

        if self.draw_state.begin(cx, DrawState::Drawing) {
            if self.begin_preview(cx).is_not_redrawing() {
                self.draw_state.end();
                return DrawStep::done();
            }
        }

        if let Some(DrawState::Drawing) = self.draw_state.get() {
            self.log_desktop_state("draw_walk");
            let padding = self.desktop_padding.max(0.0);
            let preview_layout = Layout {
                spacing: self.desktop_spacing.max(0.0),
                padding: Inset {
                    left: padding,
                    right: padding,
                    top: padding,
                    bottom: padding,
                },
                ..Layout::flow_right()
            };
            cx.begin_turtle(Walk::new(Size::fill(), Size::fill()), preview_layout);

            let control_walk =
                Walk::new(Size::Fixed(self.desktop_control_width.max(0.0)), Size::fill());
            let control_rect = cx.peek_walk_turtle(control_walk);
            self.draw_control_bg.draw_abs(cx, control_rect);
            let control_widget = self.active_control_widget();
            if !control_widget.is_empty() {
                control_widget.draw_walk_all(cx, scope, control_walk);
            } else {
                cx.walk_turtle(control_walk);
            }

            let scene_walk = Walk::new(Size::fill(), Size::fill());
            let scene_rect = cx.peek_walk_turtle(scene_walk);
            self.draw_scene_bg.draw_abs(cx, scene_rect);
            self.area = self.draw_scene_bg.area();
            let scene_widget = self.active_scene_widget();
            if !scene_widget.is_empty() {
                scene_widget.draw_walk_all(cx, scope, scene_walk);
            } else {
                cx.walk_turtle(scene_walk);
            }

            cx.end_turtle();

            if !self.permissions_widget.is_empty() {
                self.permissions_widget.draw_walk_all(
                    cx,
                    scope,
                    Walk::new(Size::fill(), Size::fill()),
                );
            }

            self.draw_state.end();
            self.end_preview(cx);
        }

        DrawStep::done()
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        if !self.xr_runtime_active {
            self.xr_runtime_active = true;
            cx.cx.widget_tree_mark_dirty(self.uid);
        }
        self.draw_xr_content(cx, scope);
        DrawStep::done()
    }
}
