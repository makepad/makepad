use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*, widget_tree::CxWidgetExt};

use super::chart_3d::Chart3DData;

#[derive(Clone, Copy, Debug, Default)]
pub struct SceneState3D {
    pub time: f64,
    pub camera_pos: Vec3f,
    pub view: Mat4f,
    pub projection: Mat4f,
    pub projection_viewport: Mat4f,
    pub clip_ndc: Vec4f,
    pub depth_range: Vec2f,
    pub depth_forward_bias: f32,
    pub viewport_rect: Rect,
}

#[derive(Clone, Debug, Default)]
pub struct SceneScope3D {
    pub scene: SceneState3D,
    pub chart_data: Option<Chart3DData>,
}

pub fn scene_state_from_scope(scope: &mut Scope) -> Option<SceneState3D> {
    if let Some(scope_3d) = scope.data.get::<SceneScope3D>() {
        return Some(scope_3d.scene);
    }
    if let Some(scene) = scope.props.get::<SceneState3D>() {
        return Some(*scene);
    }
    scope.data.get::<SceneState3D>().copied()
}

pub fn chart_data_from_scope(scope: &mut Scope) -> Option<Chart3DData> {
    if let Some(scope_3d) = scope.data.get::<SceneScope3D>() {
        return scope_3d.chart_data.clone();
    }
    scope.data.get::<Chart3DData>().cloned()
}

pub fn apply_scene_to_draw_pbr(draw: &mut DrawPbr, cx: &mut Cx2d, scene: &SceneState3D) {
    draw.set_camera_state(scene.view, scene.projection_viewport, scene.camera_pos);
    draw.set_clip_ndc(scene.clip_ndc);
    draw.set_depth_range(scene.depth_range.x, scene.depth_range.y);
    draw.set_depth_forward_bias(scene.depth_forward_bias);
    if draw.has_env_texture < 0.5 {
        let env_texture = draw.default_env_texture(cx);
        draw.set_env_texture(Some(env_texture));
    }
    draw.reset_matrix();
}

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.Scene3DBase = #(Scene3D::register_widget(vm))

    mod.widgets.Scene3D = set_type_default() do mod.widgets.Scene3DBase{
        width: Fill
        height: Fill
        draw_bg +: {
            color: #x171d26
            draw_depth: -99.0
        }
    }
}

#[derive(Script, Widget)]
pub struct Scene3D {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_list_3d: DrawList2d,

    #[live(true)]
    animating: bool,
    #[live(0.0)]
    spin_speed: f32,
    #[live(40.0)]
    camera_fov_y: f32,
    #[live(10.0)]
    camera_distance: f32,
    #[live(0.6)]
    camera_distance_min: f32,
    #[live(80.0)]
    camera_distance_max: f32,
    #[live(0.1)]
    wheel_zoom_step: f32,
    #[live(0.05)]
    camera_near: f32,
    #[live(200.0)]
    camera_far: f32,
    #[live(vec2(0.0, 1.0))]
    depth_range: Vec2f,
    #[live(0.0)]
    depth_forward_bias: f32,

    #[rust]
    next_frame: NextFrame,
    #[rust]
    time: f64,
    #[rust]
    area: Area,
    #[rust(0.0)]
    orbit_yaw: f32,
    #[rust(0.45)]
    orbit_pitch: f32,
    #[rust]
    drag_last_abs: Option<DVec2>,
    #[live(vec3(0.0, 0.0, 0.0))]
    camera_target: Vec3f,
    #[rust]
    layers: ComponentMap<LiveId, WidgetRef>,
    #[rust]
    layer_order: Vec<LiveId>,
    #[rust]
    current_scene_state: SceneState3D,
}

impl Scene3D {
    fn clip_ndc_for_rect(rect: Rect, pass_size: Vec2d) -> Vec4f {
        let pass_w = pass_size.x.max(1.0) as f32;
        let pass_h = pass_size.y.max(1.0) as f32;

        let x0 = (2.0 * rect.pos.x as f32 / pass_w) - 1.0;
        let x1 = (2.0 * (rect.pos.x + rect.size.x) as f32 / pass_w) - 1.0;
        let y0 = 1.0 - (2.0 * rect.pos.y as f32 / pass_h);
        let y1 = 1.0 - (2.0 * (rect.pos.y + rect.size.y) as f32 / pass_h);

        vec4(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
    }

    fn clip_space_viewport_matrix(clip_ndc: Vec4f) -> Mat4f {
        let sx = (clip_ndc.z - clip_ndc.x) * 0.5;
        let sy = (clip_ndc.w - clip_ndc.y) * 0.5;
        let tx = (clip_ndc.z + clip_ndc.x) * 0.5;
        let ty = (clip_ndc.w + clip_ndc.y) * 0.5;

        Mat4f {
            v: [
                sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, tx, ty, 0.0, 1.0,
            ],
        }
    }

    fn compute_subengine_transform(
        &self,
        rect: Rect,
        pass_size: Vec2d,
    ) -> (Vec3f, Vec4f, Mat4f, Mat4f, Mat4f) {
        let clip_ndc = Self::clip_ndc_for_rect(rect, pass_size);
        let viewport = Self::clip_space_viewport_matrix(clip_ndc);
        let viewport_w = ((clip_ndc.z - clip_ndc.x).abs() * 0.5 * pass_size.x as f32).max(1.0);
        let viewport_h = ((clip_ndc.w - clip_ndc.y).abs() * 0.5 * pass_size.y as f32).max(1.0);
        let aspect = (viewport_w / viewport_h).max(0.001);
        let fov = self.camera_fov_y.clamp(1.0, 179.0);
        let near = self.camera_near.max(0.001);
        let far = self.camera_far.max(near + 0.001);
        let projection = Mat4f::perspective(fov, aspect, near, far);
        let projection_viewport = Mat4f::mul(&viewport, &projection);

        let distance = self.camera_distance.clamp(
            self.camera_distance_min.max(0.01),
            self.camera_distance_max.max(self.camera_distance_min.max(0.01) + 0.01),
        );
        let yaw_angle = self.orbit_yaw + (self.time as f32) * self.spin_speed;
        let pitch_angle = self.orbit_pitch.clamp(-1.45, 1.45);
        let cos_pitch = pitch_angle.cos();
        let camera_pos = vec3(
            distance * yaw_angle.sin() * cos_pitch,
            distance * pitch_angle.sin(),
            distance * yaw_angle.cos() * cos_pitch,
        ) + self.camera_target;
        let view = Mat4f::look_at(camera_pos, self.camera_target, vec3(0.0, 1.0, 0.0));

        (camera_pos, clip_ndc, view, projection, projection_viewport)
    }

    fn draw_children_3d(&mut self, cx: &mut Cx2d, scope: &mut Scope, rect: Rect) {
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return;
        }

        let pass_size = cx.current_pass_size();
        if pass_size.x <= 1.0 || pass_size.y <= 1.0 {
            return;
        }

        let (camera_pos, clip_ndc, view_3d, projection_3d, projection_viewport_3d) =
            self.compute_subengine_transform(rect, pass_size);

        self.current_scene_state = SceneState3D {
            time: self.time,
            camera_pos,
            view: view_3d,
            projection: projection_3d,
            projection_viewport: projection_viewport_3d,
            clip_ndc,
            depth_range: self.depth_range,
            depth_forward_bias: self.depth_forward_bias,
            viewport_rect: rect,
        };

        let layer_refs: Vec<WidgetRef> = self
            .layer_order
            .iter()
            .filter_map(|id| self.layers.get(id).cloned())
            .collect();
        if layer_refs.is_empty() {
            return;
        }

        let chart_data = scope.data.get::<Chart3DData>().cloned();
        let mut scene_scope_data = SceneScope3D {
            scene: self.current_scene_state,
            chart_data,
        };
        let mut scene_scope = Scope::with_data(&mut scene_scope_data);
        let cx3d = &mut Cx3d::new(cx.cx);
        for layer in layer_refs {
            layer.draw_3d_all(cx3d, &mut scene_scope);
        }
    }
}

impl Widget for Scene3D {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        let layer_order = self.layer_order.clone();
        for id in layer_order {
            if let Some(layer) = self.layers.get_mut(&id) {
                let layer_uid = layer.widget_uid();
                cx.group_widget_actions(uid, layer_uid, |cx| {
                    layer.handle_event(cx, event, scope);
                });
            }
        }

        match event.hits_with_capture_overload(cx, self.area, true) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.drag_last_abs = Some(fe.abs);
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                if let Some(last_abs) = self.drag_last_abs {
                    let delta = fe.abs - last_abs;
                    let sensitivity = 0.01_f32;
                    self.orbit_yaw -= (delta.x as f32) * sensitivity;
                    self.orbit_pitch =
                        (self.orbit_pitch + (delta.y as f32) * sensitivity).clamp(-1.45, 1.45);
                    self.drag_last_abs = Some(fe.abs);
                    self.area.redraw(cx);
                }
            }
            Hit::FingerScroll(fs) => {
                let scroll = if fs.scroll.y.abs() > f64::EPSILON {
                    fs.scroll.y
                } else {
                    fs.scroll.x
                };
                let step = self.wheel_zoom_step.max(0.001);
                let zoom_factor = if scroll > 0.0 {
                    1.0 / (1.0 - step)
                } else {
                    1.0 - step
                };
                self.camera_distance = (self.camera_distance * zoom_factor).clamp(
                    self.camera_distance_min.max(0.01),
                    self.camera_distance_max.max(self.camera_distance_min.max(0.01) + 0.01),
                );
                self.area.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                if self.drag_last_abs.take().is_some() {
                    if fe.is_over {
                        cx.set_cursor(MouseCursor::Grab);
                    } else {
                        cx.set_cursor(MouseCursor::Default);
                    }
                }
            }
            Hit::FingerHoverIn(_) => {
                if self.drag_last_abs.is_some() {
                    cx.set_cursor(MouseCursor::Grabbing);
                } else {
                    cx.set_cursor(MouseCursor::Grab);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.drag_last_abs.is_none() {
                    cx.set_cursor(MouseCursor::Default);
                }
            }
            _ => {}
        }

        if self.animating {
            if let Event::NextFrame(ne) = event {
                self.time = ne.time;
                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
            if let Event::Startup = event {
                self.next_frame = cx.new_next_frame();
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();

        self.draw_list_3d.begin_always(cx);
        self.draw_children_3d(cx, scope, rect);
        self.draw_list_3d.end(cx);
        DrawStep::done()
    }
}

impl ScriptHook for Scene3D {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.layers.clear();
            self.layer_order.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                self.layer_order.clear();
                let mut anon_index = 0usize;
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        let id = if let Some(id) = kv.key.as_id() {
                            Some(id)
                        } else if kv.key.is_nil() {
                            let id = LiveId(anon_index as u64);
                            anon_index += 1;
                            Some(id)
                        } else {
                            None
                        };
                        let Some(id) = id else {
                            continue;
                        };
                        if !WidgetRef::value_is_newable_widget(vm, kv.value) {
                            continue;
                        }
                        self.layer_order.push(id);
                        if let Some(layer) = self.layers.get_mut(&id) {
                            layer.script_apply(vm, apply, scope, kv.value);
                        } else {
                            let layer = WidgetRef::script_from_value_scoped(vm, scope, kv.value);
                            self.layers.insert(id, layer);
                        }
                    }
                });

                self.layers.retain(|id, _| self.layer_order.contains(id));
            }
        }
        vm.cx_mut().widget_tree_mark_dirty(self.uid);
    }
}
