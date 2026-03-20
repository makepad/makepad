use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    makepad_script::ScriptFnRef,
    widget::*,
    widget_async::{CxWidgetToScriptCallExt, ScriptAsyncCalls, ScriptAsyncId, ScriptAsyncResult},
    widget_tree::CxWidgetExt,
};

use super::scene_3d::{compose_scene_node_transform, scene_node_world_transform_from_scope};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.Node3DBase = #(Node3D::register_widget(vm))
    mod.widgets.Node3D = set_type_default() do mod.widgets.Node3DBase{}
}

#[derive(Script, WidgetRef, WidgetRegister)]
pub struct Node3D {
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
    position: Vec3f,
    #[live(vec3(0.0, 0.0, 0.0))]
    rotation: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    scale: Vec3f,
    #[rust]
    script_async: ScriptAsyncCalls,
    #[rust]
    children: ComponentMap<LiveId, WidgetRef>,
    #[rust]
    child_order: Vec<LiveId>,
    #[rust]
    debug_logged_empty_draw: bool,
    #[rust]
    debug_logged_first_draw: bool,
}

impl Node3D {
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
}

impl ScriptHook for Node3D {
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
                        if !WidgetRef::value_is_newable_widget(vm, kv.value) {
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

impl WidgetNode for Node3D {
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

impl Widget for Node3D {
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
                log!(
                    "node3d render applied children={}",
                    self.child_order.len()
                );
                vm.cx_mut().redraw_all();
            }
        }
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && event.requires_visibility() {
            return;
        }

        let uid = self.uid;
        let child_order = self.child_order.clone();
        for id in child_order {
            if let Some(child) = self.children.get_mut(&id) {
                let child_uid = child.widget_uid();
                cx.group_widget_actions(uid, child_uid, |cx| {
                    child.handle_event(cx, event, scope);
                });
            }
        }
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }

        let child_refs: Vec<WidgetRef> = self
            .child_order
            .iter()
            .filter_map(|id| self.children.get(id).cloned())
            .collect();
        if child_refs.is_empty() {
            if !self.debug_logged_empty_draw {
                self.debug_logged_empty_draw = true;
                log!("node3d draw skipped: no children");
            }
            return DrawStep::done();
        }
        if !self.debug_logged_first_draw {
            self.debug_logged_first_draw = true;
            log!("node3d draw children={}", child_refs.len());
        }

        let parent_world = scene_node_world_transform_from_scope(scope);
        let local_transform =
            compose_scene_node_transform(self.position, self.rotation, self.scale);
        let world_transform = Mat4f::mul(&parent_world, &local_transform);

        let previous_world = if let Some(scene_scope) = scope.data.get_mut::<super::scene_3d::SceneScope3D>() {
            let previous_world = scene_scope.world_transform;
            scene_scope.world_transform = world_transform;
            Some(previous_world)
        } else {
            None
        };

        for child in child_refs {
            child.draw_3d_all(cx, scope);
        }

        if let Some(previous_world) = previous_world {
            if let Some(scene_scope) = scope.data.get_mut::<super::scene_3d::SceneScope3D>() {
                scene_scope.world_transform = previous_world;
            }
        }
        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
