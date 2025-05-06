use {
    crate::{
        touch_gesture::*,
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
        view::*,
    }
};

live_design! {
    ExpandablePanelBase = {{ExpandablePanel}} {}
}

#[derive(Clone, DefaultNone, Debug)]
pub enum ExpandablePanelAction {
    ScrolledAt(f64),
    None,
}

#[derive(Live, Widget)]
pub struct ExpandablePanel {
    #[deref] view: View,
    #[rust] touch_gesture: Option<TouchGesture>,
    #[live] initial_offset: f64,
}

impl LiveHook for ExpandablePanel {
    fn after_apply_from(&mut self, cx: &mut Cx, apply: &mut Apply) {
        if apply.from.is_from_doc() {
            self.apply_over(cx, live! {
                panel = { margin: { top: (self.initial_offset) }}
            });
        }
    }
}

impl Widget for ExpandablePanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);

        if let Some(touch_gesture) = self.touch_gesture.as_mut() {
            if touch_gesture.handle_event(cx, event, self.view.area()).has_changed() {
                let scrolled_at = touch_gesture.scrolled_at;
                let panel_margin = self.initial_offset - scrolled_at;
                self.apply_over(cx, live! {
                    panel = { margin: { top: (panel_margin) }}
                });
                self.redraw(cx);

                cx.widget_action(
                    self.widget_uid(),
                    &scope.path,
                    ExpandablePanelAction::ScrolledAt(scrolled_at),
                );
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let result = self.view.draw_walk(cx, scope, walk);

        if self.touch_gesture.is_none() {
            let mut touch_gesture = TouchGesture::new();
            touch_gesture.set_mode(ScrollMode::Swipe);

            // Limit the amount of dragging allowed for the panel
            let panel_height = self.view(id!(panel)).area().rect(cx).size.y;
            touch_gesture.set_range(0.0, panel_height - self.initial_offset);

            touch_gesture.reset_scrolled_at();
            self.touch_gesture = Some(touch_gesture);
        }

        result
    }
}