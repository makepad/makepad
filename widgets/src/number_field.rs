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
    animator::{Animate, AnimatorImpl},
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
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)
            color_disabled: uniform(theme.color_text_disabled)
            text_style: theme.font_regular{font_size: 8., line_spacing: 1.0}
            get_color: fn() {
                return self.color.mix(self.color_disabled, self.disabled)
            }
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
            color_disabled: uniform(theme.color_inset_disabled)
            rule_color: uniform(theme.color_bevel)
            rule_color_disabled: uniform(theme.color_bevel_disabled)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                let mid = floor(h * 0.5)

                // The two halves are lit separately, so the pointer says
                // which way a press would go before it goes. Disabled, neither
                // lights: there is nowhere a press would go.
                let live = 1.0 - self.disabled
                let upper_hot = max(0.0, self.hot) * live
                let lower_hot = max(0.0, -self.hot) * live
                let upper_down = max(0.0, self.down) * live
                let lower_down = max(0.0, -self.down) * live

                sdf.rect(0.0, 0.0, w, mid)
                sdf.fill(
                    self.color
                        .mix(self.color_hover, upper_hot)
                        .mix(self.color_down, upper_down)
                        .mix(self.color_disabled, self.disabled)
                )
                sdf.rect(0.0, mid, w, h - mid)
                sdf.fill(
                    self.color
                        .mix(self.color_hover, lower_hot)
                        .mix(self.color_down, lower_down)
                        .mix(self.color_disabled, self.disabled)
                )

                sdf.rect(0.0, mid - self.rule * 0.5, w, self.rule)
                sdf.fill(self.rule_color.mix(self.rule_color_disabled, self.disabled))

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
            border_color_disabled: uniform(theme.color_bevel_disabled)
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
                sdf.stroke(
                    self.border_color
                        .mix(self.border_color_focus, self.focus)
                        .mix(self.border_color_disabled, self.disabled)
                    1.0
                )
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
/// How far a press on the field body must travel before it is read as a
/// scrub rather than as somebody selecting the number to retype.
const BODY_DRAG_SLOP: f64 = 4.0;
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
        if disabled {
            // A press held when the field went off never sees its release.
            self.held = None;
            self.drag = None;
            cx.stop_timer(self.repeat);
            self.repeat = Timer::empty();
            self.draw_bg.hot = 0.0;
            self.draw_bg.down = 0.0;
        }
        self.draw_bg.redraw(cx);
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_text
            .set_dyn_instance(cx.cx, id!(disabled), &[self.draw_bg.disabled]);
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
                // The up and down arrow already say the two halves can
                // be pressed, and the half under the pointer lights to
                // say which. What nothing on screen says is that the
                // column can be DRAGGED, so that is what the cursor is
                // spent on.
                cx.set_cursor(MouseCursor::NsResize);
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
    /// A press in flight on the field body: where it started, how many
    /// steps it has already reported, and whether it has become a
    /// scrub. Kept apart from the spin column's own drag, which is a
    /// different gesture on a different target.
    #[rust]
    body_drag: Option<(DVec2, f64, bool)>,

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
    /// Any apply but an animation frame may have changed what the box should
    /// say or whether the field takes input: `suffix`, `precision`,
    /// `disabled`. The text is only written when the value changes and the
    /// parts only hear about `disabled` when they are told, so without this
    /// an edit to either reached the widget and never reached the screen.
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_animate() {
            return;
        }
        // A field that is new or reloaded is shown the way it is; one
        // changed while on screen moves to its new look.
        let animate = if apply.is_eval() { Animate::Yes } else { Animate::No };
        vm.with_cx_mut(|cx| {
            if apply.is_new() {
                self.value = self.value.clamp(self.min, self.max);
            }
            self.refresh_text(cx);
            self.sync_disabled(cx, animate);
        });
    }
}

/// The text a field shows for a value: `precision` decimals, then the
/// suffix exactly as it was given, leading space and all.
fn readout(value: f64, precision: usize, suffix: &str) -> String {
    format!("{:.*}{}", precision, value, suffix)
}

/// What an apply writes into the box, if anything: the readout, when that is
/// not what the box last showed. Nothing while somebody is typing: the
/// characters are theirs until they commit or abandon them, and either one
/// writes the readout with whatever the suffix is by then.
fn text_after_apply(shown: &str, readout: &str, editing: bool) -> Option<String> {
    if editing || shown == readout {
        None
    } else {
        Some(readout.to_string())
    }
}

/// Whether an event goes on to the box and the step buttons. Disabled, only
/// two do. The frame clock, because the box dims by animating and an
/// animation that never sees a frame never gets there. And the box losing
/// the keyboard, which disabling takes from it, so its caret and focus ring
/// go too. No press, key or character gets through.
fn reaches_parts(disabled: bool, event: &Event) -> bool {
    !disabled || matches!(event, Event::NextFrame(_) | Event::KeyFocusLost(_))
}

impl NumberField {
    fn format(&self) -> String {
        readout(self.value, self.precision, &self.suffix)
    }

    fn input(&self) -> WidgetRef {
        self.input.clone()
    }

    /// Somebody is typing in the box. Both are asked: `focused` is only
    /// news the box sent, and a new field's empty area would otherwise
    /// count as holding the keyboard when nothing does.
    fn editing(&self, cx: &Cx) -> bool {
        self.focused && self.input.key_focus(cx)
    }

    /// Write the readout into the box after an apply, unless it is being
    /// typed in.
    fn refresh_text(&mut self, cx: &mut Cx) {
        let readout = self.format();
        if let Some(text) = text_after_apply(&self.shown, &readout, self.editing(cx)) {
            self.input().as_text_input().set_text(cx, &text);
            self.shown = text;
        }
    }

    /// Bring the box and the step buttons in line with `disabled`, and take
    /// the keyboard from a field that has just stopped taking input: what
    /// was half typed is dropped and the readout put back, as Escape would.
    fn sync_disabled(&mut self, cx: &mut Cx, animate: Animate) {
        if self.spin.disabled(cx) != self.disabled {
            self.spin.set_disabled(cx, self.disabled);
        }
        if self.input.disabled(cx) != self.disabled {
            match animate {
                Animate::Yes => self.input.set_disabled(cx, self.disabled),
                Animate::No => {
                    if let Some(mut input) = self.input.as_text_input().borrow_mut() {
                        let state = if self.disabled {
                            ids!(disabled.on)
                        } else {
                            ids!(disabled.off)
                        };
                        input.animator_cut(cx, state);
                    }
                }
            }
        }
        if self.disabled {
            if self.editing(cx) {
                cx.set_key_focus(Area::Empty);
                self.shown.clear();
                self.write_text(cx);
            }
            self.focused = false;
            self.body_drag = None;
        }
        self.draw_bg.redraw(cx);
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

    /// Say, with the pointer, that dragging up and down changes the
    /// number. Nothing in the drawing says it, and the text box under
    /// the pointer actively says something else: a caret means "this is
    /// text you can select", which is true of a sideways drag and wrong
    /// about the gesture the field is for.
    ///
    /// Not while it is being typed in. Once the box has the keyboard
    /// the person is working on the characters, and taking the caret
    /// away from them to advertise a gesture they have already passed
    /// over would be the wrong trade.
    fn offer_drag_cursor(&self, cx: &mut Cx) {
        if !self.focused {
            cx.set_cursor(MouseCursor::NsResize);
        }
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
        self.sync_disabled(cx, Animate::Yes);
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Per-instance values, not uniforms. The shader declares all three
        // as plain numbers, which makes them instance fields, and
        // `set_uniform` on a name with no uniform behind it writes nothing:
        // the box never lit under the pointer, never showed focus and never
        // dimmed.
        let flag = |on: bool| [if on { 1.0 } else { 0.0 }];
        self.draw_bg.set_dyn_instance(cx.cx, id!(hover), &flag(self.hovered));
        self.draw_bg.set_dyn_instance(cx.cx, id!(focus), &flag(self.focused));
        self.draw_bg.set_dyn_instance(cx.cx, id!(disabled), &flag(self.disabled));

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
        if !reaches_parts(self.disabled, event) {
            return;
        }
        let actions = cx.capture_actions(|cx| {
            self.input.handle_event(cx, event, scope);
            self.spin.handle_event(cx, event, scope);
        });
        // What got through to a disabled field's parts was only ever there to
        // let them settle; the field itself does nothing with it.
        if self.disabled {
            return;
        }
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

        // Dragging the FIELD changes the value, not only dragging the
        // step column. Watched as raw mouse events rather than through
        // `hits`, because the text box covers the field and would take
        // the press first: by the time a hit came back the input would
        // already be selecting text.
        //
        // Vertical is the value, horizontal is the text. Whichever way
        // the pointer commits to first wins the gesture, so selecting a
        // number to retype still works and a drag up still counts.
        let rect = self.draw_bg.area().rect(cx);
        match event {
            Event::MouseDown(e) if rect.contains(e.abs) => {
                self.body_drag = Some((e.abs, 0.0, false));
            }
            Event::MouseMove(e) => {
                if let Some((from, reported, scrubbing)) = self.body_drag {
                    let dx = (e.abs.x - from.x).abs();
                    let dy = from.y - e.abs.y;
                    if !scrubbing {
                        // Not decided yet. It becomes a scrub only when
                        // the pointer has gone further up or down than
                        // across; a press that wanders sideways is
                        // somebody selecting the number.
                        if dy.abs() > BODY_DRAG_SLOP && dy.abs() > dx {
                            self.body_drag = Some((from, 0.0, true));
                        } else if dx > BODY_DRAG_SLOP {
                            self.body_drag = None;
                        }
                    }
                    if let Some((from, reported, true)) = self.body_drag {
                        let want = ((from.y - e.abs.y) / DRAG_POINTS_PER_STEP).trunc();
                        if want != reported {
                            let v = self.value + (want - reported) * self.step;
                            self.commit(cx, v);
                            self.body_drag = Some((from, want, true));
                        }
                    }
                    let _ = reported;
                }
            }
            Event::MouseUp(_) => {
                self.body_drag = None;
            }
            _ => {}
        }

        // The wheel works anywhere over the field, including over the text,
        // because there is nothing to aim at and every other numeric control
        // in the library answers to it.
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.hovered = true;
                self.draw_bg.redraw(cx);
                self.offer_drag_cursor(cx);
            }
            Hit::FingerHoverOver(_) => {
                // Asserted on every move, not once on the way in: the
                // text box under the pointer sets the caret cursor from
                // its own hover, and whichever of the two speaks last
                // is the one the pointer wears. Children are handled
                // at the top of this function, so this is last.
                self.offer_drag_cursor(cx);
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
    use crate::makepad_draw::cx_draw::CxDraw;

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
    fn the_readout_is_the_value_at_its_precision_then_the_suffix() {
        assert_eq!(readout(12.0, 0, ""), "12");
        assert_eq!(readout(0.3, 2, "%"), "0.30%");
        assert_eq!(readout(72.5, 1, " kg"), "72.5 kg");
    }

    #[test]
    fn an_apply_that_changes_nothing_writes_nothing() {
        // Writing the same text again would put the caret back at the end.
        assert_eq!(text_after_apply("1 pcs", "1 pcs", false), None);
    }

    #[test]
    fn an_apply_leaves_an_edit_in_progress_alone() {
        let text = readout(1.0, 0, " pcs");
        assert_eq!(text_after_apply("17", &text, true), None);
    }

    fn press() -> Event {
        Event::MouseDown(MouseDownEvent {
            abs: dvec2(10.0, 10.0),
            button: MouseButton::PRIMARY,
            window_id: WindowId(0, 0),
            modifiers: Default::default(),
            handled: std::cell::Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    #[test]
    fn a_disabled_field_passes_no_press_key_or_character_to_its_parts() {
        let character = Event::TextInput(TextInputEvent {
            input: "7".to_string(),
            ..Default::default()
        });
        for event in [
            press(),
            Event::KeyDown(KeyEvent::default()),
            Event::KeyUp(KeyEvent::default()),
            character,
        ] {
            assert!(!reaches_parts(true, &event), "{event:?}");
        }
    }

    #[test]
    fn a_disabled_field_still_gives_its_parts_the_frame_clock() {
        // The box dims by animating, and an animation only moves on frames:
        // withheld, the box never reached its disabled look.
        assert!(reaches_parts(true, &Event::NextFrame(NextFrameEvent::default())));
    }

    #[test]
    fn a_disabled_field_lets_its_box_hear_that_it_lost_the_keyboard() {
        // Disabling takes the keyboard away; the box has to be told, or its
        // caret and focus ring stay on.
        let lost = Event::KeyFocusLost(KeyFocusEvent { prev: Area::Empty, focus: Area::Empty });
        assert!(reaches_parts(true, &lost));
    }

    #[test]
    fn an_enabled_field_passes_everything_on() {
        assert!(reaches_parts(false, &press()));
        assert!(reaches_parts(false, &Event::KeyDown(KeyEvent::default())));
        assert!(reaches_parts(false, &Event::NextFrame(NextFrameEvent::default())));
    }

    #[test]
    fn the_stored_value_is_the_one_that_is_shown() {
        // Rounding to the shown precision, so a field displaying 0.30 does
        // not hold 0.30000000000000004 and hand it to its host.
        let b = Bounds { min: 0.0, max: 1.0, wrap: false, precision: 2 };
        assert_eq!(b.quantize(0.1 + 0.2), 0.3);
        assert_eq!(b.quantize(0.005), 0.01);
    }

    // The tests below build a whole field from the library's default, apply to
    // it the way the catalogue's controls do, draw it and hand it frames. The
    // ones above check the rules; these check that the rules reach the screen.

    const FIELD: DVec2 = dvec2(140.0, 24.0);

    fn new_field(cx: &mut Cx) -> NumberField {
        cx.with_vm(crate::script_mod);
        cx.with_vm(NumberField::script_new_with_default)
    }

    /// One draw of the field on its own, in the pass and draw list the test
    /// keeps.
    fn draw(cx: &mut Cx, field: &mut NumberField, pass: &DrawPass, list: &mut DrawList2d) {
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(pass, None);
        list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(FIELD, Layout::flow_down());
        let _ = field.draw_walk(&mut cx2d, &mut Scope::empty(), Walk::fixed(FIELD.x, FIELD.y));
        cx2d.end_pass_sized_turtle();
        list.end(&mut cx2d);
        cx2d.end_pass(pass);
    }

    /// An instance value as the last draw left it in the buffer behind
    /// `area`, which is what the GPU is handed. A value written anywhere
    /// else, such as a uniform the shader never declared, does not show here.
    fn drawn(cx: &Cx, area: Area, name: LiveId) -> Option<f32> {
        let inst = area.valid_instance(cx)?;
        let item = &cx.draw_lists[inst.draw_list_id].draw_items[inst.draw_item_id];
        let shader = &cx.draw_shaders[item.kind.draw_call()?.draw_shader_id.index];
        let input = shader.mapping.instances.inputs.iter().find(|input| input.id == name)?;
        item.instances.as_ref()?.get(inst.instance_offset + input.offset).copied()
    }

    /// Frames at the given times, each one for every animation that has asked
    /// for a frame so far, handed to the field the way a window hands them.
    fn run_frames(cx: &mut Cx, field: &mut NumberField, times: &[f64]) {
        for (frame, time) in times.iter().enumerate() {
            let last = cx.new_next_frame();
            let event = Event::NextFrame(NextFrameEvent {
                frame: frame as u64,
                time: *time,
                set: (1..=last.0).map(NextFrame).collect(),
            });
            field.handle_event(cx, &event, &mut Scope::empty());
        }
    }

    /// The catalogue's Suffix control on a field already showing its number:
    /// the suffix is in the box at once, and so is a new precision. The text
    /// used to be written only when the value changed, so both were stored and
    /// the box went on saying "0".
    #[test]
    fn a_suffix_or_precision_applied_to_a_field_on_screen_is_written_into_its_box() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut field = new_field(&mut cx);
        let text = |field: &NumberField| field.input.as_text_input().text();
        assert_eq!(text(&field), "0");

        script_apply_eval!(cx, field, { suffix: " pcs" });
        assert_eq!(text(&field), "0 pcs");
        script_apply_eval!(cx, field, { precision: 2 });
        assert_eq!(text(&field), "0.00 pcs");
    }

    /// The box draws the hover, focus and disabled it holds. Its shader
    /// declares all three as plain numbers, which makes them instance values,
    /// and they were written as uniforms: nothing reached the buffer, so the
    /// box never lit, never showed focus and never dimmed.
    #[test]
    fn the_box_draws_its_hover_focus_and_disabled_look() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut field = new_field(&mut cx);
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, FIELD);
        let mut list = DrawList2d::new(&mut cx);
        let look = |cx: &Cx, field: &NumberField| {
            [id!(hover), id!(focus), id!(disabled)]
                .map(|name| drawn(cx, field.draw_bg.area(), name))
        };

        draw(&mut cx, &mut field, &pass, &mut list);
        assert_eq!(look(&cx, &field), [Some(0.0), Some(0.0), Some(0.0)], "at rest");

        field.hovered = true;
        field.focused = true;
        draw(&mut cx, &mut field, &pass, &mut list);
        assert_eq!(look(&cx, &field), [Some(1.0), Some(1.0), Some(0.0)], "hovered and focused");

        // Disabling takes the keyboard, so focus goes with it.
        script_apply_eval!(cx, field, { disabled: true });
        draw(&mut cx, &mut field, &pass, &mut list);
        assert_eq!(look(&cx, &field), [Some(1.0), Some(0.0), Some(1.0)], "disabled");
        assert_eq!(drawn(&cx, field.spin.area(), id!(disabled)), Some(1.0), "the step column");
    }

    /// The catalogue's Disabled control on a field on screen: the text box
    /// fades to its disabled look over the frames that follow, and back when
    /// the field is switched on again. The fade runs on frames, and a disabled
    /// field used to pass its parts no event at all, so the box never got
    /// there.
    #[test]
    fn a_field_switched_off_on_screen_fades_its_text_box_over_the_next_frames() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut field = new_field(&mut cx);
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, FIELD);
        let mut list = DrawList2d::new(&mut cx);
        draw(&mut cx, &mut field, &pass, &mut list);
        assert_eq!(drawn(&cx, field.input.area(), id!(disabled)), Some(0.0), "at rest");

        script_apply_eval!(cx, field, { disabled: true });
        run_frames(&mut cx, &mut field, &[0.0, 0.1, 0.5, 1.0]);
        draw(&mut cx, &mut field, &pass, &mut list);
        assert_eq!(drawn(&cx, field.input.area(), id!(disabled)), Some(1.0), "switched off");

        script_apply_eval!(cx, field, { disabled: false });
        run_frames(&mut cx, &mut field, &[2.0, 2.1, 2.5, 3.0]);
        draw(&mut cx, &mut field, &pass, &mut list);
        assert_eq!(drawn(&cx, field.input.area(), id!(disabled)), Some(0.0), "switched on again");
    }
}
