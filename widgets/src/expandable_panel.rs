use crate::{makepad_derive_widget::*, makepad_draw::*, touch_gesture::*, view::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ExpandablePanelBase = #(ExpandablePanel::register_widget(vm))

    mod.widgets.ExpandablePanel = mod.widgets.ExpandablePanelBase{
        width: Fill
        height: Fill
        flow: Overlay

        panel := View{
            width: Fill
            height: Fill
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum ExpandablePanelAction {
    ScrolledAt(f64),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ExpandablePanel {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    touch_gesture: Option<TouchGesture>,
    #[live]
    initial_offset: f64,
    #[rust]
    current_panel_margin: f64,
}

impl Widget for ExpandablePanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);

        if let Some(touch_gesture) = self.touch_gesture.as_mut() {
            if touch_gesture
                .handle_event(cx, event, self.view.area())
                .has_changed()
            {
                let scrolled_at = touch_gesture.scrolled_at;
                self.current_panel_margin = self.initial_offset - scrolled_at;
                self.redraw(cx);

                cx.widget_action(
                    self.widget_uid(),
                    ExpandablePanelAction::ScrolledAt(scrolled_at),
                );
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Apply the current panel margin before drawing
        let panel_ref = self.view(cx, ids!(panel));
        if let Some(mut panel) = panel_ref.borrow_mut() {
            panel.walk.margin.top = self.current_panel_margin;
        }

        let result = self.view.draw_walk(cx, scope, walk);

        if self.touch_gesture.is_none() {
            let mut touch_gesture = TouchGesture::new();
            touch_gesture.set_mode(ScrollMode::Swipe);

            // Limit the amount of dragging allowed for the panel
            let panel_height = self.view(cx, ids!(panel)).area().rect(cx).size.y;
            touch_gesture.set_range(0.0, panel_height - self.initial_offset);

            touch_gesture.reset_scrolled_at();
            self.current_panel_margin = self.initial_offset;
            self.touch_gesture = Some(touch_gesture);
        }

        result
    }
}

impl ExpandablePanelRef {
    pub fn scrolled_at(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let ExpandablePanelAction::ScrolledAt(value) = item.cast() {
                return Some(value);
            }
        }
        None
    }

    pub fn reset(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            if let Some(touch_gesture) = inner.touch_gesture.as_mut() {
                touch_gesture.stop();
            }
            inner.current_panel_margin = inner.initial_offset;
            inner.redraw(cx);
        }
    }

    pub fn get_current_offset(&self) -> f64 {
        if let Some(inner) = self.borrow() {
            inner.current_panel_margin
        } else {
            0.0
        }
    }
}
