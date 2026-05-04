use crate::{
    makepad_derive_widget::*, makepad_draw::event::Ease, makepad_draw::*, view::*, widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.View

    mod.widgets.KeyboardViewBase = #(KeyboardView::register_widget(vm))
    mod.widgets.KeyboardView = set_type_default() do mod.widgets.KeyboardViewBase{
        width: Fill height: Fill
        keyboard_min_shift: 30.
    }
}

const KEYBOARD_SHIFT_EPSILON: f64 = 0.5;
const KEYBOARD_RECONCILE_DURATION: f64 = 0.10;
const KEYBOARD_RECONCILE_EASE: Ease = Ease::OutCubic;

#[derive(Script, ScriptHook, Widget)]
pub struct KeyboardView {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[redraw]
    #[rust]
    area: Area,
    #[live]
    outer_layout: Layout,
    #[live]
    outer_walk: Walk,
    #[live]
    keyboard_walk: Walk,
    /// Minimum gap (in logical pixels) preserved between the focused IME field's
    /// bottom edge and the top of the on-screen keyboard. Acts as breathing room
    /// so the cursor isn't pressed flush against the keyboard.
    #[live]
    keyboard_min_shift: f64,
    #[rust]
    next_frame: NextFrame,

    /// Current vertical scroll offset applied to the inner content (logical pixels).
    #[rust]
    keyboard_shift: f64,
    /// Last known on-screen keyboard height in logical pixels. Stored so the
    /// shift can be recomputed when the focused IME area moves due to a layout
    /// reflow that happens while the keyboard stays open.
    #[rust]
    keyboard_height: f64,
    #[rust(AnimState::Closed)]
    anim_state: AnimState,
    #[rust]
    draw_state: DrawStateWrap<Walk>,
}

#[derive(Clone, Copy)]
enum AnimState {
    Closed,
    Opening {
        duration: f64,
        start_time: f64,
        ease: Ease,
        from_shift: f64,
        to_shift: f64,
    },
    Open,
    Closing {
        duration: f64,
        start_time: f64,
        ease: Ease,
        from_shift: f64,
    },
}

impl KeyboardView {
    /// Compute the vertical scroll required to keep the focused IME field above
    /// an on-screen keyboard of `keyboard_height` logical pixels.
    ///
    /// The keyboard is a window-level obstruction occupying the bottom strip of
    /// the active window, so the calculation is anchored to the window's inner
    /// size, not to `self.area`. That keeps the result correct when this widget
    /// is nested under padding/scrollers, when the platform has already shrunk
    /// the surface to make room for the IME, or when there is no settled rect
    /// yet for `self.area`.
    fn compute_target_shift(&self, keyboard_height: f64, cx: &Cx) -> f64 {
        if keyboard_height <= 0.0 {
            return 0.0;
        }
        let ime_rect = cx.get_ime_area_rect();
        // Without a registered IME area there is no field to keep visible.
        if ime_rect.size.y <= 0.0 {
            return 0.0;
        }
        let window_inner_size = cx.windows[CxWindowPool::id_zero()].window_geom.inner_size;
        if window_inner_size.y <= 0.0 {
            return 0.0;
        }
        // `ime_rect` comes from `Area::rect`, which reads the GPU-side draw
        // position, i.e. the IME rect's y AFTER our own `with_scroll` has
        // been applied. We need its NATURAL (unshifted) y to compute an
        // absolute target shift. Without re-adding the current shift the
        // formula oscillates: each applied shift moves the IME up, the next
        // call sees it unobstructed and returns 0, the shift snaps back to
        // 0, the IME returns to its original position, the next call sees
        // it obstructed again, ... and we ping-pong forever, redrawing on
        // every frame.
        let keyboard_height = keyboard_height.min(window_inner_size.y).max(0.0);
        let keyboard_top = window_inner_size.y - keyboard_height;
        let ime_natural_bottom = ime_rect.pos.y + ime_rect.size.y + self.keyboard_shift;
        let needed = ime_natural_bottom + self.keyboard_min_shift - keyboard_top;
        needed
            .max(0.0)
            .min(keyboard_height)
    }

    fn set_keyboard_shift(&mut self, cx: &mut Cx, shift: f64) {
        self.keyboard_shift = shift.max(0.0);
        cx.keyboard_shift = self.keyboard_shift;
    }

    fn animate_to_shift(
        &mut self,
        cx: &mut Cx,
        start_time: f64,
        target_shift: f64,
        duration: f64,
        ease: Ease,
    ) {
        let target_shift = target_shift.max(0.0).min(self.keyboard_height.max(0.0));
        if duration <= 0.0 || (target_shift - self.keyboard_shift).abs() <= KEYBOARD_SHIFT_EPSILON {
            self.set_keyboard_shift(cx, target_shift);
            self.anim_state = AnimState::Open;
        } else {
            self.anim_state = AnimState::Opening {
                duration,
                start_time,
                ease,
                from_shift: self.keyboard_shift,
                to_shift: target_shift,
            };
            self.next_frame = cx.new_next_frame();
        }
        self.redraw(cx);
    }

    fn animate_to_zero(&mut self, cx: &mut Cx, start_time: f64, duration: f64, ease: Ease) {
        if duration <= 0.0 || self.keyboard_shift <= KEYBOARD_SHIFT_EPSILON {
            self.set_keyboard_shift(cx, 0.0);
            self.anim_state = AnimState::Closed;
            self.keyboard_height = 0.0;
        } else {
            self.anim_state = AnimState::Closing {
                from_shift: self.keyboard_shift,
                duration,
                start_time,
                ease,
            };
            self.next_frame = cx.new_next_frame();
        }
        self.redraw(cx);
    }

    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(
            walk,
            self.outer_layout
                .with_scroll(dvec2(0., self.keyboard_shift)),
        );
    }

    fn end(&mut self, cx: &mut Cx2d) {
        cx.end_turtle_with_area(&mut self.area);
    }
}

impl Widget for KeyboardView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Some(e) = self.next_frame.is_event(event) {
            match self.anim_state {
                AnimState::Opening {
                    duration,
                    start_time,
                    ease,
                    from_shift,
                    to_shift,
                } => {
                    let dt = e.time - start_time;
                    if dt < duration && duration > 0.0 {
                        let t = ease.map(dt / duration);
                        self.keyboard_shift = from_shift + (to_shift - from_shift) * t;
                        cx.keyboard_shift = self.keyboard_shift;
                        self.next_frame = cx.new_next_frame();
                    } else {
                        self.keyboard_shift = to_shift;
                        cx.keyboard_shift = self.keyboard_shift;
                        self.anim_state = AnimState::Open;
                    }
                    self.redraw(cx);
                }
                AnimState::Closing {
                    duration,
                    start_time,
                    ease,
                    from_shift,
                } => {
                    let dt = e.time - start_time;
                    if dt < duration && duration > 0.0 {
                        let t = ease.map(dt / duration);
                        self.keyboard_shift = from_shift * (1.0 - t);
                        cx.keyboard_shift = self.keyboard_shift;
                        self.next_frame = cx.new_next_frame();
                    } else {
                        self.keyboard_shift = 0.0;
                        cx.keyboard_shift = self.keyboard_shift;
                        self.anim_state = AnimState::Closed;
                        self.keyboard_height = 0.0;
                    }
                    self.redraw(cx);
                }
                _ => (),
            }
        }
        if let Event::VirtualKeyboard(vk) = event {
            match vk {
                VirtualKeyboardEvent::WillShow {
                    time,
                    height,
                    ease,
                    duration,
                } => {
                    // "Animate to a keyboard of `height` pts." Used for both
                    // initial show and any mid-flight height change: keyboard
                    // language switch, predictive bar toggle, or rotation while
                    // the keyboard is up. The platform layer can re-fire
                    // WillShow with the new height instead of needing a separate
                    // event variant.
                    self.keyboard_height = *height;
                    let target = self.compute_target_shift(*height, cx);
                    self.animate_to_shift(cx, *time, target, *duration, *ease);
                }
                VirtualKeyboardEvent::WillHide {
                    time,
                    height: _,
                    ease,
                    duration,
                } => {
                    self.animate_to_zero(cx, *time, *duration, *ease);
                }
                VirtualKeyboardEvent::DidShow { time, height } => {
                    if *height <= 0.0 {
                        self.keyboard_height = 0.0;
                        self.animate_to_shift(
                            cx,
                            *time,
                            0.0,
                            KEYBOARD_RECONCILE_DURATION,
                            KEYBOARD_RECONCILE_EASE,
                        );
                        self.view.handle_event(cx, event, scope);
                        return;
                    }
                    // Android reports per-frame IME insets via this settled
                    // event path. Keep `keyboard_shift` paired with the IME
                    // area from the last draw; post-draw reconciliation below
                    // computes the pan after the focused input registers its
                    // current rect.
                    self.keyboard_height = *height;
                    self.anim_state = AnimState::Open;
                    self.redraw(cx);
                }
                VirtualKeyboardEvent::DidHide { time } => {
                    self.animate_to_zero(
                        cx,
                        *time,
                        KEYBOARD_RECONCILE_DURATION,
                        KEYBOARD_RECONCILE_EASE,
                    );
                }
            }
        }
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self
            .draw_state
            .begin_with(cx, &(), |cx, _| self.view.walk(cx))
        {
            self.begin(cx, walk);
        }
        if let Some(walk) = self.draw_state.get() {
            self.view.draw_walk(cx, scope, walk)?;
        }
        self.end(cx);

        // Post-children reconciliation. This is the SOLE place the shift
        // is computed during steady-state. We deliberately do NOT do a
        // pre-draw reconcile, because at that point the IME area's
        // `redraw_id` stored from the previous frame no longer matches
        // the current draw list's `redraw_id` (the redraw we scheduled
        // last frame just incremented it), so `Area::rect` returns
        // `Rect::default()` and the formula collapses to 0. A pre-draw
        // recompute against that stale rect would either skip the update
        // (if shift was already 0) or worse, *reset* a settled non-zero
        // shift to 0 each frame, oscillating forever.
        //
        // Sequence after `DidShow` arrives without a preceding useful
        // `WillShow` target (Android, or an iOS frame change where the IME
        // area was stale):
        //   Frame N: handler sets state=Open, height=H, schedules redraw.
        //   Frame N draw: shift is still 0; children draw at natural
        //     positions; the focused TextInput registers its IME area
        //     with the *current* redraw_id, which makes `ime_rect` valid
        //     here at post-draw. We compute the correct target and animate
        //     toward it instead of snapping.
        //
        // Cost: one frame (~16 ms) of unshifted content on first show.
        // Acceptable, and matches what users tolerate elsewhere.
        if matches!(self.anim_state, AnimState::Open) && self.keyboard_height > 0.0 {
            let new_target = self.compute_target_shift(self.keyboard_height, cx);
            if (new_target - self.keyboard_shift).abs() > KEYBOARD_SHIFT_EPSILON {
                let time = cx.time();
                self.animate_to_shift(
                    cx,
                    time,
                    new_target,
                    KEYBOARD_RECONCILE_DURATION,
                    KEYBOARD_RECONCILE_EASE,
                );
            }
        }
        DrawStep::done()
    }
}
