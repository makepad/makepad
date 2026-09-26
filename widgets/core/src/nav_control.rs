use crate::makepad_draw::*;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.NavControlBase = #(NavControl::script_component(vm))
    mod.widgets.NavControl = set_type_default() do mod.widgets.NavControlBase{
        draw_focus +: {
            pixel: fn() {
                return #000f
            }
        }
        draw_text +: {
            text_style +: {
                font_size: 6
            }
            color: theme.color_label_inner
        }
    }

}

#[derive(Script, ScriptHook)]
pub struct NavControl {
    #[live]
    draw_list: DrawList2d,
    #[live]
    draw_focus: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[rust]
    _recent_focus: Area,
}

impl NavControl {
    pub fn send_trigger_to_scroll_stack(cx: &mut Cx, stack: Vec<Area>) {
        let mut prev_area = None;
        for next_area in stack {
            if let Some(prev_area) = prev_area {
                cx.send_trigger(
                    prev_area,
                    Trigger {
                        id: live_id!(scroll_focus_nav),
                        from: next_area,
                    },
                );
            }
            prev_area = Some(next_area);
        }
    }

    /// Tab and Shift+Tab move the key focus through the window's tab stops,
    /// wrapping at either end. With nothing focused, Tab starts at the first
    /// stop (Shift+Tab at the last). An open modal keeps them inside itself.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, root: DrawListId) {
        let Event::KeyDown(ke) = event else { return };
        if ke.key_code != KeyCode::Tab || ke.modifiers.control || ke.modifiers.logo || ke.modifiers.alt {
            return;
        }
        let stops = CxDraw::nav_scope_stops(cx, root);
        if stops.is_empty() {
            return;
        }
        let n = stops.len();
        let current = if cx.key_focus().is_empty() {
            None
        } else {
            stops.iter().position(|area| cx.has_key_focus(*area))
        };
        let next = match (current, ke.modifiers.shift) {
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
            (None, false) => 0,
            (None, true) => n - 1,
        };
        let area = stops[next];
        if let Some((_, scroll_stack)) =
            CxDraw::iterate_nav_stops(cx, root, |_, stop| (stop.area == area).then_some(area))
        {
            Self::send_trigger_to_scroll_stack(cx, scroll_stack);
        }
        cx.set_key_focus(area);
    }

    pub fn draw(&mut self, cx: &mut Cx2d) {
        if !self.draw_list.begin(cx, Walk::default()).is_redrawing() {
            return;
        }

        self.draw_list.end(cx);
    }
}
