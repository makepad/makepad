use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_script::ScriptFnRef,
    widget::*,
    widget_async::{CxWidgetToScriptCallExt, ScriptAsyncCalls, ScriptAsyncId, ScriptAsyncResult},
    widget_tree::CxWidgetExt,
};
use std::{collections::HashMap, rc::Rc};

use super::scene_draw::compose_scene_node_transform;

script_mod! {
    use mod.prelude.widgets_internal.*

    let XrBodyKind = set_type_default() do #(XrBodyKind::script_api(vm))
    mod.widgets.XrBodyKind = XrBodyKind

    mod.widgets.XrNodeBase = #(XrNode::register_widget(vm))
    mod.widgets.XrNode = set_type_default() do mod.widgets.XrNodeBase{
        body: XrBodyKind.Disabled
        physics_size: vec3(0.0, 0.0, 0.0)
        density: 1.0
        friction: 0.8
        restitution: 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Script, ScriptHook)]
pub enum XrBodyKind {
    #[pick]
    Disabled,
    Dynamic,
    Fixed,
}

impl Default for XrBodyKind {
    fn default() -> Self {
        Self::Disabled
    }
}

#[derive(Clone)]
pub struct XrRuntimeBodyState {
    pub pose: Pose,
    pub scale: Vec3f,
}

#[derive(Clone, Default)]
pub struct XrDrawScopeData {
    pub runtime_bodies: Rc<HashMap<WidgetUid, XrRuntimeBodyState>>,
    pub env_texture: Option<Texture>,
    pub camera_texture: Option<Texture>,
    pub camera_source_size: Vec2f,
    pub camera_rotation_steps: f32,
    pub camera_center_offset_uv: Vec2f,
    pub camera_enabled: bool,
    pub pointer_tips: [Option<Vec3f>; 2],
}

pub fn xr_runtime_body_from_scope(
    scope: &mut Scope,
    uid: WidgetUid,
) -> Option<XrRuntimeBodyState> {
    scope
        .data
        .get::<XrDrawScopeData>()
        .and_then(|scope_data| scope_data.runtime_bodies.get(&uid).cloned())
}

pub fn xr_pointer_tips_from_scope(scope: &mut Scope) -> [Option<Vec3f>; 2] {
    scope
        .data
        .get::<XrDrawScopeData>()
        .map(|scope_data| scope_data.pointer_tips)
        .unwrap_or([None, None])
}

pub fn xr_env_texture_from_scope(scope: &mut Scope) -> Option<Texture> {
    scope
        .data
        .get::<XrDrawScopeData>()
        .and_then(|scope_data| scope_data.env_texture.clone())
}

#[derive(Clone, Default)]
pub struct XrPassthroughScopeData {
    pub camera_texture: Option<Texture>,
    pub source_size: Vec2f,
    pub rotation_steps: f32,
    pub center_offset_uv: Vec2f,
    pub enabled: bool,
}

pub fn xr_passthrough_from_scope(scope: &mut Scope) -> XrPassthroughScopeData {
    scope
        .data
        .get::<XrDrawScopeData>()
        .map(|scope_data| XrPassthroughScopeData {
            camera_texture: scope_data.camera_texture.clone(),
            source_size: scope_data.camera_source_size,
            rotation_steps: scope_data.camera_rotation_steps,
            center_offset_uv: scope_data.camera_center_offset_uv,
            enabled: scope_data.camera_enabled,
        })
        .unwrap_or_default()
}

#[derive(Script, WidgetRef, WidgetRegister)]
pub struct XrNode {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live(true)]
    visible: bool,
    #[live]
    on_render: ScriptFnRef,
    #[live(vec3(0.0, 0.0, 0.0))]
    pos: Vec3f,
    #[live(vec3(0.0, 0.0, 0.0))]
    rot: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    scale: Vec3f,
    #[live]
    body: XrBodyKind,
    #[live(vec3(0.0, 0.0, 0.0))]
    physics_size: Vec3f,
    #[rust]
    implicit_physics_size: Vec3f,
    #[rust]
    physics_size_explicit: bool,
    #[live(1.0)]
    density: f32,
    #[live(0.8)]
    friction: f32,
    #[live(0.0)]
    restitution: f32,
    #[rust]
    script_async: ScriptAsyncCalls,
    #[rust]
    children: ComponentMap<LiveId, WidgetRef>,
    #[rust]
    child_order: Vec<LiveId>,
}

impl XrNode {
    fn make_render_me(&self, vm: &mut ScriptVm) -> ScriptValue {
        if self.source.is_zero() {
            return NIL;
        }

        let source_obj = self.source.as_object();
        let source_proto = vm.bx.heap.proto(source_obj);
        let proto = if source_proto.as_object().is_some() {
            source_proto
        } else {
            source_obj.into()
        };
        vm.bx.heap.new_with_proto_no_vec(proto).into()
    }

    pub fn local_transform(&self) -> Mat4f {
        compose_scene_node_transform(self.pos, self.rot, self.scale)
    }

    pub fn pos(&self) -> Vec3f {
        self.pos
    }

    pub fn rot(&self) -> Vec3f {
        self.rot
    }

    pub fn scale(&self) -> Vec3f {
        self.scale
    }

    pub fn body_kind(&self) -> XrBodyKind {
        self.body
    }

    pub fn set_implicit_physics_size(&mut self, size: Vec3f) {
        self.implicit_physics_size = vec3f(size.x.max(0.0), size.y.max(0.0), size.z.max(0.0));
    }

    pub fn physics_half_extents(&self) -> Vec3f {
        let physics_size = if self.physics_size_explicit
            || self.physics_size.x > 0.0
            || self.physics_size.y > 0.0
            || self.physics_size.z > 0.0
        {
            self.physics_size
        } else {
            self.implicit_physics_size
        };
        vec3f(
            physics_size.x.max(0.0) * 0.5,
            physics_size.y.max(0.0) * 0.5,
            physics_size.z.max(0.0) * 0.5,
        )
    }

    pub fn density(&self) -> f32 {
        self.density.max(0.0)
    }

    pub fn friction(&self) -> f32 {
        self.friction.max(0.0)
    }

    pub fn restitution(&self) -> f32 {
        self.restitution.max(0.0)
    }

    pub fn child_count(&self) -> usize {
        self.child_order.len()
    }
}

pub fn xr_widget_world_transform(
    cx: &mut Cx3d,
    scope: &mut Scope,
    uid: WidgetUid,
    node: &XrNode,
) -> Mat4f {
    if let Some(runtime_body) = xr_runtime_body_from_scope(scope, uid) {
        Mat4f::mul(
            &runtime_body.pose.to_mat4(),
            &Mat4f::nonuniform_scaled_translation(
                vec3(runtime_body.scale.x, runtime_body.scale.y, runtime_body.scale.z),
                vec3(0.0, 0.0, 0.0),
            ),
        )
    } else {
        let parent_world = cx.scene_world_transform_3d();
        Mat4f::mul(&parent_world, &node.local_transform())
    }
}

impl ScriptHook for XrNode {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_reload() {
            self.child_order.clear();
        }
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        let physics_size_present = value.as_object().is_some_and(|obj| {
            vm.bx.heap
                .value_for_apply(obj.into(), id!(physics_size).into(), &Apply::Eval)
                .is_some()
        });
        if physics_size_present {
            self.physics_size_explicit = true;
        } else if !apply.is_eval() {
            self.physics_size_explicit = false;
        }

        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                self.child_order.clear();
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
                        let can_new = WidgetRef::value_is_newable_widget(vm, kv.value);
                        if !can_new {
                            continue;
                        }
                        self.child_order.push(id);
                        if let Some(child) = self.children.get_mut(&id) {
                            child.script_apply(vm, apply, scope, kv.value);
                        } else {
                            let child = WidgetRef::script_from_value_scoped(vm, scope, kv.value);
                            self.children.insert(id, child);
                        }
                        if let Some(child) = self.children.get(&id) {
                            vm.cx_mut()
                                .widget_tree_insert_child_deep(self.uid, id, child.clone());
                        }
                    }
                });
                self.children.retain(|id, _| self.child_order.contains(id));
            }
        }

        vm.cx_mut().widget_tree_mark_dirty(self.uid);
    }
}

impl WidgetNode for XrNode {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        Area::Empty
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for id in &self.child_order {
            if let Some(child) = self.children.get(id) {
                visit(*id, child.clone());
            }
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        cx.redraw_all();
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            self.redraw(cx);
        }
    }

    fn visible(&self) -> bool {
        self.visible
    }
}

impl Widget for XrNode {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(render) {
            let me = self.make_render_me(vm);
            return vm.with_cx_mut(|cx| {
                cx.widget_to_script_async_call_fwd(
                    self.uid,
                    &mut self.script_async,
                    me,
                    self.source.clone(),
                    self.on_render.clone(),
                    args,
                    id!(render),
                )
            });
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn script_result(&mut self, vm: &mut ScriptVm, id: ScriptAsyncId, result: ScriptValue) {
        let Some(call) = self.script_async.take(id) else {
            return;
        };

        if call.method() == id!(render) && !result.is_err() {
            if let Some(me_obj) = call.me().as_object() {
                self.script_apply(vm, &Apply::Reload, &mut Scope::empty(), me_obj.into());
                vm.cx_mut().redraw_all();
            }
        }
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && event.requires_visibility() {
            return;
        }

        let uid = self.uid;
        for index in 0..self.child_order.len() {
            let id = self.child_order[index];
            let Some(child) = self.children.get(&id).cloned() else {
                continue;
            };
            let child_uid = child.widget_uid();
            cx.group_widget_actions(uid, child_uid, |cx| {
                child.handle_event(cx, event, scope);
            });
        }
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }

        if !self.child_order.iter().any(|id| self.children.contains_key(id)) {
            return DrawStep::done();
        }

        let world_transform = xr_widget_world_transform(cx, scope, self.uid, self);
        let previous_world = cx.set_scene_world_transform_3d(world_transform);

        for index in 0..self.child_order.len() {
            let id = self.child_order[index];
            let Some(child) = self.children.get(&id).cloned() else {
                continue;
            };
            child.draw_3d_all(cx, scope);
        }

        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }

        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
