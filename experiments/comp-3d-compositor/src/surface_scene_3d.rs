use makepad_widgets::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceSceneState3D {
    pub camera_pos: Vec3f,
    pub view: Mat4f,
    pub projection_viewport: Mat4f,
    pub clip_ndc: Vec4f,
    pub depth_range: Vec2f,
    pub depth_forward_bias: f32,
}

#[derive(Clone, Debug, Default)]
pub struct SurfaceSceneScope3D {
    pub scene: SurfaceSceneState3D,
    pub world_transform: Mat4f,
}

pub fn surface_scene_state_from_scope(scope: &mut Scope) -> Option<SurfaceSceneState3D> {
    if let Some(scope_3d) = scope.data.get::<SurfaceSceneScope3D>() {
        return Some(scope_3d.scene);
    }
    if let Some(scene) = scope.props.get::<SurfaceSceneState3D>() {
        return Some(*scene);
    }
    scope.data.get::<SurfaceSceneState3D>().copied()
}

pub fn surface_scene_world_transform_from_scope(scope: &mut Scope) -> Mat4f {
    scope
        .data
        .get::<SurfaceSceneScope3D>()
        .map(|scope_3d| scope_3d.world_transform)
        .unwrap_or_else(Mat4f::identity)
}

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.SurfaceScene3DBase = #(SurfaceScene3D::register_widget(vm))
    mod.widgets.SurfaceScene3D = set_type_default() do mod.widgets.SurfaceScene3DBase{
        width: Fill
        height: Fill
        draw_bg +: {
            color: #x0b1118
            draw_depth: -99.0
        }
    }
}

#[derive(Script, WidgetRef, WidgetRegister)]
pub struct SurfaceScene3D {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_list_3d: DrawList2d,
    #[live(46.0)]
    camera_fov_y: f32,
    #[live(10.0)]
    camera_distance: f32,
    #[live(0.02)]
    camera_near: f32,
    #[live(400.0)]
    camera_far: f32,
    #[live(vec3(0.0, 0.4, 0.0))]
    camera_target: Vec3f,
    #[live(vec2(0.0, 1.0))]
    depth_range: Vec2f,
    #[live(0.0)]
    depth_forward_bias: f32,
    #[rust]
    area: Area,
    #[rust]
    layers: ComponentMap<LiveId, WidgetRef>,
    #[rust]
    layer_order: Vec<LiveId>,
    #[rust]
    current_scene_state: SurfaceSceneState3D,
}

pub(crate) fn surface_scene_state_for_rect(
    rect: Rect,
    pass_size: Vec2d,
    camera_fov_y: f32,
    camera_distance: f32,
    camera_near: f32,
    camera_far: f32,
    camera_target: Vec3f,
    depth_range: Vec2f,
    depth_forward_bias: f32,
) -> SurfaceSceneState3D {
    let pass_w = pass_size.x.max(1.0) as f32;
    let pass_h = pass_size.y.max(1.0) as f32;
    let x0 = (2.0 * rect.pos.x as f32 / pass_w) - 1.0;
    let x1 = (2.0 * (rect.pos.x + rect.size.x) as f32 / pass_w) - 1.0;
    let y0 = 1.0 - (2.0 * rect.pos.y as f32 / pass_h);
    let y1 = 1.0 - (2.0 * (rect.pos.y + rect.size.y) as f32 / pass_h);
    let clip_ndc = vec4(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
    let sx = (clip_ndc.z - clip_ndc.x) * 0.5;
    let sy = (clip_ndc.w - clip_ndc.y) * 0.5;
    let tx = (clip_ndc.z + clip_ndc.x) * 0.5;
    let ty = (clip_ndc.w + clip_ndc.y) * 0.5;
    let viewport = Mat4f {
        v: [
            sx, 0.0, 0.0, 0.0,
            0.0, sy, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            tx, ty, 0.0, 1.0,
        ],
    };
    let viewport_w = ((clip_ndc.z - clip_ndc.x).abs() * 0.5 * pass_size.x as f32).max(1.0);
    let viewport_h = ((clip_ndc.w - clip_ndc.y).abs() * 0.5 * pass_size.y as f32).max(1.0);
    let aspect = (viewport_w / viewport_h).max(0.001);
    let projection = Mat4f::perspective(
        camera_fov_y.clamp(1.0, 179.0),
        aspect,
        camera_near.max(0.001),
        camera_far.max(camera_near + 0.001),
    );
    let camera_pos = camera_target + vec3(0.0, 0.0, camera_distance);
    let view = Mat4f::look_at(camera_pos, camera_target, vec3(0.0, 1.0, 0.0));
    SurfaceSceneState3D {
        camera_pos,
        view,
        projection_viewport: Mat4f::mul(&viewport, &projection),
        clip_ndc,
        depth_range,
        depth_forward_bias,
    }
}

impl SurfaceScene3D {
    fn draw_children_3d(&mut self, cx: &mut Cx2d, rect: Rect) {
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return;
        }

        let pass_size = cx.current_pass_size();
        if pass_size.x <= 1.0 || pass_size.y <= 1.0 {
            return;
        }

        self.current_scene_state = surface_scene_state_for_rect(
            rect,
            pass_size,
            self.camera_fov_y,
            self.camera_distance,
            self.camera_near,
            self.camera_far,
            self.camera_target,
            self.depth_range,
            self.depth_forward_bias,
        );

        let layer_refs: Vec<WidgetRef> = self
            .layer_order
            .iter()
            .filter_map(|id| self.layers.get(id).cloned())
            .collect();
        if layer_refs.is_empty() {
            return;
        }

        let mut scene_scope_data = SurfaceSceneScope3D {
            scene: self.current_scene_state,
            world_transform: Mat4f::identity(),
        };
        let mut scene_scope = Scope::with_data(&mut scene_scope_data);
        let cx3d = &mut Cx3d::new(cx.cx);
        for layer in layer_refs {
            layer.draw_3d_all(cx3d, &mut scene_scope);
        }
    }
}

impl WidgetNode for SurfaceScene3D {
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
        for id in &self.layer_order {
            if let Some(layer) = self.layers.get(id) {
                visit(*id, layer.clone());
            }
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for SurfaceScene3D {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();

        self.draw_list_3d.begin_always(cx);
        self.draw_children_3d(cx, rect);
        self.draw_list_3d.end(cx);
        DrawStep::done()
    }
}

impl ScriptHook for SurfaceScene3D {
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
                        if let Some(layer) = self.layers.get(&id) {
                            vm.cx_mut()
                                .widget_tree_insert_child_deep(self.uid, id, layer.clone());
                        }
                    }
                });

                self.layers.retain(|id, _| self.layer_order.contains(id));
            }
        }
        vm.cx_mut().widget_tree_mark_dirty(self.uid);
    }
}
