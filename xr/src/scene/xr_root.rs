use crate::*;
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.XrRootBase = #(XrRoot::register_widget(vm))
    mod.widgets.XrRoot = set_type_default() do mod.widgets.XrRootBase{
        xr_panel_pixels: vec2(960.0, 1200.0)
        width: Fill
        height: Fill
        flow: Overlay

        desktop := View{
            width: Fill
            height: Fill
            flow: Right
            spacing: 0

            control_host := View{
                width: 360
                height: Fill
                flow: Down
                padding: Inset{left: 20 right: 20 top: 20 bottom: 20}
                spacing: 12
                show_bg: true
                draw_bg.color: #x0d1520
            }

            scene_host := View{
                width: Fill
                height: Fill
                show_bg: true
                draw_bg.color: #x10161f
            }
        }

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

#[derive(Script, Widget)]
pub struct XrRoot {
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
    #[rust]
    template_widgets: HashMap<LiveId, WidgetRef>,
    #[rust]
    scene_order: Vec<LiveId>,
    #[rust]
    active_scene: LiveId,
    #[rust]
    sync_pending: bool,
    #[new]
    xr_draw_list: DrawList,
    #[new]
    xr_control_draw_list: DrawList2d,
    #[live]
    xr_pass: ScriptDrawPass,
    #[rust(Mat4f::nonuniform_scaled_translation(vec3(0.0004,-0.0004,0.12),vec3(-0.25,0.25,-0.5)))]
    xr_view_matrix: Mat4f,
    #[rust(Mat4f::nonuniform_scaled_translation(vec3(0.0004,-0.0004,0.0004),vec3(-0.25,0.25,-0.5)))]
    xr_hit_matrix: Mat4f,
    #[rust]
    xr_view_matrix_initialized: bool,
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
    #[deref]
    view: View,
}

impl XrRoot {
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

    fn active_control_id(&self, in_xr_mode: bool) -> LiveId {
        if in_xr_mode
            && self.control_xr != LiveId(0)
            && self.template_widget(self.control_xr).is_some()
        {
            self.control_xr
        } else {
            self.control_2d
        }
    }

    fn active_control_widget(&self, in_xr_mode: bool) -> WidgetRef {
        if in_xr_mode && self.control_xr != LiveId(0) {
            if let Some(widget) = self.template_widget(self.control_xr) {
                return widget;
            }
        }
        self.template_widget(self.control_2d)
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
        self.sync_pending = true;
        self.try_sync_hosts(cx);
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

    fn set_host_child(&self, cx: &mut Cx, host: WidgetRef, child_id: LiveId, child: WidgetRef) -> bool {
        let Some(mut host_view) = host.borrow_mut::<View>() else {
            return false;
        };
        let parent_uid = host_view.widget_uid();
        let replace = host_view.children.len() != 1
            || host_view.children[0].0 != child_id
            || host_view.children[0].1 != child;
        if !replace {
            return true;
        }
        host_view.children.clear();
        if !child.is_empty() {
            host_view.children.push((child_id, child.clone()));
        }
        drop(host_view);
        cx.widget_tree_mark_dirty(parent_uid);
        if !child.is_empty() {
            cx.widget_tree_insert_child_deep(parent_uid, child_id, child);
        }
        true
    }

    fn try_sync_hosts(&mut self, cx: &mut Cx) {
        let control_host = self.widget(cx, ids!(control_host));
        let scene_host = self.widget(cx, ids!(scene_host));
        if control_host.is_empty() || scene_host.is_empty() {
            return;
        }

        let in_xr_mode = cx.in_xr_mode();
        let control_widget = self.active_control_widget(in_xr_mode);
        let control_id = self.active_control_id(in_xr_mode);
        let scene_id = self.active_scene_id();
        let scene_widget = self.active_scene_widget();

        if control_id != LiveId(0) {
            self.set_host_child(cx, control_host, control_id, control_widget);
        }
        if scene_id != LiveId(0) {
            self.set_host_child(cx, scene_host, scene_id, scene_widget);
        }
        self.sync_pending = false;
    }

    fn draw_xr_controls(&mut self, cx: &mut Cx2d, scope: &mut Scope) {
        if !self.xr_visible {
            return;
        }
        let control_widget = self.active_control_widget(true);
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

        self.view
            .children
            .retain(|(id, _)| !template_ids.contains(id));
        vm.cx_mut().widget_tree_mark_dirty(self.widget_uid());

        self.sync_pending = true;
        self.try_sync_hosts(vm.cx_mut());
        let scene_widget = self.active_scene_widget();
        if !scene_widget.is_empty() {
            let _ = scene_widget.script_call(vm, live_id!(render), NIL);
        }
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
        if self.sync_pending {
            self.try_sync_hosts(cx);
        }

        if !cx.in_xr_mode() {
            self.xr_view_matrix_initialized = false;
            self.xr_visible = true;
        }

        if let Event::Draw(e) = event {
            self.try_sync_hosts(cx);
            if !cx.in_xr_mode() {
                self.view.handle_event(cx, event, scope);
                return;
            }
            if e.xr_state.is_none() {
                return;
            }

            let mut cx_draw = CxDraw::new(cx, e);
            let cx3d = &mut Cx3d::new(&mut cx_draw);
            self.xr_pass.handle.set_as_xr_pass(cx3d);
            cx3d.begin_pass(&self.xr_pass.handle, Some(4.0));
            self.xr_draw_list.begin_always(cx3d);
            let scene_widget = self.active_scene_widget();
            if !scene_widget.is_empty() {
                let options = self.options();
                let mut scene_scope = Scope::with_props(&options);
                scene_widget.draw_3d_all(cx3d, &mut scene_scope);
            }
            self.xr_draw_list.end(cx3d);
            let cx2d = &mut Cx2d::new(cx3d.cx);
            self.draw_xr_controls(cx2d, scope);
            cx3d.end_pass(&self.xr_pass.handle);
            return;
        }

        self.view.handle_event(cx, event, scope);

        if !cx.in_xr_mode() {
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
                self.xr_view_matrix_initialized = true;
                self.xr_visible = true;
            }

            let control_widget = self.active_control_widget(true);
            if !control_widget.is_empty() {
                let xr_event = XrLocalEvent::from_update_event(update, &self.xr_hit_matrix);
                if self.xr_visible {
                    control_widget.handle_event(cx, &Event::XrLocal(xr_event.clone()), scope);
                }
                xr_event.process_end(cx);
            }
            self.try_sync_hosts(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.try_sync_hosts(cx.cx);
        self.view.draw_walk(cx, scope, walk)
    }
}
