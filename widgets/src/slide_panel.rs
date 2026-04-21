use crate::{
    animator::{Animator, AnimatorAction, AnimatorImpl, Play},
    makepad_derive_widget::*,
    makepad_draw::*,
    view::*,
    widget::*,
    widget_async::ScriptAsyncResult,
    WidgetMatchEvent, WindowAction,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SlideSide = #(SlideSide::script_api(vm))
    mod.widgets.splat(mod.widgets.SlideSide)

    mod.widgets.SlidePanelBase = #(SlidePanel::register_widget(vm))

    mod.widgets.SlidePanel = set_type_default() do mod.widgets.SlidePanelBase{
        animator: Animator{
            active: {
                default: @off
                on: AnimatorState{
                    redraw: true
                    from: {all: Forward {duration: 0.5}}
                    ease: InQuad
                    apply: {
                        active: 0.0
                    }
                }
                off: AnimatorState{
                    redraw: true
                    from: {all: Forward {duration: 0.5}}
                    ease: OutQuad
                    apply: {
                        active: 1.0
                    }
                }
            }
        }
    }
}

#[derive(Copy, Clone, Debug, Script, ScriptHook, Default)]
pub enum SlideSide {
    #[pick]
    #[default]
    Left,
    Right,
    Top,
}

#[derive(Script, ScriptHook, Widget, Animator)]
pub struct SlidePanel {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    frame: View,
    #[apply_default]
    animator: Animator,
    #[live]
    active: f64,
    #[live]
    side: SlideSide,
}

#[derive(Clone, Debug, Default)]
pub enum SlidePanelAction {
    #[default]
    None,
}

impl Widget for SlidePanel {
    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        _args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(open) {
            vm.with_cx_mut(|cx| self.open(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(close) {
            vm.with_cx_mut(|cx| self.close(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(toggle) {
            vm.with_cx_mut(|cx| self.toggle(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(is_open) {
            let is_open = vm.with_cx(|cx| self.is_open(cx));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(is_open));
        }
        if method == live_id!(is_animating) {
            let is_animating = self.animator.is_track_animating(id!(active));
            return ScriptAsyncResult::Return(ScriptValue::from_bool(is_animating));
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.frame.handle_event(cx, event, scope);
        self.widget_match_event(cx, event, scope);

        if self.animator_handle_event(cx, event).must_redraw() {
            self.frame.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let panel_rect = cx.peek_walk_turtle(walk);
        let parent_rect = cx.turtle().rect();
        let abs_pos = match self.side {
            SlideSide::Top => dvec2(
                parent_rect.pos.x,
                parent_rect.pos.y - panel_rect.size.y * self.active,
            ),
            SlideSide::Left => dvec2(
                parent_rect.pos.x - panel_rect.size.x * self.active,
                parent_rect.pos.y,
            ),
            SlideSide::Right => dvec2(
                parent_rect.pos.x + parent_rect.size.x - panel_rect.size.x
                    + panel_rect.size.x * self.active,
                parent_rect.pos.y,
            ),
        };

        self.frame.draw_walk(cx, scope, walk.with_abs_pos(abs_pos))
    }
}

impl WidgetMatchEvent for SlidePanel {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        for action in actions {
            if let WindowAction::WindowGeomChange(_ce) = action.as_widget_action().cast() {
                self.redraw(cx);
            }
        }
    }
}

impl SlidePanel {
    pub fn open(&mut self, cx: &mut Cx) {
        self.animator_play(cx, ids!(active.on));
        self.frame.redraw(cx);
    }

    pub fn close(&mut self, cx: &mut Cx) {
        self.animator_play(cx, ids!(active.off));
        self.frame.redraw(cx);
    }

    pub fn toggle(&mut self, cx: &mut Cx) {
        if self.animator_in_state(cx, ids!(active.on)) {
            self.close(cx);
        } else {
            self.open(cx);
        }
    }

    pub fn is_open(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(active.on))
    }

    pub fn redraw(&mut self, cx: &mut Cx) {
        self.frame.redraw(cx);
    }
}

impl SlidePanelRef {
    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }

    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open(cx);
        }
    }

    pub fn toggle(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.toggle(cx);
        }
    }

    pub fn is_open(&self, cx: &Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.is_open(cx)
        } else {
            false
        }
    }

    pub fn is_animating(&self, _cx: &mut Cx) -> bool {
        if let Some(inner) = self.borrow() {
            inner.animator.is_track_animating(id!(active))
        } else {
            false
        }
    }
}

impl SlidePanelSet {
    pub fn close(&self, cx: &mut Cx) {
        for item in self.iter() {
            item.close(cx);
        }
    }

    pub fn open(&self, cx: &mut Cx) {
        for item in self.iter() {
            item.open(cx);
        }
    }
}
