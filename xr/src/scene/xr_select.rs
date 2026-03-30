use super::xr_node::{xr_widget_world_transform, XrNode};
use crate::prelude::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.XrSelectBase = #(XrSelect::register_widget(vm))
    mod.widgets.XrSelect = mod.widgets.XrSelectBase{}
}

#[derive(Clone, Debug, Default)]
pub enum XrSelectAction {
    ActiveChildChanged(LiveId),
    #[default]
    None,
}

#[derive(Script, WidgetRef, WidgetRegister)]
pub struct XrSelect {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    active_child: LiveId,
    #[deref]
    node: XrNode,
    #[rust]
    children: ComponentMap<LiveId, WidgetRef>,
    #[rust]
    child_order: Vec<LiveId>,
}

impl XrSelect {
    pub fn node(&self) -> &XrNode {
        &self.node
    }

    fn first_child_id(&self) -> Option<LiveId> {
        self.child_order
            .iter()
            .copied()
            .find(|id| self.children.contains_key(id))
    }

    fn sync_child_visibility(&self, cx: &mut Cx) {
        for id in &self.child_order {
            if let Some(child) = self.children.get(id) {
                child.set_visible(cx, *id == self.active_child);
            }
        }
    }

    fn ensure_active_child(&mut self, cx: &mut Cx) -> Option<LiveId> {
        if self.children.contains_key(&self.active_child) {
            return Some(self.active_child);
        }
        let active_child = self.first_child_id()?;
        self.active_child = active_child;
        self.sync_child_visibility(cx);
        Some(active_child)
    }

    fn render_child(&self, vm: &mut ScriptVm, child_id: LiveId) {
        let Some(child) = self.children.get(&child_id).cloned() else {
            return;
        };
        let _ = child.script_call(vm, live_id!(render), NIL);
    }

    pub fn set_active_child(&mut self, cx: &mut Cx, child_id: LiveId) -> Option<WidgetRef> {
        let child = self.children.get(&child_id)?.clone();
        if self.active_child != child_id {
            self.active_child = child_id;
            self.sync_child_visibility(cx);
            cx.with_vm(|vm| self.render_child(vm, child_id));
            cx.widget_action(self.uid, XrSelectAction::ActiveChildChanged(child_id));
            self.redraw(cx);
        } else {
            child.set_visible(cx, true);
        }
        Some(child)
    }

    fn active_child_widget(&mut self, cx: &mut Cx) -> Option<(LiveId, WidgetRef)> {
        let active_child = self.ensure_active_child(cx)?;
        self.children
            .get(&active_child)
            .cloned()
            .map(|child| (active_child, child))
    }

    pub fn active_child_widget_ref(&self) -> Option<WidgetRef> {
        self.children.get(&self.active_child).cloned()
    }
}

impl ScriptHook for XrSelect {
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

        vm.with_cx_mut(|cx| {
            let _ = self.ensure_active_child(cx);
            self.sync_child_visibility(cx);
            cx.widget_tree_mark_dirty(self.uid);
        });
    }
}

impl WidgetNode for XrSelect {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn cast_inner_any(&self, type_id: std::any::TypeId) -> Option<&dyn std::any::Any> {
        if type_id == std::any::TypeId::of::<XrNode>() {
            Some(&self.node)
        } else {
            None
        }
    }

    fn cast_inner_any_mut(&mut self, type_id: std::any::TypeId) -> Option<&mut dyn std::any::Any> {
        if type_id == std::any::TypeId::of::<XrNode>() {
            Some(&mut self.node)
        } else {
            None
        }
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

    fn visible(&self) -> bool {
        self.node.visible()
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        self.node.set_visible(cx, visible);
    }
}

impl Widget for XrSelect {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(render) {
            let active_child = self.active_child;
            self.render_child(vm, active_child);
            return ScriptAsyncResult::Return(NIL);
        }

        if method == live_id!(select) {
            let Some(child_id) = args.as_id() else {
                return ScriptAsyncResult::MethodNotFound;
            };
            vm.with_cx_mut(|cx| {
                let _ = self.set_active_child(cx, child_id);
            });
            return ScriptAsyncResult::Return(NIL);
        }

        if method == live_id!(active_child) {
            return ScriptAsyncResult::Return(ScriptValue::from_id(self.active_child));
        }

        if self.children.contains_key(&method) {
            vm.with_cx_mut(|cx| {
                let _ = self.set_active_child(cx, method);
            });
            return ScriptAsyncResult::Return(NIL);
        }

        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.node.visible() && event.requires_visibility() {
            return;
        }

        let Some((_, child)) = self.active_child_widget(cx) else {
            return;
        };
        let uid = self.uid;
        let child_uid = child.widget_uid();
        cx.group_widget_actions(uid, child_uid, |cx| child.handle_event(cx, event, scope));
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        if !self.node.visible() {
            return DrawStep::done();
        }

        let Some((_, child)) = self.active_child_widget(cx.cx) else {
            return DrawStep::done();
        };

        let world_transform = xr_widget_world_transform(cx, scope, self.uid, &self.node);
        let previous_world = cx.set_scene_world_transform_3d(world_transform);
        child.draw_3d_all(cx, scope);
        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }

        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}

impl XrSelectRef {
    pub fn set_active_child(&self, cx: &mut Cx, child_id: LiveId) -> Option<WidgetRef> {
        let mut inner = self.borrow_mut()?;
        inner.set_active_child(cx, child_id)
    }
}
