use crate::{
    makepad_derive_widget::*, makepad_draw::*, makepad_script::ScriptFnRef, widget::*,
    widget_async::CxWidgetToScriptCallExt, widget_tree::CxWidgetExt,
};
use crate::makepad_platform::studio::{AppToStudio, TweakHitsResponse};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.RootBase = #(Root::register_widget(vm))

    mod.widgets.Root = set_type_default() do mod.widgets.RootBase{
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
    #[live]
    on_startup: ScriptFnRef,
    #[rust]
    started: bool,
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
        vm.cx_mut().widget_tree_mark_dirty(self.uid);
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

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, component) in self.components.iter() {
            visit(*id, component.clone());
        }
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        Walk::default()
    }
}

impl Widget for Root {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Startup = event {
            if !self.started {
                self.started = true;
                let uid = self.uid;
                cx.widget_to_script_call(
                    uid,
                    NIL,
                    self.source.clone(),
                    self.on_startup.clone(),
                    &[],
                );
            }
        }
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

        for (_id, component) in self.components.iter_mut() {
            component.handle_event(cx, event, scope);
        }

        if let Event::TweakRay(e) = event {
            let hit_uids = e.hit_widget_uids.borrow().clone();
            let mut resolved_rect = None;
            let mut resolved_uid = None;
            for uid in hit_uids.iter().copied().rev() {
                let widget = cx.widget_tree().widget(WidgetUid(uid));
                if widget.is_empty() {
                    continue;
                }
                let area = widget.area();
                if area.is_valid(cx) {
                    let rect = area.rect(cx);
                    if rect.contains(e.abs) {
                        resolved_rect = Some(rect);
                        resolved_uid = Some(uid);
                        break;
                    }
                }
            }
            let dpi_factor = e.dpi_factor.max(1.0);
            let hit_rect = resolved_rect.or(e.hit_rect.get());
            let (left, top, width, height) = if let Some(rect) = hit_rect {
                (
                    rect.pos.x * dpi_factor,
                    rect.pos.y * dpi_factor,
                    rect.size.x * dpi_factor,
                    rect.size.y * dpi_factor,
                )
            } else {
                (0.0, 0.0, 0.0, 0.0)
            };
            Cx::send_studio_message(AppToStudio::TweakHits(TweakHitsResponse {
                window_id: e.window_id.id(),
                dpi_factor,
                ray_x: e.abs.x * dpi_factor,
                ray_y: e.abs.y * dpi_factor,
                left,
                top,
                width,
                height,
                widget_uids: resolved_uid.into_iter().collect(),
            }));
        }
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        for (_id, component) in self.components.iter() {
            component.draw_3d_all(cx, scope);
        }
        DrawStep::done()
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        self.draw_state.begin(cx, DrawState::Component(0));

        while let Some(DrawState::Component(step)) = self.draw_state.get() {
            if let Some((id, component)) = self.components.iter_mut().nth(step) {
                let _id = *id;
                let walk = component.walk(cx);
                component.draw_walk(cx, scope, walk)?;
                self.draw_state.set(DrawState::Component(step + 1));
            } else {
                self.draw_state.end();
            }
        }
        DrawStep::done()
    }
}
