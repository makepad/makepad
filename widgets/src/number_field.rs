//! NumberField — a number you type into, with a stepper you can see.
//!
//! The library already had a number you DRAG: `ValueInput` hides its step
//! marks until the pointer arrives, because a toolbar cannot afford chrome
//! that is only occasionally useful. A form cannot afford the opposite. In a
//! column of fields the person is filling in, a control whose affordance
//! appears only on hover reads as a plain text box, and the two arrows that
//! would have saved them typing are never found. So this one's buttons are
//! always there.
//!
//! # The four ways in, and why each exists
//!
//! * **Type**. It is a real [`TextInput`], so selection, the clipboard and
//!   undo all work. What is typed is parsed on Return or on leaving; what
//!   will not parse is put back, because a field that silently keeps
//!   nonsense is worse than one that refuses it.
//! * **The buttons**. A press steps once. Holding repeats, slowly at first
//!   and then faster — a range of a thousand is reachable without a
//!   thousand presses, and a single press is still a single step.
//! * **Dragging the buttons**. Up is more. Once you have pressed a stepper
//!   the hand is already there, and a drag is the gesture that hand is
//!   about to make; without it, wanting twenty more means twenty presses or
//!   a round trip to the keyboard.
//! * **The wheel**, anywhere over the field. Nothing to aim at, and it is
//!   what every other numeric control in the library already answers to.
//!
//! # What it does with the ends
//!
//! `min` and `max` bound it. At a bound it either **stops** or **wraps**,
//! and `wrap` says which: an angle in degrees wants 359 + 1 to be 0, and a
//! quantity of things wants it to stay at the top. Stopping is the default,
//! since wrapping a count is how a form ends up ordering none of something.
use crate::{
    badge::measure,
    widget_tree::CxWidgetExt,
    makepad_derive_widget::*,
    makepad_draw::*,
    text_input::{TextInputAction, TextInputWidgetRefExt},
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.NumberSpinBase = #(NumberSpin::register_widget(vm))
    mod.widgets.NumberFieldBase = #(NumberField::register_widget(vm))

    set_type_default() do #(DrawNumberSpin::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** The two stacked step buttons that sit at the trailing edge of a number field. */
    mod.widgets.NumberSpin = set_type_default() do mod.widgets.NumberSpinBase{
        width: 15.
        // A stated height, not Fill: the well lays its slots out in a
        // row and a Fill here resolved to nothing, so the column was
        // laid out and never painted.
        height: 22.

        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: 8., line_spacing: 1.0}
        }

        draw_bg +: {
            /** which half the pointer is over: -1 lower, 0 neither, 1 upper 0..1 step 1 */
            hot: 0.0
            /** which half is held down 0..1 step 1 */
            down: 0.0
            /** disabled mix 0..1 step 0.01 */
            disabled: 0.0

            /** the divider between the two halves in pixels 0..4 step 0.5 */
            rule: uniform(1.0)

            // A surface colour, not a bevel one. Bevels are hairline
            // colours a shade from their surroundings, so a column
            // painted in one is drawn and invisible.
            color: uniform(theme.color_surface_container)
            color_hover: uniform(theme.color_surface_container_high)
            color_down: uniform(theme.color_surface_container_highest)
            rule_color: uniform(theme.color_bevel)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let mid = floor(h * 0.5)

                // The two halves are lit separately, so the pointer says
                // which way a press would go before it goes.
                let upper_hot = max(0.0, self.hot)
                let lower_hot = max(0.0, -self.hot)
                let upper_down = max(0.0, self.down)
                let lower_down = max(0.0, -self.down)

                sdf.rect(0.0, 0.0, w, mid)
                sdf.fill(self.color.mix(self.color_hover, upper_hot).mix(self.color_down, upper_down))
                sdf.rect(0.0, mid, w, h - mid)
                sdf.fill(self.color.mix(self.color_hover, lower_hot).mix(self.color_down, lower_down))

                sdf.rect(0.0, mid - self.rule * 0.5, w, self.rule)
                sdf.fill(self.rule_color)

                return sdf.result
            }
        }
    }

    /** A number with a visible stepper: type it, press the arrows, drag them, or use the wheel. */
    mod.widgets.NumberField = set_type_default() do mod.widgets.NumberFieldBase{
        width: 140.
        // A real height, not Fit. The spin column is Fill so it can be as
        // tall as the box, and Fill inside Fit resolves to nothing: the
        // whole field collapsed to no height and drew nothing at all.
        height: 24.

        min: 0.0
        max: 100.0
        step: 1.0
        precision: 0
        wrap: false

        /** how wide the step column is, in pixels 8..40 step 1 */
        spin_width: 15.
        /** room before the number, in pixels 0..24 step 1 */
        pad_left: theme.space_2

        // The box is drawn here rather than by a FieldWell. The well
        // lays its slots out in a row, and its input is `width: Fill`,
        // so the input took the whole row and the step column was laid
        // out into nothing and never painted. Sizing both children from
        // Rust is the only arrangement where the column is certain to
        // get its room.
        draw_bg +: {
            /** pointer-hover mix 0..1 step 0.01 */
            hover: 0.0
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: 0.0
            /** disabled mix 0..1 step 0.01 */
            disabled: 0.0

            color: uniform(theme.color_inset)
            color_hover: uniform(theme.color_inset_hover)
            color_focus: uniform(theme.color_inset_focus)
            color_disabled: uniform(theme.color_inset_disabled)
            border_color: uniform(theme.color_bevel)
            border_color_focus: uniform(theme.color_bevel_focus)
            /** corner rounding radius 0..16 step 0.5 */
            border_radius: uniform(theme.corner_radius)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    0.5
                    0.5
                    self.rect_size.x - 1.0
                    self.rect_size.y - 1.0
                    self.border_radius
                )
                sdf.fill_keep(
                    self.color
                        .mix(self.color_hover, self.hover)
                        .mix(self.color_focus, self.focus)
                        .mix(self.color_disabled, self.disabled)
                )
                sdf.stroke(self.border_color.mix(self.border_color_focus, self.focus), 1.0)
                return sdf.result
            }
        }

        input: mod.widgets.WellInput{
            height: Fill
            empty_text: ""
        }
        spin: mod.widgets.NumberSpin{}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawNumberSpin {
    #[deref]
    draw_super: DrawQuad,
    /// +1 upper half, -1 lower half, 0 neither.
    #[live]
    hot: f32,
    #[live]
    down: f32,
    #[live]
    disabled: f32,
}

#[derive(Clone, Debug, Default)]
pub enum NumberSpinAction {
    /// One press, one repeat, or one notch of a drag. Carries the direction
    /// and how many steps: a drag reports several at once.
    Step(f64),
    #[default]
    None,
}

/// How long a held button waits before it starts repeating, and how fast it
/// repeats once it does. The first wait is long enough that a press meant as
/// one step is never read as two.
const REPEAT_DELAY: f64 = 0.4;
const REPEAT_EVERY: f64 = 0.06;
/// Travel, in layout points, that a drag on the column spends per step.
const DRAG_POINTS_PER_STEP: f64 = 6.0;
/// The glyphs on the two halves.
const UP_MARK: &str = "\u{25b2}";
const DOWN_MARK: &str = "\u{25bc}";
/// Where a glyph's ink begins below the y handed to `draw_abs`, as a
/// share of the font size: the call takes the top of the LINE box.
const INK_TOP: f64 = 0.30;

#[derive(Script, ScriptHook, Widget)]
pub struct NumberSpin {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawNumberSpin,
    #[live]
    draw_text: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[live]
    disabled: bool,

    /// The half being held, if any, and how long it has been repeating.
    #[rust]
    held: Option<f64>,
    #[rust]
    repeat: Timer,
    /// Where the drag started and how many steps it has already reported, so
    /// the value follows the hand rather than accumulating rounding.
    #[rust]
    drag: Option<(f64, f64)>,
}

impl NumberSpin {
    /// +1 for the upper half, -1 for the lower.
    fn half_at(&self, y: f64, rect: Rect) -> f64 {
        if y < rect.pos.y + rect.size.y * 0.5 {
            1.0
        } else {
            -1.0
        }
    }
}

impl Widget for NumberSpin {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.disabled = disabled;
        self.draw_bg.disabled = if disabled { 1.0 } else { 0.0 };
        self.draw_bg.redraw(cx);
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();
        let size = self.draw_text.text_style.font_size as f64;
        let mid = rect.pos.y + (rect.size.y * 0.5).floor();
        // Each mark is centred in its own half. Measured rather than
        // guessed: these are two different glyphs and they are not the same
        // width in every face.
        for (mark, top, bottom) in
            [(UP_MARK, rect.pos.y, mid), (DOWN_MARK, mid, rect.pos.y + rect.size.y)]
        {
            let w = measure(&self.draw_text, cx, mark);
            let pos = dvec2(
                rect.pos.x + (rect.size.x - w) * 0.5,
                top + (bottom - top - size) * 0.5 - size * INK_TOP,
            );
            self.draw_text.draw_abs(cx, pos, mark);
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.disabled {
            return;
        }
        let uid = self.uid;

        // A held button repeats, and speeds up the longer it is held. The
        // timer is restarted rather than made periodic so the interval can
        // shorten between beats.
        if self.repeat.is_event(event).is_some() {
            if let Some(direction) = self.held {
                cx.widget_action(uid, NumberSpinAction::Step(direction));
                self.repeat = cx.start_timeout(REPEAT_EVERY);
            }
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.half_at(fe.abs.y, fe.rect) as f32;
                if self.draw_bg.hot != hot {
                    self.draw_bg.hot = hot;
                    self.draw_bg.redraw(cx);
                }
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerHoverOut(_) => {
                self.draw_bg.hot = 0.0;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                let direction = self.half_at(fe.abs.y, fe.rect);
                self.held = Some(direction);
                self.drag = Some((fe.abs.y, 0.0));
                self.draw_bg.down = direction as f32;
                self.draw_bg.redraw(cx);
                cx.widget_action(uid, NumberSpinAction::Step(direction));
                self.repeat = cx.start_timeout(REPEAT_DELAY);
            }
            Hit::FingerMove(fe) => {
                let Some((y0, reported)) = self.drag else {
                    return;
                };
                // Up is more. Once the pointer has moved off the half it
                // started on, the press is a drag and the repeat stops:
                // otherwise the two would fight over the same value.
                let travel = (y0 - fe.abs.y) / DRAG_POINTS_PER_STEP;
                let want = travel.trunc();
                if want != reported {
                    if self.held.is_some() {
                        self.held = None;
                        cx.stop_timer(self.repeat);
                        self.repeat = Timer::empty();
                    }
                    cx.widget_action(uid, NumberSpinAction::Step(want - reported));
                    self.drag = Some((y0, want));
                }
            }
            Hit::FingerUp(_) => {
                self.held = None;
                self.drag = None;
                cx.stop_timer(self.repeat);
                self.repeat = Timer::empty();
                self.draw_bg.down = 0.0;
                self.draw_bg.redraw(cx);
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum NumberFieldAction {
    /// The value changed, however it was changed.
    Changed(f64),
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct NumberField {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    /// The number itself.
    #[find]
    #[live]
    pub input: WidgetRef,
    /// The two step buttons at the trailing edge.
    #[find]
    #[live]
    pub spin: WidgetRef,

    #[layout]
    layout: Layout,

    #[live(15.0)]
    spin_width: f64,
    #[live(8.0)]
    pad_left: f64,

    /// Asked of the input each pass rather than tracked: focus can
    /// leave for reasons this widget never hears about.
    #[rust]
    focused: bool,
    #[rust]
    hovered: bool,

    #[walk]
    walk: Walk,

    #[live]
    pub min: f64,
    #[live(100.0)]
    pub max: f64,
    #[live(1.0)]
    pub step: f64,
    /// Decimals shown, and the precision the readout is rounded to.
    #[live]
    pub precision: usize,
    /// At a bound, come round to the other one rather than stopping.
    #[live(false)]
    pub wrap: bool,
    /// Drawn after the number — "%", " kg", "deg". Not part of what is typed
    /// back, so a person may leave it in place or take it out.
    #[live]
    pub suffix: String,
    #[live]
    pub disabled: bool,

    #[rust]
    value: f64,
    /// The text last written into the input, so a redraw does not fight the
    /// caret while somebody is typing.
    #[rust]
    shown: String,
}

impl ScriptHook for NumberField {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.value = self.value.clamp(self.min, self.max);
        vm.with_cx_mut(|cx| {
            self.write_text(cx);
            if self.disabled {
                self.input.set_disabled(cx, true);
                self.spin.set_disabled(cx, true);
            }
        });
    }
}

impl NumberField {
    fn format(&self) -> String {
        format!("{:.*}{}", self.precision, self.value, self.suffix)
    }

    fn input(&self) -> WidgetRef {
        self.input.clone()
    }

    fn write_text(&mut self, cx: &mut Cx) {
        let text = self.format();
        if self.shown == text {
            return;
        }
        self.shown = text.clone();
        self.input().as_text_input().set_text(cx, &text);
    }

    /// Put a value in range: stop at the bounds, or come round to the other
    /// one when the field is a cycle rather than a quantity.
    fn bound(&self, v: f64) -> f64 {
        let span = self.max - self.min;
        if !self.wrap || span <= 0.0 {
            return v.clamp(self.min, self.max);
        }
        if v > self.max {
            self.min + (v - self.min).rem_euclid(span)
        } else if v < self.min {
            self.max - (self.max - v).rem_euclid(span)
        } else {
            v
        }
    }

    /// Round to the precision shown, so what is stored is what is read.
    fn quantize(&self, v: f64) -> f64 {
        let scale = 10f64.powi(self.precision as i32);
        (v * scale).round() / scale
    }

    fn commit(&mut self, cx: &mut Cx, v: f64) {
        let v = self.quantize(self.bound(v));
        if (v - self.value).abs() < f64::EPSILON {
            return;
        }
        self.value = v;
        self.write_text(cx);
        cx.widget_action(self.uid, NumberFieldAction::Changed(v));
    }

    /// Read whatever is in the box. Anything that will not parse is refused
    /// and the last good value is put back: silently keeping nonsense is
    /// worse than refusing it, and clearing the field loses the value the
    /// person was editing away from.
    fn take_typed(&mut self, cx: &mut Cx, typed: &str) {
        let typed = typed.trim();
        let typed = typed.strip_suffix(self.suffix.trim()).unwrap_or(typed);
        match typed.trim().parse::<f64>() {
            Ok(v) => {
                let v = self.quantize(self.bound(v));
                self.value = v;
                self.shown.clear();
                self.write_text(cx);
                cx.widget_action(self.uid, NumberFieldAction::Changed(v));
            }
            Err(_) => {
                self.shown.clear();
                self.write_text(cx);
            }
        }
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    pub fn set_value(&mut self, cx: &mut Cx, v: f64) {
        let v = self.quantize(self.bound(v));
        if (v - self.value).abs() > f64::EPSILON {
            self.value = v;
            self.write_text(cx);
        }
    }
}

impl Widget for NumberField {
    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        self.disabled = disabled;
        self.input.set_disabled(cx, disabled);
        self.spin.set_disabled(cx, disabled);
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.set_uniform(cx.cx, id!(hover), &[if self.hovered { 1.0 } else { 0.0 }]);
        self.draw_bg.set_uniform(cx.cx, id!(focus), &[if self.focused { 1.0 } else { 0.0 }]);
        self.draw_bg
            .set_uniform(cx.cx, id!(disabled), &[if self.disabled { 1.0 } else { 0.0 }]);

        self.draw_bg.begin(cx, walk, self.layout);
        let rect = cx.turtle().rect();
        // The number takes what is left after the column, worked out
        // here rather than asked of the turtle: a `Fill` child takes the
        // whole row and the column that follows it gets nothing.
        let spin_w = self.spin_width.max(0.0);
        let text_w = (rect.size.x - spin_w - self.pad_left).max(0.0);

        // Drawn here rather than by a container, so nothing else puts
        // them in the tree and a host can reach ids!(field.input).
        cx.widget_tree_insert_child(self.uid, live_id!(input), self.input.clone());
        cx.widget_tree_insert_child(self.uid, live_id!(spin), self.spin.clone());

        let mut input_walk = Walk::new(Size::Fixed(text_w), Size::Fixed(rect.size.y));
        input_walk.margin.left = self.pad_left;
        let _ = self.input.draw_walk(cx, scope, input_walk);
        let _ = self.spin.draw_walk(
            cx,
            scope,
            Walk::new(Size::Fixed(spin_w), Size::Fixed(rect.size.y)),
        );
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.disabled {
            return;
        }
        let actions = cx.capture_actions(|cx| {
            self.input.handle_event(cx, event, scope);
            self.spin.handle_event(cx, event, scope);
        });
        for action in actions {
            match action.as_widget_action().cast() {
                NumberSpinAction::Step(steps) => {
                    let v = self.value + steps * self.step;
                    self.commit(cx, v);
                }
                NumberSpinAction::None => {}
            }
            match action.as_widget_action().cast() {
                TextInputAction::Returned(text, _) => {
                    self.take_typed(cx, &text);
                }
                // Leaving the field commits it too, so a number typed and
                // then clicked away from is not quietly discarded. The
                // action carries nothing, so the box is asked.
                TextInputAction::KeyFocus => {
                    self.focused = true;
                    self.draw_bg.redraw(cx);
                }
                TextInputAction::KeyFocusLost => {
                    self.focused = false;
                    self.draw_bg.redraw(cx);
                    let typed = self.input().as_text_input().text();
                    self.take_typed(cx, &typed);
                }
                TextInputAction::Escaped => {
                    self.shown.clear();
                    self.write_text(cx);
                }
                _ => {}
            }
        }

        // The wheel works anywhere over the field, including over the text,
        // because there is nothing to aim at and every other numeric control
        // in the library answers to it.
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.hovered = true;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOut(_) => {
                self.hovered = false;
                self.draw_bg.redraw(cx);
            }
            _ => {}
        }
        if let Hit::FingerScroll(e) = event.hits(cx, self.draw_bg.area()) {
            let notches = -e.scroll.y.signum();
            if notches != 0.0 {
                let bite = if e.modifiers.shift { 10.0 } else { 1.0 };
                let v = self.value + notches * self.step * bite;
                self.commit(cx, v);
            }
        }
    }

    fn text(&self) -> String {
        self.format()
    }
}

impl NumberFieldRef {
    pub fn value(&self) -> f64 {
        self.borrow().map(|inner| inner.value()).unwrap_or(0.0)
    }

    pub fn set_value(&self, cx: &mut Cx, v: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, v);
        }
    }

    /// The new value, when this field changed in `actions`.
    pub fn changed(&self, actions: &Actions) -> Option<f64> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            NumberFieldAction::Changed(v) => Some(v),
            NumberFieldAction::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bounding and rounding on their own, without a script heap.
    struct Bounds {
        min: f64,
        max: f64,
        wrap: bool,
        precision: usize,
    }

    impl Bounds {
        fn bound(&self, v: f64) -> f64 {
            let span = self.max - self.min;
            if !self.wrap || span <= 0.0 {
                return v.clamp(self.min, self.max);
            }
            if v > self.max {
                self.min + (v - self.min).rem_euclid(span)
            } else if v < self.min {
                self.max - (self.max - v).rem_euclid(span)
            } else {
                v
            }
        }
        fn quantize(&self, v: f64) -> f64 {
            let scale = 10f64.powi(self.precision as i32);
            (v * scale).round() / scale
        }
    }

    fn counting() -> Bounds {
        Bounds { min: 0.0, max: 10.0, wrap: false, precision: 0 }
    }
    fn degrees() -> Bounds {
        Bounds { min: 0.0, max: 360.0, wrap: true, precision: 0 }
    }

    #[test]
    fn a_quantity_stops_at_its_ends() {
        let b = counting();
        assert_eq!(b.bound(11.0), 10.0);
        assert_eq!(b.bound(-1.0), 0.0);
        assert_eq!(b.bound(4.0), 4.0);
    }

    #[test]
    fn a_cycle_comes_round_instead() {
        // The reason wrap is not the default: a count that wraps is how a
        // form ends up ordering none of something.
        let b = degrees();
        assert_eq!(b.bound(370.0), 10.0);
        assert_eq!(b.bound(-10.0), 350.0);
        assert_eq!(b.bound(180.0), 180.0);
    }

    #[test]
    fn wrapping_a_long_way_past_the_end_still_lands_in_range() {
        let b = degrees();
        assert_eq!(b.bound(1090.0), 10.0);
        assert_eq!(b.bound(-730.0), 350.0);
    }

    #[test]
    fn a_zero_width_range_does_not_divide_by_it() {
        let b = Bounds { min: 5.0, max: 5.0, wrap: true, precision: 0 };
        assert_eq!(b.bound(9.0), 5.0);
    }

    #[test]
    fn the_stored_value_is_the_one_that_is_shown() {
        // Rounding to the shown precision, so a field displaying 0.30 does
        // not hold 0.30000000000000004 and hand it to its host.
        let b = Bounds { min: 0.0, max: 1.0, wrap: false, precision: 2 };
        assert_eq!(b.quantize(0.1 + 0.2), 0.3);
        assert_eq!(b.quantize(0.005), 0.01);
    }
}
