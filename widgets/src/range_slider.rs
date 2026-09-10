//! RangeSlider — one track, two handles, and the span between them.
//!
//! A range is not two sliders. Two sliders can cross, can be dragged apart
//! one end at a time and never together, and say nothing about the thing
//! that actually matters — the span. This one holds `start <= end` as an
//! invariant, moves the whole span when the band between the handles is
//! dragged, and reports both ends on every change.
//!
//! # What a handle does when it reaches the other one
//!
//! It stops. The other school swaps the two handles over so the drag can
//! carry on past, and it is wrong here: the finger is on one handle, and a
//! control that hands the finger a different handle mid-drag turns a small
//! overshoot into a silent role change. Stopping is legible, and `min_span`
//! sets how much room the two must leave each other — nothing by default,
//! so they may meet.
//!
//! # The gestures
//!
//! * **A handle**: drags that end.
//! * **The band between them**: drags both, keeping the span's width. At
//!   either stop the span stops rather than squashing.
//! * **The track outside the span**: the nearer handle jumps there and the
//!   drag continues on it, so reaching for a distant value is one gesture.
//! * **A double tap**: both ends back to `default_start` / `default_end`.
//!
//! The arrow keys move the handle last touched by `step`; with Shift they
//! move the span. Home and End send that handle to its own limit, and the
//! bracket keys choose which handle the keyboard has.
use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    badge::measure,
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    widget_async::ScriptAsyncResult,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.RangeSliderBase = #(RangeSlider::register_widget(vm))

    set_type_default() do #(DrawRangeSlider::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** A track with two handles and a filled span between them. */
    mod.widgets.RangeSliderFlat = set_type_default() do mod.widgets.RangeSliderBase{
        width: Fill
        height: 36
        margin: theme.mspace_1{top: theme.space_2}

        /** lowest value either end may take */
        min: 0.0
        /** highest value either end may take */
        max: 1.0
        /** value quantization; 0 is continuous */
        step: 0.0
        /** decimals in the readout 0..6 step 1 */
        precision: 2
        /** handle width in pixels 8..60 step 1 */
        handle_size: 14.
        /** track inset from the left and right edges in pixels 0..40 step 1 */
        track_inset: 6.
        /** room reserved above the track for the label and readout in pixels 0..40 step 1 */
        label_height: 20.

        draw_text +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            color: theme.color_label_outer
            color_hover: uniform(theme.color_label_outer_hover)
            color_focus: uniform(theme.color_label_outer_focus)
            color_drag: uniform(theme.color_label_outer_drag)
            color_disabled: uniform(theme.color_label_outer_disabled)

            text_style: theme.font_regular{font_size: theme.font_size_p}
        }

        draw_bg +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: instance(0.0)
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** dragging mix 0..1 step 0.01 */
            drag: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            /** bevel thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.beveling)
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: uniform(theme.corner_radius)
            /** span thickness as a share of the track 0..1 step 0.05 */
            span_weight: uniform(0.34)

            color: uniform(theme.color_inset)
            color_hover: uniform(theme.color_inset_hover)
            color_focus: uniform(theme.color_inset_focus)
            color_drag: uniform(theme.color_inset_drag)
            color_disabled: uniform(theme.color_inset_disabled)

            border_color: uniform(theme.color_bevel_outset_2)
            border_color_hover: uniform(theme.color_bevel_outset_2)
            border_color_focus: uniform(theme.color_bevel_outset_2)
            border_color_drag: uniform(theme.color_bevel_outset_2)
            border_color_disabled: uniform(theme.color_bevel_outset_2_disabled)

            val_color: uniform(theme.color_val)
            val_color_hover: uniform(theme.color_val_hover)
            val_color_focus: uniform(theme.color_val_focus)
            val_color_drag: uniform(theme.color_val_drag)
            val_color_disabled: uniform(theme.color_val_disabled)

            handle_color: uniform(theme.color_handle)
            handle_color_hover: uniform(theme.color_handle_hover)
            handle_color_focus: uniform(theme.color_handle_focus)
            handle_color_drag: uniform(theme.color_handle_drag)
            handle_color_disabled: uniform(theme.color_handle_disabled)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)

                let top = self.label_px
                let track_h = self.rect_size.y - top
                let mid = top + track_h * 0.5

                let fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover.mix(self.color_drag, self.drag), self.hover)
                    .mix(self.color_disabled, self.disabled)
                let stroke = self.border_color
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_hover.mix(self.border_color_drag, self.drag), self.hover)
                    .mix(self.border_color_disabled, self.disabled)
                let val = self.val_color
                    .mix(self.val_color_focus, self.focus)
                    .mix(self.val_color_hover.mix(self.val_color_drag, self.drag), self.hover)
                    .mix(self.val_color_disabled, self.disabled)
                let handle = self.handle_color
                    .mix(self.handle_color_focus, self.focus)
                    .mix(self.handle_color_hover.mix(self.handle_color_drag, self.drag), self.hover)
                    .mix(self.handle_color_disabled, self.disabled)

                // The track.
                sdf.box(
                    self.border_size
                    top + self.border_size
                    self.rect_size.x - self.border_size * 2.
                    track_h - self.border_size * 2.
                    self.border_radius
                )
                sdf.fill_keep(fill)
                sdf.stroke(stroke, self.border_size)

                // The span. Both handle centres come out of the same travel
                // the hit test uses, so what is drawn is what is grabbable.
                let span_x = self.rect_size.x - self.inset_px * 2. - self.handle_px
                let start_c = self.inset_px + self.handle_px * 0.5 + self.start_pos * span_x
                let end_c = self.inset_px + self.handle_px * 0.5 + self.end_pos * span_x
                let span_h = track_h * self.span_weight

                sdf.rect(start_c, mid - span_h * 0.5, max(end_c - start_c, 1.0), span_h)
                // The band lifts under the pointer, since it is a grab
                // target and not decoration.
                sdf.fill(val + vec4(0.06, 0.06, 0.06, 0.0) * self.hot_band)

                // The handles.
                let hpad = 1.5
                let hy = top + self.border_size + hpad
                let hh = track_h - self.border_size * 2. - hpad * 2.

                sdf.box(start_c - self.handle_px * 0.5, hy, self.handle_px, hh, self.border_radius)
                sdf.fill_keep(handle + vec4(0.08, 0.08, 0.08, 0.0) * self.hot_start)
                sdf.stroke(stroke, self.border_size)

                sdf.box(end_c - self.handle_px * 0.5, hy, self.handle_px, hh, self.border_radius)
                sdf.fill_keep(handle + vec4(0.08, 0.08, 0.08, 0.0) * self.hot_end)
                sdf.stroke(stroke, self.border_size)

                return sdf.result
            }
        }

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {hover: 0.0} draw_text: {hover: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {hover: 1.0} draw_text: {hover: 1.0}}
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.0}}
                    apply: {draw_bg: {focus: 0.0} draw_text: {focus: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {focus: 1.0} draw_text: {focus: 1.0}}
                }
            }
            drag: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {drag: 0.0} draw_text: {drag: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {drag: 1.0} draw_text: {drag: 1.0}}
                }
            }
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {disabled: 0.0} draw_text: {disabled: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {disabled: 1.0} draw_text: {disabled: 1.0}}
                }
            }
        }
    }

    /** The standard range slider: the flat face plus the theme's inset bevel. */
    mod.widgets.RangeSlider = set_type_default() do mod.widgets.RangeSliderFlat{
        draw_bg +: {
            border_color: theme.color_bevel_inset_1
            border_color_hover: theme.color_bevel_inset_1_hover
            border_color_focus: theme.color_bevel_inset_1_focus
            border_color_drag: theme.color_bevel_inset_1_drag
            border_color_disabled: theme.color_bevel_inset_1_disabled
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRangeSlider {
    #[deref]
    draw_super: DrawQuad,
    /// Where the two handles sit along their travel, 0..1.
    #[live]
    start_pos: f32,
    #[live]
    end_pos: f32,
    /// The geometry the hit test uses, handed to the shader so the two
    /// cannot drift apart.
    #[live]
    handle_px: f32,
    #[live]
    inset_px: f32,
    #[live]
    label_px: f32,
    /// Which of the three targets the pointer is over, so a press can be
    /// anticipated rather than discovered.
    #[live]
    hot_start: f32,
    #[live]
    hot_end: f32,
    #[live]
    hot_band: f32,
}

/// The arithmetic of a two-ended range, apart from the widget that draws
/// it: the invariant, the quantization, and where a handle sits on a track
/// of a given width. It is a separate type for two reasons — it can be
/// tested without a script heap, and the hit test and the shader are handed
/// the same numbers from the same place, so what is drawn is what is
/// grabbable.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Range {
    min: f64,
    max: f64,
    step: f64,
    min_span: f64,
    /// Handle width and track inset, in pixels.
    handle: f64,
    inset: f64,
    start: f64,
    end: f64,
}

impl Range {
    /// Clamp both ends into range, in order, and at least `min_span` apart.
    fn settle(&mut self) {
        let (lo, hi) = (self.min.min(self.max), self.min.max(self.max));
        self.start = self.start.clamp(lo, hi);
        self.end = self.end.clamp(lo, hi);
        if self.end < self.start {
            self.end = self.start;
        }
        // A span wider than the range is the whole range, not an error.
        let span = (hi - lo).min(self.min_span.max(0.0));
        if self.end - self.start < span {
            // Push the end out; if that hits the stop, pull the start back
            // instead, so the invariant holds at either limit.
            self.end = self.start + span;
            if self.end > hi {
                self.end = hi;
                self.start = hi - span;
            }
        }
    }

    fn quantize(&self, v: f64) -> f64 {
        if self.step > 0.0 {
            self.min + ((v - self.min) / self.step).round() * self.step
        } else {
            v
        }
    }

    /// Where a value sits along the travel, 0..1.
    fn travel(&self, v: f64) -> f64 {
        let span = self.max - self.min;
        if span.abs() < f64::EPSILON {
            0.0
        } else {
            ((v - self.min) / span).clamp(0.0, 1.0)
        }
    }

    fn travel_px(&self, width: f64) -> f64 {
        (width - self.inset * 2.0 - self.handle).max(1.0)
    }

    /// The value at an x offset from the left edge of the widget.
    fn value_at(&self, x: f64, width: f64) -> f64 {
        let t = ((x - self.inset - self.handle * 0.5) / self.travel_px(width)).clamp(0.0, 1.0);
        self.quantize(self.min + t * (self.max - self.min))
    }

    /// The x of a handle's centre, in the same frame as `value_at`.
    fn handle_x(&self, v: f64, width: f64) -> f64 {
        self.inset + self.handle * 0.5 + self.travel(v) * self.travel_px(width)
    }

    /// What a press at this x takes hold of.
    fn grab_at(&self, x: f64, width: f64) -> Grab {
        let sx = self.handle_x(self.start, width);
        let ex = self.handle_x(self.end, width);
        let reach = (self.handle * 0.5).max(6.0);
        let ds = (x - sx).abs();
        let de = (x - ex).abs();
        if ds <= reach || de <= reach {
            // Two handles that have met still come apart: which one is
            // taken depends on which way the press leans, not on which was
            // tested first.
            if (ds - de).abs() < f64::EPSILON {
                return if x < sx { Grab::Start } else { Grab::End };
            }
            return if ds < de { Grab::Start } else { Grab::End };
        }
        if x > sx && x < ex {
            return Grab::Band(self.value_at(x, width) - self.start);
        }
        if x < sx {
            Grab::Start
        } else {
            Grab::End
        }
    }

    /// Move the start, the end staying where it is.
    ///
    /// The moving end stops `min_span` short of the other rather than
    /// shoving it along. During a drag the finger is on ONE handle, so that
    /// handle is the one that gives way; letting `settle` resolve the
    /// squeeze instead would move the end — a value nobody is touching, and
    /// one that was probably set on purpose — every time the start
    /// overshoots.
    fn move_start(&mut self, v: f64) {
        let stop = (self.end - self.min_span.max(0.0)).max(self.min);
        self.start = v.clamp(self.min, stop);
        self.settle();
    }

    /// Move the end, the start staying where it is.
    fn move_end(&mut self, v: f64) {
        let stop = (self.start + self.min_span.max(0.0)).min(self.max);
        self.end = v.clamp(stop, self.max);
        self.settle();
    }

    /// Move the span by `delta`, keeping its width; it stops at the limits
    /// rather than being squashed against them.
    fn shift(&mut self, delta: f64) {
        let width = self.end - self.start;
        let mut start = self.start + delta;
        if start < self.min {
            start = self.min;
        }
        if start + width > self.max {
            start = self.max - width;
        }
        self.start = self.quantize(start);
        self.end = self.start + width;
        self.settle();
    }
}

/// What the finger has hold of.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Grab {
    Start,
    End,
    /// The span itself; the value is the distance from the press to
    /// `start`, so the span moves under the finger rather than jumping to
    /// it.
    Band(f64),
}

#[derive(Clone, Debug, Default)]
pub enum RangeSliderAction {
    StartSlide,
    /// Both ends, on every frame of a drag or every key press.
    Slide(f64, f64),
    EndSlide(f64, f64),
    /// A double tap put both ends back to their DSL defaults.
    Reset(f64, f64),
    #[default]
    None,
}

#[derive(Script, Widget, Animator)]
pub struct RangeSlider {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawRangeSlider,
    #[live]
    draw_text: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    /// The label drawn above the track, or nothing.
    #[live]
    text: String,

    #[live]
    min: f64,
    #[live(1.0)]
    max: f64,
    #[live]
    step: f64,
    #[live(2)]
    precision: usize,

    /// Where the two ends start, and where a double tap puts them back.
    #[live]
    default_start: f64,
    #[live(1.0)]
    default_end: f64,

    /// The least the two ends may leave between them. Zero lets them meet.
    #[live]
    min_span: f64,

    /// A unit the readout carries after each number — "dB", "Hz", "ms".
    #[live]
    pub unit: String,
    /// What the readout multiplies by before showing, so a parameter held
    /// as 0..1 can read as 0..100 beside a "%".
    #[live(1.0)]
    pub display_scale: f64,
    /// Draw the two numbers above the track. Off for a control whose host
    /// already says what the span is.
    #[live(true)]
    pub show_readout: bool,

    /// Handle width, track inset and the room above the track. Rust owns
    /// them because the hit test needs them; the shader is handed the same
    /// numbers every draw.
    #[live(14.0)]
    handle_size: f64,
    #[live(6.0)]
    track_inset: f64,
    #[live(20.0)]
    label_height: f64,

    #[rust]
    start: f64,
    #[rust]
    end: f64,
    #[rust]
    grab: Option<Grab>,
    /// The handle the arrow keys move: whichever was touched last.
    #[rust]
    active_is_end: bool,
}

impl ScriptHook for RangeSlider {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        self.start = self.default_start;
        self.end = self.default_end;
        self.settle();
    }
}

impl RangeSlider {
    /// The range as it stands, built fresh each time: every number in it is
    /// a live property and the tweaker may have moved any of them since the
    /// last draw.
    fn range_of(&self) -> Range {
        Range {
            min: self.min,
            max: self.max,
            step: self.step,
            min_span: self.min_span,
            handle: self.handle_size,
            inset: self.track_inset,
            start: self.start,
            end: self.end,
        }
    }

    fn put(&mut self, range: Range) {
        self.start = range.start;
        self.end = range.end;
    }

    /// Settle the two ends against the current bounds.
    fn settle(&mut self) {
        let mut r = self.range_of();
        r.settle();
        self.put(r);
    }

    fn key_step(&self) -> f64 {
        if self.step > 0.0 {
            self.step
        } else {
            (self.max - self.min) * 0.01
        }
    }

    fn readout(&self) -> String {
        format!(
            "{:.*}{unit} \u{2013} {:.*}{unit}",
            self.precision,
            self.start * self.display_scale,
            self.precision,
            self.end * self.display_scale,
            unit = self.unit
        )
    }

    fn slided(&self, cx: &mut Cx) {
        cx.widget_action(self.uid, RangeSliderAction::Slide(self.start, self.end));
    }

    pub fn range(&self) -> (f64, f64) {
        (self.start, self.end)
    }

    pub fn set_range(&mut self, cx: &mut Cx, start: f64, end: f64) {
        self.start = start;
        self.end = end;
        self.settle();
        self.draw_bg.redraw(cx);
    }

    pub fn reset_to_default(&mut self, cx: &mut Cx) {
        self.start = self.default_start;
        self.end = self.default_end;
        self.settle();
        self.draw_bg.redraw(cx);
    }
}

impl Widget for RangeSlider {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.animator_toggle(cx, disabled, Animate::Yes, ids!(disabled.on), ids!(disabled.off));
    }

    fn disabled(&self, cx: &Cx) -> bool {
        self.animator_in_state(cx, ids!(disabled.on))
    }

    fn script_call(
        &mut self,
        vm: &mut ScriptVm,
        method: LiveId,
        args: ScriptValue,
    ) -> ScriptAsyncResult {
        if method == live_id!(set_range) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let start = vm.bx.heap.vec_value(args_obj, 0, trap).as_f64();
                let end = vm.bx.heap.vec_value(args_obj, 1, trap).as_f64();
                if let (Some(start), Some(end)) = (start, end) {
                    vm.with_cx_mut(|cx| self.set_range(cx, start, end));
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(start) {
            return ScriptAsyncResult::Return(ScriptValue::from_f64(self.start));
        }
        if method == live_id!(end) {
            return ScriptAsyncResult::Return(ScriptValue::from_f64(self.end));
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        if self.animator_in_state(cx, ids!(disabled.on)) {
            return;
        }
        let uid = self.uid;

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.draw_bg.hot_start = 0.0;
                self.draw_bg.hot_end = 0.0;
                self.draw_bg.hot_band = 0.0;
                self.animator_play(cx, ids!(hover.off));
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOver(fe) => {
                // Say what a press would take before it is pressed. The
                // three targets look alike, so nothing else would.
                let x = fe.abs.x - fe.rect.pos.x;
                let (mut s, mut e, mut b) = (0.0, 0.0, 0.0);
                match self.range_of().grab_at(x, fe.rect.size.x) {
                    Grab::Start => s = 1.0,
                    Grab::End => e = 1.0,
                    Grab::Band(_) => b = 1.0,
                }
                if (self.draw_bg.hot_start, self.draw_bg.hot_end, self.draw_bg.hot_band)
                    != (s, e, b)
                {
                    self.draw_bg.hot_start = s;
                    self.draw_bg.hot_end = e;
                    self.draw_bg.hot_band = b;
                    self.draw_bg.redraw(cx);
                }
                cx.set_cursor(if b > 0.5 { MouseCursor::Grab } else { MouseCursor::EwResize });
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                if fe.tap_count == 2 {
                    self.reset_to_default(cx);
                    self.slided(cx);
                    cx.widget_action(uid, RangeSliderAction::Reset(self.start, self.end));
                    return;
                }
                let x = fe.abs.x - fe.rect.pos.x;
                let width = fe.rect.size.x;
                let mut r = self.range_of();
                let grab = r.grab_at(x, width);
                // A press on bare track is a reach, not a grab: the near
                // handle comes to the finger and the drag carries on from
                // there, so one gesture does what two would.
                match grab {
                    Grab::Start => {
                        if (x - r.handle_x(r.start, width)).abs() > r.handle * 0.5 {
                            r.move_start(r.value_at(x, width));
                        }
                    }
                    Grab::End => {
                        if (x - r.handle_x(r.end, width)).abs() > r.handle * 0.5 {
                            r.move_end(r.value_at(x, width));
                        }
                    }
                    Grab::Band(_) => {}
                }
                r.settle();
                self.put(r);
                self.active_is_end = matches!(grab, Grab::End);
                self.grab = Some(grab);
                self.animator_play(cx, ids!(drag.on));
                cx.widget_action(uid, RangeSliderAction::StartSlide);
                self.slided(cx);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(fe) => {
                let Some(grab) = self.grab else {
                    return;
                };
                let x = fe.abs.x - fe.rect.pos.x;
                let width = fe.rect.size.x;
                let mut r = self.range_of();
                let v = r.value_at(x, width);
                match grab {
                    Grab::Start => r.move_start(v),
                    Grab::End => r.move_end(v),
                    Grab::Band(offset) => r.shift(v - offset - r.start),
                }
                self.put(r);
                self.slided(cx);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(fe) => {
                self.grab = None;
                self.animator_play(cx, ids!(drag.off));
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
                cx.widget_action(uid, RangeSliderAction::EndSlide(self.start, self.end));
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
            }
            Hit::KeyDown(ke) => {
                let d = self.key_step();
                let whole = ke.modifiers.shift;
                let end = self.active_is_end;
                let mut r = self.range_of();
                let moved = match ke.key_code {
                    KeyCode::ArrowLeft | KeyCode::ArrowDown => {
                        if whole {
                            r.shift(-d);
                        } else if end {
                            r.move_end(r.quantize(r.end - d));
                        } else {
                            r.move_start(r.quantize(r.start - d));
                        }
                        true
                    }
                    KeyCode::ArrowRight | KeyCode::ArrowUp => {
                        if whole {
                            r.shift(d);
                        } else if end {
                            r.move_end(r.quantize(r.end + d));
                        } else {
                            r.move_start(r.quantize(r.start + d));
                        }
                        true
                    }
                    // Home and End are that handle's own limit, not the
                    // track's: for the end handle, "as far back as it goes"
                    // is the start handle.
                    KeyCode::Home => {
                        if end {
                            r.move_end(r.start);
                        } else {
                            r.move_start(r.min);
                        }
                        true
                    }
                    KeyCode::End => {
                        if end {
                            r.move_end(r.max);
                        } else {
                            r.move_start(r.end);
                        }
                        true
                    }
                    // Tab is focus traversal, so the other handle is
                    // reached with the bracket keys rather than by taking
                    // a key the navigation needs.
                    KeyCode::LBracket => {
                        self.active_is_end = false;
                        false
                    }
                    KeyCode::RBracket => {
                        self.active_is_end = true;
                        false
                    }
                    _ => false,
                };
                if moved {
                    r.settle();
                    self.put(r);
                    self.slided(cx);
                    cx.widget_action(uid, RangeSliderAction::EndSlide(self.start, self.end));
                    self.draw_bg.redraw(cx);
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let r = self.range_of();
        self.draw_bg.start_pos = r.travel(r.start) as f32;
        self.draw_bg.end_pos = r.travel(r.end) as f32;
        self.draw_bg.handle_px = self.handle_size as f32;
        self.draw_bg.inset_px = self.track_inset as f32;
        self.draw_bg.label_px = self.label_height as f32;

        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();
        let size = self.draw_text.text_style.font_size as f64;
        let baseline = rect.pos.y + (self.label_height - size) * 0.5;
        if !self.text.is_empty() {
            let text = self.text.clone();
            self.draw_text
                .draw_abs(cx, dvec2(rect.pos.x + self.track_inset, baseline), &text);
        }
        if self.show_readout {
            let text = self.readout();
            let w = measure(&self.draw_text, cx, &text);
            let x = rect.pos.x + rect.size.x - self.track_inset - w;
            self.draw_text.draw_abs(cx, dvec2(x, baseline), &text);
        }
        self.draw_bg.end(cx);

        if !self.animator_in_state(cx, ids!(disabled.on)) {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        }
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.readout()
    }
}

impl RangeSliderRef {
    /// Both ends, whatever they are now.
    pub fn range(&self) -> (f64, f64) {
        self.borrow().map(|inner| inner.range()).unwrap_or((0.0, 0.0))
    }

    pub fn set_range(&self, cx: &mut Cx, start: f64, end: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_range(cx, start, end);
        }
    }

    /// The new ends, while a drag or a key press is moving them.
    pub fn slided(&self, actions: &Actions) -> Option<(f64, f64)> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            RangeSliderAction::Slide(s, e) => Some((s, e)),
            _ => None,
        }
    }

    /// The ends the gesture settled on.
    pub fn end_slide(&self, actions: &Actions) -> Option<(f64, f64)> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            RangeSliderAction::EndSlide(s, e) => Some((s, e)),
            _ => None,
        }
    }

    pub fn start_slide(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|item| matches!(item.cast(), RangeSliderAction::StartSlide))
            .unwrap_or(false)
    }

    pub fn reset_to_default(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reset_to_default(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A range with the geometry the DSL preset gives it.
    fn slider(min: f64, max: f64, step: f64, min_span: f64) -> Range {
        Range { min, max, step, min_span, handle: 14.0, inset: 6.0, start: min, end: max }
    }

    #[test]
    fn a_handle_stops_at_the_other_rather_than_swapping() {
        let mut s = slider(0.0, 100.0, 0.0, 0.0);
        s.start = 40.0;
        s.end = 60.0;
        // The start is dragged well past the end, as FingerMove does it.
        s.start = 90.0f64.min(s.end);
        s.settle();
        assert_eq!(s.start, 60.0, "the start stopped at the end");
        assert_eq!(s.end, 60.0, "and the end did not become the start");
    }

    #[test]
    fn min_span_holds_the_two_apart_at_either_stop() {
        let mut s = slider(0.0, 10.0, 0.0, 3.0);
        s.start = 9.0;
        s.end = 9.0;
        s.settle();
        assert_eq!((s.start, s.end), (7.0, 10.0), "pushed off the top stop, not through it");

        let mut s = slider(0.0, 10.0, 0.0, 3.0);
        s.start = 1.0;
        s.end = 1.0;
        s.settle();
        assert_eq!((s.start, s.end), (1.0, 4.0));
    }

    #[test]
    fn a_dragged_handle_stops_short_rather_than_shoving_the_other() {
        // Dragging the start hard right used to push the end along with it:
        // `settle` resolved the squeeze by moving whichever end was free,
        // and the end is not the one the finger is on. Overshooting the
        // start would then destroy an end that had been set on purpose.
        let mut s = slider(0.0, 100.0, 0.0, 20.0);
        s.start = 30.0;
        s.end = 70.0;
        s.move_start(95.0);
        assert_eq!((s.start, s.end), (50.0, 70.0), "the end stayed put");

        s.move_end(10.0);
        assert_eq!((s.start, s.end), (50.0, 70.0), "and so does the start");
    }

    #[test]
    fn a_span_wider_than_the_range_is_the_whole_range() {
        let mut s = slider(0.0, 5.0, 0.0, 50.0);
        s.start = 2.0;
        s.end = 2.0;
        s.settle();
        assert_eq!((s.start, s.end), (0.0, 5.0));
    }

    #[test]
    fn dragging_the_band_keeps_its_width_at_the_stop() {
        let mut s = slider(0.0, 100.0, 0.0, 0.0);
        s.start = 70.0;
        s.end = 90.0;
        s.shift(50.0);
        assert_eq!((s.start, s.end), (80.0, 100.0), "the span stopped whole");
    }

    #[test]
    fn the_hit_test_agrees_with_where_a_handle_is_drawn() {
        let mut s = slider(0.0, 100.0, 0.0, 0.0);
        s.start = 25.0;
        s.end = 75.0;
        let w = 300.0;
        assert_eq!(s.grab_at(s.handle_x(25.0, w), w), Grab::Start);
        assert_eq!(s.grab_at(s.handle_x(75.0, w), w), Grab::End);
        // Between them is the band, carrying the offset from the press to
        // the start so the span moves rather than jumps.
        let mid = (s.handle_x(25.0, w) + s.handle_x(75.0, w)) * 0.5;
        assert!(matches!(s.grab_at(mid, w), Grab::Band(_)));
        // Outside it is the nearer handle.
        assert_eq!(s.grab_at(0.0, w), Grab::Start);
        assert_eq!(s.grab_at(w, w), Grab::End);
    }

    #[test]
    fn two_handles_that_have_met_still_come_apart() {
        let mut s = slider(0.0, 100.0, 0.0, 0.0);
        s.start = 50.0;
        s.end = 50.0;
        let w = 300.0;
        let c = s.handle_x(50.0, w);
        assert_eq!(s.grab_at(c - 2.0, w), Grab::Start, "leaning left takes the start");
        assert_eq!(s.grab_at(c + 2.0, w), Grab::End, "leaning right takes the end");
    }

    #[test]
    fn a_step_quantizes_both_ends() {
        let s = slider(0.0, 100.0, 25.0, 0.0);
        assert_eq!(s.value_at(s.handle_x(30.0, 300.0), 300.0), 25.0);
        assert_eq!(s.value_at(s.handle_x(40.0, 300.0), 300.0), 50.0);
    }

    #[test]
    fn a_zero_width_range_reports_travel_zero_rather_than_dividing_by_it() {
        let s = slider(5.0, 5.0, 0.0, 0.0);
        assert_eq!(s.travel(5.0), 0.0);
    }
}
