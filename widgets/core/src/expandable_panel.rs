//! ExpandablePanel - a panel over a background, dragged up and down by hand.
//!
//! The panel is a child named `panel`, declared by whoever uses the widget and
//! declared LAST: this is an overlay, and the last child is the one on top. It
//! is deliberately not declared here as an empty slot to be filled - see the
//! note in the preset for what that costs.
//!
//! `initial_offset` is how far down the panel rests, and it is the whole of
//! the travel: the panel may be pulled up until it meets the top of the area
//! and pushed back down to where it started, and no further either way.
//!
//! What it deliberately does not do is decide anything about the number it
//! reports. It does not snap, settle, spring to a detent or fade what is
//! behind it. A host that wants any of that reads `scrolled_at` and does it.
use crate::{makepad_derive_widget::*, makepad_draw::*, touch_gesture::*, view::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ExpandablePanelBase = #(ExpandablePanel::register_widget(vm))

    mod.widgets.ExpandablePanel = mod.widgets.ExpandablePanelBase{
        width: Fill
        height: Fill
        flow: Overlay

        // No `panel :=` here, on purpose. A child declared on a preset is
        // copied into every instance ahead of that instance's own children,
        // so a panel slot held open here takes the FIRST place in the overlay
        // and is painted over by the background it is meant to sit above - in
        // whatever order the instance writes its own children, since it can
        // only ever write them after this one. The panel is the instance's to
        // declare, and it declares it last.
    }
}

/// Where the panel rests and how far a drag may carry it, apart from the
/// widget that draws it. It is a separate type for two reasons - it can be
/// tested without a drawing pass, and the limits handed to the gesture and the
/// margin handed to the layout come out of the same place, so the panel cannot
/// be dragged somewhere it is not allowed to sit.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Travel {
    /// How far below the top of the panel area the panel rests.
    offset: f64,
}

impl Travel {
    fn new(offset: f64) -> Self {
        // A panel resting above the top of its own area has no travel to give,
        // and a reversed range is not a harmless one: the gesture clamps into
        // whatever range it is given, and clamping into a reversed one panics.
        Self {
            offset: if offset.is_finite() { offset.max(0.0) } else { 0.0 },
        }
    }

    /// What the drag is allowed: nothing below the resting place, and up as
    /// far as the top of the panel area. Past the top the panel would be
    /// pulling its own contents out of sight, which is not what this is for.
    fn range(&self) -> (f64, f64) {
        (0.0, self.offset)
    }

    /// The panel's top margin once a drag has carried it `scrolled_at`. It
    /// sits a little outside `range` while the gesture's rubber band is
    /// stretched, which springs back on its own.
    fn margin(&self, scrolled_at: f64) -> f64 {
        self.offset - scrolled_at
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
    /// How far down the panel rests, and how far up a drag may take it.
    #[live]
    initial_offset: f64,
    #[rust]
    current_panel_margin: f64,
}

impl ExpandablePanel {
    /// The travel as it stands, built fresh each time: `initial_offset` is a
    /// live property and the tweaker may have moved it since the last draw.
    fn travel(&self) -> Travel {
        Travel::new(self.initial_offset)
    }
}

impl Widget for ExpandablePanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);

        let travel = self.travel();
        if let Some(touch_gesture) = self.touch_gesture.as_mut() {
            if touch_gesture
                .handle_event(cx, event, self.view.area())
                .has_changed()
            {
                let scrolled_at = touch_gesture.scrolled_at;
                self.current_panel_margin = travel.margin(scrolled_at);
                self.redraw(cx);

                cx.widget_action(
                    self.widget_uid(),
                    ExpandablePanelAction::ScrolledAt(scrolled_at),
                );
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The travel comes out of `initial_offset` alone, so the gesture can
        // be given its limits before anything has been drawn. The limits used
        // to be taken from the panel's measured height on the frame the
        // gesture was made - the one frame the panel had not been offset yet,
        // so the measurement was the height of the whole area, and the drag
        // could carry the panel clean off the top of it.
        let travel = self.travel();
        let gesture = self.touch_gesture.get_or_insert_with(|| {
            let mut gesture = TouchGesture::new();
            gesture.set_mode(ScrollMode::Swipe);
            gesture.reset_scrolled_at();
            gesture
        });
        let (min, max) = travel.range();
        gesture.set_range(min, max);
        // Where the panel sits is settled before the draw rather than after
        // it. The resting margin used to be set at the end of the first
        // draw_walk with nothing asking for another one, so the first frame
        // drew the panel at the top of the area however far down it rests.
        self.current_panel_margin = travel.margin(gesture.scrolled_at);

        let panel_ref = self.view(cx, ids!(panel));
        if let Some(mut panel) = panel_ref.borrow_mut() {
            panel.walk.margin.top = self.current_panel_margin;
        }

        self.view.draw_walk(cx, scope, walk)
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
            // The next draw settles this anyway; setting it here keeps
            // `get_current_offset` truthful in the same breath as the reset.
            let rest = inner.travel().margin(0.0);
            inner.current_panel_margin = rest;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_rests_as_far_down_as_the_offset_says() {
        let travel = Travel::new(120.0);
        assert_eq!(travel.margin(0.0), 120.0);
    }

    #[test]
    fn the_travel_ends_at_the_top_of_the_area() {
        // The old limit was the panel's measured height less the offset, and
        // the measurement was taken on the one frame the panel had not been
        // offset yet: in a 260 high area that is 140, and 140 of travel from a
        // rest of 120 leaves the panel 20 above the top of the area.
        let travel = Travel::new(120.0);
        let (min, max) = travel.range();
        assert_eq!(max, 120.0, "the travel is the offset, not a measurement");
        assert_eq!(travel.margin(max), 0.0, "pulled right up, the panel meets the top");
        assert_eq!(travel.margin(min), 120.0, "pushed right down, its resting place");
    }

    #[test]
    fn a_range_never_comes_out_reversed() {
        // TouchGesture::set_range clamps into the range it is handed, and a
        // clamp whose low end is above its high end panics.
        for offset in [0.0, 1.0, 120.0, -50.0, f64::NAN, f64::INFINITY] {
            let (min, max) = Travel::new(offset).range();
            assert!(min <= max, "offset {offset} gave the range {min}..{max}");
        }
    }

    #[test]
    fn an_offset_of_nothing_leaves_nothing_to_drag() {
        let travel = Travel::new(0.0);
        assert_eq!(travel.range(), (0.0, 0.0));
        assert_eq!(travel.margin(0.0), 0.0);
    }
}
