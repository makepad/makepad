use crate::prelude::*;
use crate::scene::{
    xr_draw_list_depth, xr_sort_child_draw_order, xr_widget_is_transparent,
    xr_widget_local_sort_center, XrDrawScopeData,
};
use makepad_widgets::{makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    use mod.widgets.*

    mod.draw.DrawXrSceneTexture = mod.std.set_type_default() do #(DrawXrSceneTexture::script_shader(vm)){
        ..mod.draw.DrawQuad
        scene_texture: texture_2d(float)
        tint: vec4(1.0, 1.0, 1.0, 1.0)

        pixel: fn() {
            let color = self.scene_texture.sample_as_bgra(self.pos)
            return Pal.premul(color * self.tint)
        }
    }

    mod.widgets.XrSceneViewBase = #(XrSceneView::register_widget(vm))
    mod.widgets.XrSceneView = set_type_default() do mod.widgets.XrSceneViewBase{
        width: Fill
        height: Fill
        clear_color: #x0b1016
        camera: mod.widgets.XrCamera{
            fov_y: 34.0
            desktop_target: vec3(0.0, 0.02, 0.0)
            distance: 3.8
            distance_min: 0.35
            distance_max: 40.0
            wheel_zoom_step: 0.10
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawXrSceneTexture {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    tint: Vec4f,
}

impl DrawXrSceneTexture {
    pub fn set_scene_texture(&mut self, texture: &Texture) {
        self.draw_super.draw_vars.set_texture(0, texture);
    }
}

#[derive(Script, WidgetRef, WidgetRegister)]
pub struct XrSceneView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawXrSceneTexture,
    #[live(vec4(0.043, 0.063, 0.086, 1.0))]
    clear_color: Vec4f,
    #[live]
    camera: XrCamera,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    area: Area,
    #[rust(false)]
    initialized: bool,
    #[rust]
    children: Vec<(LiveId, WidgetRef)>,
    #[rust]
    live_update_order: Vec<LiveId>,
}

impl XrSceneView {
    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
    }

    fn set_pass_camera(&self, cx: &mut Cx, scene: &SceneState3D) {
        let camera_inv = scene.view.invert();
        let pass_uniforms = &mut cx.passes[self.pass.draw_pass_id()].pass_uniforms;
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

    fn transform_point(transform: &Mat4f, point: Vec3f) -> Vec3f {
        let v = transform.transform_vec4(vec4(point.x, point.y, point.z, 1.0));
        if v.w.abs() > 0.000_001 {
            vec3(v.x / v.w, v.y / v.w, v.z / v.w)
        } else {
            vec3(v.x, v.y, v.z)
        }
    }

    fn draw_scene(&mut self, cx: &mut Cx3d, _scope: &mut Scope, scene_state: SceneState3D) {
        self.draw_list.begin_always(cx);
        cx.begin_scene_3d(scene_state);
        let previous_world = cx.set_scene_world_transform_3d(Mat4f::identity());

        let mut draw_scope = XrDrawScopeData {
            tracking_from_content: Mat4f::identity(),
            content_from_tracking: Mat4f::identity(),
            ..Default::default()
        };
        let mut scene_scope = Scope::with_data(&mut draw_scope);

        let mut draw_order_entries = Vec::with_capacity(self.children.len());
        for i in 0..self.children.len() {
            let child = self.children[i].1.clone();
            let child_center = xr_widget_local_sort_center(&child)
                .map(|center| Self::transform_point(&Mat4f::identity(), center));
            if let Some(child_center) = child_center {
                draw_order_entries.push((
                    i,
                    xr_draw_list_depth(&scene_state, child_center),
                    xr_widget_is_transparent(&child),
                ));
            } else {
                draw_order_entries.push((i, 0.0, false));
            }
        }
        xr_sort_child_draw_order(&mut draw_order_entries);

        for (index, _, _) in draw_order_entries {
            let child = self.children[index].1.clone();
            child.draw_3d_all(cx, &mut scene_scope);
        }

        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }
        cx.end_scene_3d();
        self.draw_list.end(cx);
    }
}

impl ScriptHook for XrSceneView {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.live_update_order.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if let Some(obj) = value.as_object() {
            if apply.is_reload() {
                self.live_update_order.clear();
            } else {
                self.children.clear();
            }
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
                    if apply.is_reload() {
                        self.live_update_order.push(id);
                    }

                    if let Some((_, child)) = self
                        .children
                        .iter_mut()
                        .find(|(child_id, _)| *child_id == id)
                    {
                        child.script_apply(vm, apply, scope, kv.value);
                    } else {
                        let child = WidgetRef::script_from_value_scoped(vm, scope, kv.value);
                        self.children.push((id, child.clone()));
                    }

                    if let Some((_, child)) =
                        self.children.iter().find(|(child_id, _)| *child_id == id)
                    {
                        vm.cx_mut()
                            .widget_tree_insert_child_deep(self.uid, id, child.clone());
                    }
                }
            });
        }

        if apply.is_reload() {
            if !self.live_update_order.is_empty() || self.children.is_empty() {
                for (idx, id) in self.live_update_order.iter().enumerate() {
                    if let Some(pos) = self
                        .children
                        .iter()
                        .position(|(child_id, _)| child_id == id)
                    {
                        self.children.swap(idx, pos);
                    }
                }
                self.children.truncate(self.live_update_order.len());
            }
        }

        vm.cx_mut().widget_tree_mark_dirty(self.uid);
    }
}

impl WidgetNode for XrSceneView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, child) in &self.children {
            visit(*id, child.clone());
        }
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for XrSceneView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.camera.handle_desktop_interaction(cx, event);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return DrawStep::done();
        }

        self.ensure_initialized(cx.cx);
        self.camera.set_desktop_viewport_rect(rect);
        self.pass.set_size(cx, rect.size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));

        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        if let Some(scene_state) = self.camera.desktop_scene_state(rect, cx.time()) {
            self.set_pass_camera(cx.cx, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_scene(cx3d, scope, scene_state);
        }
        cx.end_pass(&self.pass);

        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        cx.set_pass_area(&self.pass, self.area);
        DrawStep::done()
    }
}
