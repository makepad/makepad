use crate::prelude::XrSharedHand;
use makepad_widgets::{
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_script::ScriptFnRef,
    widget::*,
    widget_async::{CxWidgetToScriptCallExt, ScriptAsyncCalls, ScriptAsyncId, ScriptAsyncResult},
    widget_tree::CxWidgetExt,
};
use std::{cmp::Ordering, collections::HashMap, rc::Rc};

use crate::util::scene_draw::compose_scene_node_transform;

script_mod! {
    use mod.prelude.widgets_internal.*

    let XrBodyKind = set_type_default() do #(XrBodyKind::script_api(vm))
    mod.widgets.XrBodyKind = XrBodyKind

    let XrPhysicsShape = set_type_default() do #(XrPhysicsShape::script_api(vm))
    mod.widgets.XrPhysicsShape = XrPhysicsShape

    let XrRenderClass = set_type_default() do #(XrRenderClass::script_api(vm))
    mod.widgets.XrRenderClass = XrRenderClass

    let XrSharedObjectPolicy = set_type_default() do #(XrSharedObjectPolicy::script_api(vm))
    mod.widgets.XrSharedObjectPolicy = XrSharedObjectPolicy

    mod.widgets.XrNodeBase = #(XrNode::register_widget(vm))
    mod.widgets.XrNode = set_type_default() do mod.widgets.XrNodeBase{
        body: XrBodyKind.Disabled
        physics_shape: XrPhysicsShape.Box
        render_class: XrRenderClass.Opaque
        shared_object_policy: XrSharedObjectPolicy.None
        spawn_pool: false
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Script, ScriptHook)]
pub enum XrPhysicsShape {
    #[pick]
    Box,
    Sphere,
}

impl Default for XrPhysicsShape {
    fn default() -> Self {
        Self::Box
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Script, ScriptHook)]
pub enum XrRenderClass {
    #[pick]
    Opaque,
    Transparent,
}

impl Default for XrRenderClass {
    fn default() -> Self {
        Self::Opaque
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Script, ScriptHook)]
pub enum XrSharedObjectPolicy {
    None,
    #[pick]
    BootstrapShared,
    OnDemandShared,
    PooledOnDemand,
}

impl Default for XrSharedObjectPolicy {
    fn default() -> Self {
        Self::None
    }
}

pub const XR_HAND_INFLUENCE_POINTS_PER_HAND: usize = 6;
pub const XR_HAND_INFLUENCE_POINT_COUNT: usize = XR_HAND_INFLUENCE_POINTS_PER_HAND * 2;

#[derive(Clone, Copy, Debug, Default)]
pub struct XrHandInfluencePoint {
    pub pos: Vec3f,
    pub gain_scale: f32,
    pub radius_scale: f32,
}

#[derive(Clone)]
pub struct XrRuntimeBodyState {
    pub pose: Pose,
    pub scale: Vec3f,
    pub linvel: Vec3f,
    pub angvel: Vec3f,
    pub sleeping: bool,
    pub held_by: Option<XrSharedHand>,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum XrNodeAction {
    SceneChanged,
    #[default]
    None,
}

#[derive(Clone, Default)]
pub struct XrDrawScopeData {
    pub runtime_bodies: Rc<HashMap<WidgetUid, XrRuntimeBodyState>>,
    pub tracking_from_content: Mat4f,
    pub content_from_tracking: Mat4f,
    pub env_texture: Option<Texture>,
    pub camera_texture: Option<Texture>,
    pub camera_source_size: Vec2f,
    pub camera_rotation_steps: f32,
    pub camera_center_offset_uv: Vec2f,
    pub camera_enabled: bool,
    pub hand_influence_points: [Option<XrHandInfluencePoint>; XR_HAND_INFLUENCE_POINT_COUNT],
}

#[derive(Clone, Default)]
pub struct XrPassthroughScopeData {
    pub camera_texture: Option<Texture>,
    pub source_size: Vec2f,
    pub rotation_steps: f32,
    pub center_offset_uv: Vec2f,
    pub enabled: bool,
}

#[derive(Clone, Default)]
pub struct XrDrawContext {
    scope_data: XrDrawScopeData,
}

impl XrDrawContext {
    pub fn from_scope(scope: &mut Scope) -> Self {
        Self {
            scope_data: scope
                .data
                .get::<XrDrawScopeData>()
                .cloned()
                .unwrap_or_default(),
        }
    }

    pub fn runtime_body(&self, uid: WidgetUid) -> Option<XrRuntimeBodyState> {
        self.scope_data.runtime_bodies.get(&uid).cloned()
    }

    pub fn tracking_from_content(&self) -> Mat4f {
        self.scope_data.tracking_from_content
    }

    pub fn content_from_tracking(&self) -> Mat4f {
        self.scope_data.content_from_tracking
    }

    pub fn hand_influence_points(
        &self,
    ) -> [Option<XrHandInfluencePoint>; XR_HAND_INFLUENCE_POINT_COUNT] {
        self.scope_data.hand_influence_points
    }

    pub fn env_texture(&self) -> Option<Texture> {
        self.scope_data.env_texture.clone()
    }

    pub fn passthrough(&self) -> XrPassthroughScopeData {
        XrPassthroughScopeData {
            camera_texture: self.scope_data.camera_texture.clone(),
            source_size: self.scope_data.camera_source_size,
            rotation_steps: self.scope_data.camera_rotation_steps,
            center_offset_uv: self.scope_data.camera_center_offset_uv,
            enabled: self.scope_data.camera_enabled,
        }
    }
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
    #[live]
    physics_shape: XrPhysicsShape,
    #[live]
    render_class: XrRenderClass,
    #[live]
    shared_object_policy: XrSharedObjectPolicy,
    #[live(false)]
    spawn_pool: bool,
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
    #[new]
    draw_list: DrawList,
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

    pub fn physics_shape(&self) -> XrPhysicsShape {
        self.physics_shape
    }

    pub fn render_class(&self) -> XrRenderClass {
        self.render_class
    }

    pub fn shared_object_policy(&self) -> XrSharedObjectPolicy {
        self.shared_object_policy
    }

    pub fn bootstrap_shared(&self) -> bool {
        matches!(
            self.shared_object_policy,
            XrSharedObjectPolicy::BootstrapShared
        )
    }

    pub fn is_transparent(&self) -> bool {
        matches!(self.render_class, XrRenderClass::Transparent)
    }

    pub fn spawn_pool(&self) -> bool {
        self.spawn_pool
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

    fn child_world_sort_center(child: &WidgetRef) -> Option<Vec3f> {
        xr_widget_local_sort_center(child)
    }

    fn child_is_transparent(child: &WidgetRef) -> bool {
        xr_widget_is_transparent(child)
    }

    fn transform_point(transform: &Mat4f, point: Vec3f) -> Vec3f {
        transform
            .transform_vec4(vec4f(point.x, point.y, point.z, 1.0))
            .to_vec3f()
    }
}

pub fn xr_widget_with_scene_node<R>(
    widget: &WidgetRef,
    visit: impl FnOnce(&XrNode) -> R,
) -> Option<R> {
    if let Some(node) = widget.borrow::<XrNode>() {
        return Some(visit(&node));
    }
    if let Some(node) = widget.cast_inner::<XrNode>() {
        return Some(visit(&node));
    }
    None
}

pub fn xr_widget_children(widget: &WidgetRef, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
    widget.children(visit);
}

pub fn xr_widget_local_sort_center(widget: &WidgetRef) -> Option<Vec3f> {
    xr_widget_with_scene_node(widget, |node| node.pos())
}

pub fn xr_widget_is_transparent(widget: &WidgetRef) -> bool {
    xr_widget_with_scene_node(widget, |node| node.is_transparent()).unwrap_or(false)
}

pub fn xr_draw_list_depth(scene_state: &SceneState3D, world_pos: Vec3f) -> f32 {
    let view_pos =
        scene_state
            .view
            .transform_vec4(vec4f(world_pos.x, world_pos.y, world_pos.z, 1.0));
    if view_pos.w.abs() > 1.0e-6 {
        view_pos.z / view_pos.w
    } else {
        view_pos.z
    }
}

pub fn xr_sort_child_draw_order(draw_order_entries: &mut [(usize, f32, bool)]) {
    if draw_order_entries.len() <= 1 {
        return;
    }

    draw_order_entries.sort_by(|a, b| match (a.2, b.2) {
        (false, true) => Ordering::Less,
        (true, false) => Ordering::Greater,
        (false, false) => {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        }
        (true, true) => {
            a.1.partial_cmp(&b.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        }
    });
}

pub fn xr_widget_world_transform(
    cx: &mut Cx3d,
    scope: &mut Scope,
    uid: WidgetUid,
    node: &XrNode,
) -> Mat4f {
    let draw_context = XrDrawContext::from_scope(scope);
    if let Some(runtime_body) = draw_context.runtime_body(uid) {
        Mat4f::mul(
            &runtime_body.pose.to_mat4(),
            &Mat4f::nonuniform_scaled_translation(
                vec3(
                    runtime_body.scale.x,
                    runtime_body.scale.y,
                    runtime_body.scale.z,
                ),
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
            vm.bx
                .heap
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
                vm.cx_mut()
                    .widget_action(self.uid, XrNodeAction::SceneChanged);
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

        if !self
            .child_order
            .iter()
            .any(|id| self.children.contains_key(id))
        {
            return DrawStep::done();
        }

        let scene_state = match cx.scene_state_3d() {
            Some(scene_state) => scene_state,
            None => return DrawStep::done(),
        };
        let world_transform = xr_widget_world_transform(cx, scope, self.uid, self);
        self.draw_list.set_reset_zbias(cx.cx, true);
        self.draw_list.begin_always(cx);
        let previous_world = cx.set_scene_world_transform_3d(world_transform);
        let mut draw_order_entries = Vec::new();

        for index in 0..self.child_order.len() {
            let id = self.child_order[index];
            let Some(child) = self.children.get(&id).cloned() else {
                continue;
            };
            if let Some(child_center) = Self::child_world_sort_center(&child) {
                let child_center = Self::transform_point(&world_transform, child_center);
                draw_order_entries.push((
                    index,
                    xr_draw_list_depth(&scene_state, child_center),
                    Self::child_is_transparent(&child),
                ));
            } else {
                draw_order_entries.push((index, 0.0, false));
            }
        }
        xr_sort_child_draw_order(&mut draw_order_entries);

        for (index, _, _) in draw_order_entries {
            let id = self.child_order[index];
            let Some(child) = self.children.get(&id).cloned() else {
                continue;
            };
            child.draw_3d_all(cx, scope);
        }

        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }
        self.draw_list.end(cx);

        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
