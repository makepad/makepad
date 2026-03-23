use crate::scene_draw::SceneState3D;
use crate::xr_env::XrEnv;
use crate::*;
use makepad_widgets::makepad_script::ScriptFnRef;
use std::rc::Rc;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.XrRootBase = #(XrRoot::register_widget(vm))
    mod.widgets.XrRoot = set_type_default() do mod.widgets.XrRootBase{
        width: Fill
        height: Fill
        flow: Overlay

        window +: {
            inner_size: vec2(1400, 900)
        }
        pass +: {
            clear_color: #x0b1118
        }
        env: mod.widgets.XrEnv{}
    }
}

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct XrRoot {
    #[uid] uid: WidgetUid,
    #[source] source: ScriptObjectRef,
    #[rust] area: Area,
    #[walk] walk: Walk,
    #[layout] layout: Layout,

    // Window + Pass
    #[live] window: ScriptWindowHandle,
    #[live] pass: ScriptDrawPass,
    #[new] depth_texture: Texture,
    #[new] main_draw_list: DrawList2d,
    #[new] xr_draw_list: DrawList,

    // Environment (physics + env draws)
    #[live] env: XrEnv,

    // Desktop orbit camera
    #[live(28.0)] camera_fov_y: f32,
    #[live(3.4)] camera_distance: f32,
    #[live(0.05)] camera_near: f32,
    #[live(200.0)] camera_far: f32,
    #[live(0.25)] camera_distance_min: f32,
    #[live(30.0)] camera_distance_max: f32,
    #[live(0.08)] wheel_zoom_step: f32,
    #[rust(0.0)] orbit_yaw: f32,
    #[rust(0.45)] orbit_pitch: f32,
    #[rust] orbit_last_abs: Option<DVec2>,

    // Startup callback
    #[live] on_startup: ScriptFnRef,

    // Children (from := declarations)
    #[rust] children: Vec<(LiveId, WidgetRef)>,

    // State
    #[rust] initialized: bool,
    #[rust] started: bool,
    #[rust] last_xr_state: Option<Rc<XrState>>,
    #[rust] next_frame: NextFrame,

    // Depth range for pass
    #[live(vec2(0.0, 1.0))] depth_range: Vec2f,
    #[live(0.0)] depth_forward_bias: f32,
}

impl XrRoot {
    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized { return; }
        self.initialized = true;
        self.window.handle.set_pass(cx, &self.pass.handle);
        self.pass.handle.set_pass_name(cx, "xr_root_window");
        self.depth_texture = Texture::new_with_format(cx, TextureFormat::DepthD32 {
            size: TextureSize::Auto,
            initial: true,
        });
        self.pass.handle.set_depth_texture(
            cx,
            &self.depth_texture,
            DrawPassClearDepth::ClearWith(1.0),
        );
    }

    fn set_pass_camera(&self, cx: &mut Cx, scene: &SceneState3D) {
        let camera_inv = scene.view.invert();
        let pass_uniforms = &mut cx.passes[self.pass.handle.draw_pass_id()].pass_uniforms;
        pass_uniforms.camera_projection = scene.projection;
        pass_uniforms.camera_projection_r = scene.projection;
        pass_uniforms.camera_view = scene.view;
        pass_uniforms.camera_view_r = scene.view;
        pass_uniforms.depth_projection = scene.projection;
        pass_uniforms.depth_projection_r = scene.projection;
        pass_uniforms.depth_view = scene.view;
        pass_uniforms.depth_view_r = scene.view;
        pass_uniforms.camera_inv = camera_inv;
        pass_uniforms.camera_inv_r = camera_inv;
    }

    fn desktop_scene_state(&self, viewport_rect: Rect, time: f64) -> Option<SceneState3D> {
        if viewport_rect.size.x <= 1.0 || viewport_rect.size.y <= 1.0 { return None; }

        let distance = self.camera_distance.clamp(
            self.camera_distance_min.max(0.01),
            self.camera_distance_max.max(self.camera_distance_min.max(0.01) + 0.01),
        );
        let pitch = self.orbit_pitch.clamp(-1.45, 1.45);
        let cos_pitch = pitch.cos();
        let position = vec3f(
            distance * self.orbit_yaw.sin() * cos_pitch,
            distance * pitch.sin(),
            distance * self.orbit_yaw.cos() * cos_pitch,
        );
        let forward = (vec3f(0.0, 0.0, 0.0) - position).normalize();
        let head_pose = Pose::new(Quat::look_rotation(forward, vec3f(0.0, 1.0, 0.0)), position);

        let aspect = (viewport_rect.size.x / viewport_rect.size.y).max(0.001) as f32;
        let projection = Mat4f::perspective(
            self.camera_fov_y.clamp(1.0, 179.0),
            aspect,
            self.camera_near.max(0.001),
            self.camera_far.max(self.camera_near + 0.001),
        );

        Some(SceneState3D {
            time,
            camera_pos: head_pose.position,
            view: head_pose.to_mat4().invert(),
            projection,
            clip_ndc: vec4(-1.0, -1.0, 1.0, 1.0),
            depth_range: self.depth_range,
            depth_forward_bias: self.depth_forward_bias,
            use_pass_camera: true,
            viewport_rect,
        })
    }

    fn xr_scene_state(&self, state: &XrState) -> SceneState3D {
        SceneState3D {
            time: state.time,
            camera_pos: state.head_pose.position,
            view: Mat4f::identity(),
            projection: Mat4f::identity(),
            clip_ndc: vec4(-1.0, -1.0, 1.0, 1.0),
            depth_range: self.depth_range,
            depth_forward_bias: self.depth_forward_bias,
            use_pass_camera: true,
            viewport_rect: Rect::default(),
        }
    }

    fn draw_3d_content(&mut self, cx: &mut Cx3d, _scope: &mut Scope) {
        self.xr_draw_list.begin_always(cx);

        let scene_state = if let Some(state) = self.last_xr_state.as_ref() {
            self.xr_scene_state(state)
        } else if let Some(ss) = cx.scene_state_3d() {
            ss
        } else {
            SceneState3D::default()
        };

        // Prepare env draws + get scope data
        let mut draw_scope = {
            let cx2d = &mut Cx2d::new(cx.cx);
            self.env.prepare_and_draw(cx2d)
        };

        cx.begin_scene_3d(scene_state);
        let mut scene_scope = Scope::with_data(&mut draw_scope);
        for i in 0..self.children.len() {
            let child = self.children[i].1.clone();
            child.draw_3d_all(cx, &mut scene_scope);
        }
        cx.end_scene_3d();

        self.xr_draw_list.end(cx);
    }

    fn handle_draw_event(&mut self, cx: &mut Cx, e: &DrawEvent, scope: &mut Scope) {
        self.ensure_initialized(cx);
        if cx.in_xr_mode() {
            if e.xr_state.is_none() { return; }
            let mut cx_draw = CxDraw::new(cx, e);
            let cx3d = &mut Cx3d::new(&mut cx_draw);
            self.pass.handle.set_as_xr_pass(cx3d);
            cx3d.begin_pass(&self.pass.handle, Some(4.0));
            self.draw_3d_content(cx3d, scope);
            cx3d.end_pass(&self.pass.handle);
        } else {
            let mut cx_draw = CxDraw::new(cx, e);
            let cx2d = &mut Cx2d::new(&mut cx_draw);
            self.draw_all(cx2d, scope);
        }
    }

    fn handle_desktop_interaction(&mut self, cx: &mut Cx, event: &Event) {
        match event.hits_with_capture_overload(cx, self.area, true) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.orbit_last_abs = Some(fe.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                if let Some(last_abs) = self.orbit_last_abs {
                    let delta = fe.abs - last_abs;
                    self.orbit_yaw -= (delta.x as f32) * 0.01;
                    self.orbit_pitch = (self.orbit_pitch + (delta.y as f32) * 0.01).clamp(-1.45, 1.45);
                    self.orbit_last_abs = Some(fe.abs);
                    cx.redraw_all();
                }
            }
            Hit::FingerScroll(fs) => {
                let scroll_axis = if fs.scroll.y.abs() > f64::EPSILON { fs.scroll.y } else { fs.scroll.x };
                if scroll_axis.abs() > f64::EPSILON {
                    let step = self.wheel_zoom_step.max(0.001);
                    let factor = if scroll_axis > 0.0 { 1.0 / (1.0 - step) } else { 1.0 - step };
                    self.camera_distance = (self.camera_distance * factor).clamp(
                        self.camera_distance_min.max(0.01),
                        self.camera_distance_max.max(self.camera_distance_min.max(0.01) + 0.01),
                    );
                    cx.redraw_all();
                }
            }
            Hit::FingerUp(_) => {
                self.orbit_last_abs = None;
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerHoverIn(_) => { cx.set_cursor(MouseCursor::Grab); }
            Hit::FingerHoverOut(_) => { cx.set_cursor(MouseCursor::Default); }
            _ => {}
        }
    }

    fn select_scene_by_id(&self, cx: &mut Cx, target_id: LiveId) {
        for (_, child) in &self.children {
            if self.select_scene_recursive(cx, child, target_id) {
                return;
            }
        }
    }

    fn select_scene_recursive(&self, cx: &mut Cx, widget: &WidgetRef, target_id: LiveId) -> bool {
        let mut siblings = Vec::new();
        let mut found = false;
        widget.children(&mut |id, child| {
            siblings.push((id, child.clone()));
            if id == target_id { found = true; }
        });
        if found {
            for (id, child) in &siblings {
                child.set_visible(cx, *id == target_id);
            }
            return true;
        }
        for (_, child) in &siblings {
            if self.select_scene_recursive(cx, child, target_id) {
                return true;
            }
        }
        false
    }

    pub fn depth_mesh_visible(&self) -> bool {
        self.env.depth_mesh_visible()
    }

    pub fn toggle_depth_mesh_visible(&mut self, cx: &mut Cx) -> bool {
        let visible = self.env.toggle_depth_mesh_visible();
        cx.redraw_all();
        visible
    }
}

impl ScriptHook for XrRoot {
    fn on_before_apply(&mut self, _vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        if apply.is_reload() {
            self.children.clear();
        }
    }

    fn on_after_apply(&mut self, vm: &mut ScriptVm, apply: &Apply, scope: &mut Scope, value: ScriptValue) {
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                self.children.clear();
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        let Some(id) = kv.key.as_id() else { continue };
                        if !WidgetRef::value_is_newable_widget(vm, kv.value) { continue }
                        let child = WidgetRef::script_from_value_scoped(vm, scope, kv.value);
                        self.children.push((id, child));
                    }
                });
            }
        }
        vm.with_cx_mut(|cx| {
            cx.widget_tree_mark_dirty(self.uid);
        });
    }
}

impl WidgetNode for XrRoot {
    fn widget_uid(&self) -> WidgetUid { self.uid }
    fn walk(&mut self, _cx: &mut Cx) -> Walk { self.walk }
    fn area(&self) -> Area { self.area }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, child) in &self.children {
            visit(*id, child.clone());
        }
    }

    fn redraw(&mut self, cx: &mut Cx) { cx.redraw_all(); }
}

impl Widget for XrRoot {
    fn script_call(&mut self, vm: &mut ScriptVm, method: LiveId, args: ScriptValue) -> ScriptAsyncResult {
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
            self.env.mark_scene_dirty();
            for i in 0..self.children.len() {
                let child = self.children[i].1.clone();
                let _ = child.script_call(vm, live_id!(render), NIL);
            }
            vm.with_cx_mut(|cx| {
                self.env.ensure_physics(cx, &self.children);
            });
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(select_scene) {
            let Some(scene_id) = args.as_id() else {
                return ScriptAsyncResult::MethodNotFound;
            };
            vm.with_cx_mut(|cx| {
                self.select_scene_by_id(cx, scene_id);
            });
            return ScriptAsyncResult::Return(NIL);
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Draw(e) = event {
            self.handle_draw_event(cx, e, scope);
            return;
        }

        if let Event::Startup = event {
            if !self.started {
                self.started = true;
                cx.widget_to_script_call(self.uid, NIL, self.source.clone(), self.on_startup.clone(), &[]);
                // Also render all children directly (script callback may not dispatch correctly)
                cx.with_vm(|vm| {
                    for i in 0..self.children.len() {
                        let child = self.children[i].1.clone();
                        let _ = child.script_call(vm, live_id!(render), NIL);
                    }
                });
                self.env.ensure_physics(cx, &self.children);
                self.next_frame = cx.new_next_frame();
                cx.redraw_all();
            }
        }

        if let Event::XrUpdate(update) = event {
            self.last_xr_state = Some(update.state.clone());
            if update.clicked_menu() {
                self.env.reset_physics(cx);
            }
            self.env.ensure_physics(cx, &self.children);
            self.env.step_physics(cx);
        }

        if let Event::NextFrame(_ne) = event {
            if self.next_frame.is_event(event).is_some() {
                if !cx.in_xr_mode() {
                    self.env.ensure_physics(cx, &self.children);
                    self.env.step_physics(cx);
                }
                self.next_frame = cx.new_next_frame();
            }
        }

        self.env.handle_event(cx, event);

        for i in 0..self.children.len() {
            let child = self.children[i].1.clone();
            child.handle_event(cx, event, scope);
        }

        if !cx.in_xr_mode() {
            self.handle_desktop_interaction(cx, event);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        if cx.cx.in_xr_mode() { return DrawStep::done(); }

        self.ensure_initialized(cx.cx);
        let will_redraw = cx.will_redraw(&mut self.main_draw_list, Walk::default());
        if !will_redraw { return DrawStep::done(); }

        cx.begin_pass(&self.pass.handle, None);
        self.main_draw_list.begin_always(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());

        let pass_rect = Rect { pos: dvec2(0.0, 0.0), size };
        cx.add_aligned_rect_area(&mut self.area, pass_rect);

        if let Some(scene_state) = self.desktop_scene_state(pass_rect, cx.time()) {
            self.set_pass_camera(cx.cx, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_3d_content(cx3d, scope);
        }

        cx.end_pass_sized_turtle();
        self.main_draw_list.end(cx);
        cx.end_pass(&self.pass.handle);
        DrawStep::done()
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        self.draw_3d_content(cx, scope);
        DrawStep::done()
    }
}
