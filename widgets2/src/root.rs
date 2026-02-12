use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*, widget_tree::CxWidgetExt};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.RootBase = #(Root::register_widget(vm))

    mod.widgets.Root = set_type_default() do mod.widgets.RootBase{
        // Designer window commented out for now
        // design_window = Designer{}
    }
}

#[derive(Script, WidgetRef, WidgetRegister)]
pub struct Root {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[rust]
    area: Area,
    #[rust]
    components: ComponentMap<LiveId, WidgetRef>,
    #[new]
    xr_draw_list: DrawList,
    #[live]
    xr_pass: ScriptDrawPass,
    #[rust]
    draw_state: DrawStateWrap<DrawState>,
}

#[derive(Clone)]
enum DrawState {
    Component(usize),
}

impl ScriptHook for Root {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Handle children from the object's vec
        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    let id = kv.key.as_id().unwrap_or(LiveId(0));
                    let cx = vm.cx_mut();
                    // Only open design window in makepad studio
                    if id == live_id!(design_window) && !cx.in_makepad_studio() {
                        continue;
                    }
                    // Only show xr_hands if XR mode is available
                    if id == live_id!(xr_hands) && !cx.os_type().has_xr_mode() {
                        continue;
                    }
                    // Get or create widget
                    if let Some(widget) = self.components.get_mut(&id) {
                        widget.script_apply(vm, apply, scope, kv.value);
                    } else {
                        let widget = WidgetRef::script_from_value_scoped(vm, scope, kv.value);
                        self.components.insert(id, widget);
                    }
                }
            });
        }
    }
}

impl WidgetNode for Root {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn redraw(&mut self, cx: &mut Cx) {
        for component in self.components.values_mut() {
            component.redraw(cx);
        }
    }

    fn area(&self) -> Area {
        self.area
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        Walk::default()
    }
}

impl Widget for Root {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Draw(e) = event {
            if cx.in_xr_mode() {
                if !e.xr_state.is_some() {
                    return;
                }
                let mut cx_draw = CxDraw::new(cx, e);
                let cx = &mut Cx3d::new(&mut cx_draw);
                // lets begin a 3D drawlist in the global context
                self.xr_pass.handle.set_as_xr_pass(cx);
                cx.begin_pass(&self.xr_pass.handle, Some(4.0));
                self.xr_draw_list.begin_always(cx);
                self.draw_3d_all(cx, scope);
                self.xr_draw_list.end(cx);
                cx.end_pass(&self.xr_pass.handle);
                return;
            } else {
                let mut cx_draw = CxDraw::new(cx, e);
                let cx = &mut Cx2d::new(&mut cx_draw);
                self.draw_all(cx, scope);
                return;
            }
        }

        for (id, component) in self.components.iter_mut() {
            cx.with_node(component.widget_uid(), *id, component.clone(), |cx| {
                component.handle_event(cx, event, scope);
            });
        }
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        for (id, component) in self.components.iter() {
            cx.with_node(component.widget_uid(), *id, component.clone(), |cx| {
                component.draw_3d_all(cx, scope);
            });
        }
        DrawStep::done()
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        self.draw_state.begin(cx, DrawState::Component(0));

        while let Some(DrawState::Component(step)) = self.draw_state.get() {
            if let Some((id, component)) = self.components.iter_mut().nth(step) {
                let id = *id;
                let walk = component.walk(cx);
                cx.with_node(component.widget_uid(), id, component.clone(), |cx| {
                    component.draw_walk(cx, scope, walk)
                })?;
                self.draw_state.set(DrawState::Component(step + 1));
            } else {
                self.draw_state.end();
            }
        }
        DrawStep::done()
    }
}
