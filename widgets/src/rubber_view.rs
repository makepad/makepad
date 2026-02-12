use crate::{makepad_derive_widget::*, makepad_draw::*, view::View, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.RubberViewBase = #(RubberView::register_widget(vm))

    mod.widgets.RubberView = set_type_default() do mod.widgets.RubberViewBase{
        width: Fill
        height: Fit
        smoothing: 0.5
    }
}

#[derive(Clone)]
enum DrawState {
    Begin,
    Drawing,
}

/// A View wrapper that smoothly animates its size when child content changes.
/// When the child size is static, the animation stops (no more nextframe/redraw calls).
#[derive(Script, ScriptHook, Widget)]
pub struct RubberView {
    #[deref]
    view: View,

    #[walk]
    walk: Walk,

    #[layout]
    layout: Layout,

    /// Smoothing factor (0-1, higher = faster animation)
    #[live(0.5)]
    pub smoothing: f32,

    /// Current animated height
    #[rust]
    animated_height: f64,

    /// Target height (actual content height)
    #[rust]
    target_height: f64,

    /// NextFrame for driving animation
    #[rust]
    next_frame: NextFrame,

    /// Last frame time for dt calculation
    #[rust]
    last_time: f64,

    #[rust]
    draw_state: DrawStateWrap<DrawState>,

    #[rust]
    area: Area,
}

impl Widget for RubberView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, DrawState::Begin) {
            // Begin our outer turtle - use Fit height so we can measure
            cx.begin_turtle(walk, self.layout);
            self.draw_state.set(DrawState::Drawing);
        }

        if let Some(DrawState::Drawing) = self.draw_state.get() {
            // Draw the inner view with Fit height
            let inner_walk = Walk {
                width: Size::fill(),
                height: Size::fit(),
                ..Walk::default()
            };
            self.view.draw_walk(cx, scope, inner_walk)?;

            // Get the actual content height
            let used = cx.turtle().used();
            let content_height = used.y;

            if content_height > 0.0 {
                if self.animated_height == 0.0 {
                    // First draw - snap to content height
                    self.animated_height = content_height;
                    self.target_height = content_height;
                } else if (self.target_height - content_height).abs() > 0.5 {
                    // Content height changed - update target and start animation
                    self.target_height = content_height;
                    self.next_frame = cx.new_next_frame();
                }
            }

            // Override used height to report animated height instead of actual
            // This makes the container grow smoothly while content is fully drawn
            if self.animated_height > 0.0 && self.animated_height < content_height {
                cx.turtle_mut().set_used(used.x, self.animated_height);
            }

            cx.end_turtle_with_area(&mut self.area);
            self.draw_state.end();
        }

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Handle animation NextFrame
        if let Some(ev) = self.next_frame.is_event(event) {
            let time = ev.time;
            let dt = if self.last_time > 0.0 {
                (time - self.last_time).max(0.001)
            } else {
                1.0 / 60.0
            };
            self.last_time = time;

            let distance = self.target_height - self.animated_height;

            if distance.abs() > 0.5 {
                // Exponential smoothing
                let frame_rate_adjust = (dt * 60.0).min(1.0);
                let factor = 1.0 - (1.0 - self.smoothing as f64).powf(frame_rate_adjust);

                self.animated_height += distance * factor;

                // Request redraw and continue animation
                self.view.redraw(cx);
                self.next_frame = cx.new_next_frame();
            } else {
                // Snap to target and stop animating
                self.animated_height = self.target_height;
            }
        }

        self.view.handle_event(cx, event, scope);
    }
}

impl RubberView {
    /// Reset the animation state (for reused widgets)
    pub fn reset_animation(&mut self) {
        self.animated_height = 0.0;
        self.target_height = 0.0;
        self.last_time = 0.0;
    }
}

impl RubberViewRef {
    pub fn reset_animation(&self) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reset_animation();
        }
    }
}
