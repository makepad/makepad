//! ValueInput — a scrubbable number field.
//!
//! One compact widget, three ways in, exactly the Blender number-field
//! contract:
//!
//! * **Scrub**: click-drag horizontally anywhere on the field bends the
//!   value continuously (`step` per pixel) — fine control, live actions.
//! * **Step**: hovering reveals ‹ › arrows at the edges; a click on either
//!   steps the value ±`step`.
//! * **Type**: a plain click (no drag) drops into text editing — the
//!   embedded [`TextInput`] takes focus with the value selected; Enter
//!   commits, Escape reverts.
//!
//! A fifth way in, `track: true`, is off by default and changes what a press
//! MEANS. A field wide enough to carry a fill reads as a slider, and on a
//! slider the value belongs under the pointer: the press lands it at the
//! point it fell on, the drag keeps it there, and the release ends the
//! gesture once. The scrub above cannot do that — it measures travel from
//! wherever the press happened, so a press alone moves nothing and crossing
//! a two-hundred-point row at `step` per pixel takes a range's worth of
//! travel. The colour picker's channel rows are the case that forced it.
//!
//! On a track the two ends of the fill are the two ends of the range, so
//! there is no room left for a click that means something else: the edge
//! arrows retire, and the two gestures are told apart in TIME rather than in
//! space. One press sets the value; a SECOND press on the heels of the first
//! — the double-click every desktop already teaches — puts back what the
//! field held before the pair and opens the editor, so "type here" never
//! leaves a number behind that nobody asked for. Return does the same for a
//! hand already on the keyboard, and the arrow keys step the field once the
//! press has left the focus here.
//!
//! The widget owns the interaction and the chrome; real text editing is
//! delegated to the wrapped `TextInput` (the Slider idiom). Configure with
//! `min`/`max`/`step`/`precision` and an optional `suffix`. Every change —
//! scrub, arrow, or committed typing — emits [`ValueInputAction::Changed`];
//! hosts that mirror an external source (a clock, a sensor) push updates
//! back with [`ValueInput::set_value`], which yields while the operator's
//! finger or keyboard owns the field.

use crate::{
    badge::measure, makepad_derive_widget::*, makepad_draw::*, text_input::*, widget::*,
};

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ValueInputAction {
    /// The value changed: a scrub step, an arrow click, or committed
    /// typing. Carries the new (clamped) value.
    Changed(f64),
    #[default]
    None,
}

/// The end of a gesture on a track field, said once: the release that
/// finished a drag, a step, or a committed line of typing.
///
/// It is an action of its own rather than another arm of [`ValueInputAction`]
/// because the hosts that never asked for a track read that enum exhaustively
/// and would rather not learn about a mode they do not use. Followers of a
/// drag read `Changed`; whoever writes the value down reads this.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ValueInputTrackAction {
    Ended(f64),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawValueInput::script_shader(vm)){
        ..mod.draw.DrawQuad // splat in draw quad
    }

    mod.widgets.ValueInputBase = #(ValueInput::register_widget(vm))
    mod.widgets.ValueInput = set_type_default() do mod.widgets.ValueInputBase{
        width: 76
        height: 22
        /** the colour of the step marks at either end */
        arrow_color: #xa9b4bf

        draw_bg +: {
            hover: uniform(0.0)
            drag: uniform(0.0)
            focus: uniform(0.0)
            color: theme.color_inset
            border_color: theme.color_bevel
            // The share of the track the fill covers, or a negative number
            // on a field that is not one. Rust writes it on every draw, so
            // it is not a knob anybody can turn.
            fill: -1.0
            // A wash of the theme's own lightening overlay, and not the
            // value accent every other track in the library fills with: the
            // accent is chosen to be legible AGAINST a track with nothing
            // written on it, and this field writes its number straight over
            // the fill. A wash lifts the field's own colour instead, so the
            // ink over it is the ink that was already there.
            /** the fill behind the number on a track field */
            fill_color: uniform(theme.color_u_3)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                sdf.box(0.5, 0.5, w - 1.0, h - 1.0, 4.0)
                let lift = 0.06 * max(self.hover, self.drag) + 0.05 * self.focus
                sdf.fill(self.color + vec4(lift, lift, lift, 0.0))
                // The fill is the whole invitation to press: a field that
                // takes a press where it lands has to show where the value
                // stands, or there is nothing on screen to aim at. Negative
                // is off, which is every field that did not ask for a track.
                if self.fill >= 0.0 {
                    sdf.box(1.0, 1.0, max(1.0, (w - 2.0) * self.fill), h - 2.0, 3.5)
                    sdf.fill(self.fill_color)
                    sdf.box(0.5, 0.5, w - 1.0, h - 1.0, 4.0)
                }
                sdf.stroke(self.border_color, 1.0)
                // The step marks are drawn as glyphs by the Rust side.
                // They were two mirrored SDF paths here and only ever
                // the second one painted: swapping them changed nothing,
                // moving the failing one to mid-field changed nothing,
                // and the same geometry as a filled box painted fine.
                return sdf.result
            }
        }
        draw_text +: {
            color: #xf4f7fa
            // Line spacing of one, so the line box IS the ink box. With
            // the theme's leading the line is taller than the digits,
            // and centring the LINE leaves the digits riding high in a
            // field this short: a run of digits has no descender to
            // fill the bottom of the box.
            text_style: theme.font_bold{font_size: 11, line_spacing: 1.0}
        }
        text_input: TextInput{
            width: Fill
            height: Fill
            empty_text: ""
        }
    }
}

/// How far a press may wander (layout points) and still count as a CLICK
/// (edit mode) on release rather than a scrub.
const CLICK_SLOP: f64 = 3.0;
/// The edge band (layout points) where a plain click means STEP, not edit.
const ARROW_BAND: f64 = 14.0;
/// The step marks, shown while the pointer is over the field.
const LEFT_MARK: &str = "\u{2039}";
const RIGHT_MARK: &str = "\u{203a}";
/// How far each mark sits in from its edge.
const MARK_INSET: f64 = 5.0;
/// Where a glyph's ink begins below the y handed to `draw_abs`, as a
/// share of the font size. The call takes the top of the LINE box, and
/// the ink of a digit starts about a third of the way down it. Measured
/// on this face rather than assumed: without it a number centred by
/// arithmetic sits low, and one centred by `Align` sits high, because
/// digits have no descender to fill the bottom of the line.
const INK_TOP: f64 = 0.30;
/// How far a track's fill is held off the field's border, in points. The
/// press reads against the same two numbers the shader paints between, so
/// the value a point means and the fill the eye sees cannot drift apart.
const TRACK_INSET: f64 = 1.0;

/// Where a track's fill begins and ends inside a field `width` points wide.
pub fn track_span(width: f64) -> (f64, f64) {
    (TRACK_INSET, (width - TRACK_INSET).max(TRACK_INSET + 1.0))
}

/// The value a press at `x` — measured from the field's left edge — names on
/// a track whose fill runs from `lo` to `hi` in the same frame.
///
/// This is the fill's own arithmetic read backwards, which is the point of
/// it: press where the fill ends and you get the end of the range, press
/// half way and you get the middle. Past either end is that end, because a
/// hand that overshoots meant the limit.
pub fn track_value_at(x: f64, lo: f64, hi: f64, min: f64, max: f64) -> f64 {
    let span = (hi - lo).max(1.0);
    let t = ((x - lo) / span).clamp(0.0, 1.0);
    min + t * (max - min)
}

/// What a press on a track means, from the count of taps the platform hands
/// it. The two meanings are told apart in TIME because a track has no room
/// left in SPACE: its two ends are the ends of the range, and a column given
/// over to typing would put one of them out of reach.
pub fn track_press_means_typing(tap_count: u32) -> bool {
    tap_count >= 2
}

/// A widget-side shader with a fill in it, so a track field can show where
/// its value stands. `fill` is the share of the track covered, or a negative
/// number on the fields that are not tracks.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawValueInput {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    fill: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ValueInput {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawValueInput,
    #[live]
    draw_text: DrawText,
    #[live]
    text_input: TextInput,

    #[live(0.0)]
    pub min: f64,
    #[live(1.0)]
    pub max: f64,
    #[live(0.1)]
    pub step: f64,
    /// Decimal places shown (and used for the edit seed).
    #[live(1.0)]
    pub precision: f64,
    /// Drawn after the number ("%", " bpm", …). Not part of the edit text.
    #[live]
    pub suffix: String,
    /// Behave the way a filled row looks: the press lands the value where it
    /// falls instead of scrubbing from it. Off, and nothing below this line
    /// happens at all.
    #[live]
    pub track: bool,

    #[rust]
    value: f64,
    /// The colour of the step marks. On the widget, not on `draw_bg`,
    /// because the marks are glyphs now and Rust draws them.
    #[live]
    pub arrow_color: Vec4f,

    #[rust]
    editing: bool,
    /// The editor has been opened and still owes itself the keyboard.
    #[rust]
    edit_focus: bool,
    /// The pointer is over the field, so the marks show.
    #[rust]
    hovered: bool,
    /// A press in flight: (start x, value at press, wandered-past-slop).
    #[rust]
    drag: Option<(f64, f64, bool)>,
    /// A track's press is live from the first frame: the value follows the
    /// pointer until the release.
    #[rust]
    tracking: bool,
    /// What the field held before the current run of presses, so the second
    /// press of a double — the one that means "type here" — can put it back.
    #[rust]
    track_press_value: f64,
}

impl ValueInput {
    fn format(&self) -> String {
        format!("{:.*}{}", self.precision.max(0.0) as usize, self.value, self.suffix)
    }

    fn edit_seed(&self) -> String {
        format!("{:.*}", self.precision.max(0.0) as usize, self.value)
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// Push an externally-owned value (a clock repainting its readout).
    /// Yields while the operator's finger or keyboard owns the field.
    pub fn set_value(&mut self, cx: &mut Cx, value: f64) {
        if self.editing || self.drag.is_some() || self.tracking {
            return;
        }
        let value = value.clamp(self.min, self.max);
        if (value - self.value).abs() > f64::EPSILON {
            self.value = value;
            self.draw_bg.redraw(cx);
        }
    }

    fn commit(&mut self, cx: &mut Cx, uid: WidgetUid, value: f64) {
        let value = value.clamp(self.min, self.max);
        self.value = value;
        cx.widget_action(uid, ValueInputAction::Changed(value));
        self.draw_bg.redraw(cx);
    }

    /// Say that a gesture on a track has finished. Every discrete way of
    /// moving a track field — a step, a wheel notch, a committed line of
    /// typing — is both the change and the end of it, and says so.
    fn ended(&mut self, cx: &mut Cx, uid: WidgetUid) {
        if self.track {
            cx.widget_action(uid, ValueInputTrackAction::Ended(self.value));
        }
    }

    /// The value a press at `abs_x` names, snapped to `step` so the number
    /// the field shows is the number it holds. A track with no step of its
    /// own lands wherever the pointer is.
    fn track_value(&self, cx: &Cx, abs_x: f64) -> f64 {
        let face = self.draw_bg.area().rect(cx);
        let (lo, hi) = track_span(face.size.x);
        let raw = track_value_at(abs_x - face.pos.x, lo, hi, self.min, self.max);
        if self.step > 0.0 {
            ((raw / self.step).round() * self.step).clamp(self.min, self.max)
        } else {
            raw
        }
    }

    /// Move a track under the pointer: the change is only said when the
    /// value really moved, so a drag along a row of 255 values does not file
    /// a report per pixel.
    fn track_to(&mut self, cx: &mut Cx, uid: WidgetUid, value: f64) {
        if (value - self.value).abs() > f64::EPSILON {
            self.value = value;
            cx.widget_action(uid, ValueInputAction::Changed(value));
            self.draw_bg.redraw(cx);
        }
    }

    /// The second press of a double: the field goes back to what it held
    /// before the pair began, and the editor opens on it. Without the undo, a
    /// person reaching for the keyboard would first have to watch the value
    /// jump to wherever they happened to double-click.
    fn type_here(&mut self, cx: &mut Cx, uid: WidgetUid) {
        let back = self.track_press_value;
        self.tracking = false;
        self.draw_bg.set_uniform(cx, id!(drag), &[0.0]);
        self.track_to(cx, uid, back);
        self.ended(cx, uid);
        self.enter_edit(cx);
    }

    fn enter_edit(&mut self, cx: &mut Cx) {
        self.editing = true;
        self.edit_focus = true;
        let seed = self.edit_seed();
        self.text_input.set_text(cx, &seed);
        self.text_input.set_key_focus(cx);
        self.text_input.select_all(cx);
        self.draw_bg.redraw(cx);
    }

    fn exit_edit(&mut self, cx: &mut Cx) {
        self.editing = false;
        self.draw_bg.redraw(cx);
    }
}

impl Widget for ValueInput {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Written before the quad is begun, because that is when its
        // instance is handed to the shader.
        self.draw_bg.fill = if self.track && self.max > self.min {
            (((self.value - self.min) / (self.max - self.min)) as f32).clamp(0.0, 1.0)
        } else {
            -1.0
        };
        self.draw_bg.begin(cx, walk, self.layout);
        if self.editing {
            let _ = self.text_input.draw_walk(cx, scope, Walk::fill());
            // The editor asks for the keyboard the moment it opens, and on
            // the first edit of its life it has no area yet to ask with: the
            // request lands on nothing and the typing goes nowhere. It has
            // one now, so it asks again, once.
            if self.edit_focus && !self.text_input.area().is_empty() {
                self.edit_focus = false;
                self.text_input.set_key_focus(cx);
            }
        } else {
            let rect = cx.turtle().rect();
            let size = self.draw_text.text_style.font_size as f64;
            let y = rect.pos.y + (rect.size.y - size) * 0.5 - size * INK_TOP;
            let text = self.format();
            let tw = measure(&self.draw_text, cx, &text);
            self.draw_text
                .draw_abs(cx, dvec2(rect.pos.x + (rect.size.x - tw) * 0.5, y), &text);
            // Not on a track: there the ends of the field are the ends of
            // the range, and two little buttons sitting on them would take
            // the very presses that mean "nothing" and "everything".
            if (self.hovered || self.drag.is_some()) && !self.track {
                let rest = self.draw_text.color;
                self.draw_text.color = self.arrow_color;
                let mw = measure(&self.draw_text, cx, RIGHT_MARK);
                self.draw_text.draw_abs(cx, dvec2(rect.pos.x + MARK_INSET, y), LEFT_MARK);
                self.draw_text.draw_abs(
                    cx,
                    dvec2(rect.pos.x + rect.size.x - MARK_INSET - mw, y),
                    RIGHT_MARK,
                );
                self.draw_text.color = rest;
            }
        }
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    /// The number as the field shows it (precision and suffix), so
    /// `/snap` reads what a person reads.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.format())
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();

        if self.editing {
            for action in
                cx.capture_actions(|cx| self.text_input.handle_event(cx, event, scope))
            {
                match action.as_widget_action().cast() {
                    TextInputAction::Returned(text, _) => {
                        // Garbage reverts by simply not committing.
                        if let Ok(v) = text.trim().parse::<f64>() {
                            self.commit(cx, uid, v);
                            self.ended(cx, uid);
                        }
                        self.exit_edit(cx);
                    }
                    TextInputAction::Escaped | TextInputAction::KeyFocusLost => {
                        self.exit_edit(cx);
                    }
                    _ => {}
                }
            }
            return;
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.hovered = true;
                self.draw_bg.set_uniform(cx, id!(hover), &[1.0]);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOver(_) => {
                // Both say the same thing with the same arrows: what the hand
                // is about to do here is move a value left and right. A track
                // used to point with a hand, which says no more than "this
                // answers a press" — true of every row in the panel.
                cx.set_cursor(MouseCursor::EwResize);
            }
            // The wheel over a number is the cheapest way to nudge one,
            // and every other numeric control in the library answers to
            // it. Shift takes ten steps, the way a coarse drag would.
            Hit::FingerScroll(e) => {
                if !self.editing {
                    let notches = -e.scroll.y.signum();
                    if notches != 0.0 {
                        let bite = if e.modifiers.shift { 10.0 } else { 1.0 };
                        self.commit(cx, uid, self.value + notches * self.step * bite);
                        self.ended(cx, uid);
                    }
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hovered = false;
                self.draw_bg.set_uniform(cx, id!(hover), &[0.0]);
                self.draw_bg.redraw(cx);
            }
            // A track's press is the beginning of the gesture and its first
            // move at once. The second press of a double is the other
            // meaning: it puts the field back where the pair found it and
            // opens the editor, so reaching for the keyboard never leaves a
            // number behind.
            Hit::FingerDown(fe) if fe.device.is_primary_hit() && self.track => {
                cx.set_key_focus(self.draw_bg.area());
                if track_press_means_typing(fe.tap_count) {
                    self.type_here(cx, uid);
                } else {
                    self.track_press_value = self.value;
                    self.tracking = true;
                    self.draw_bg.set_uniform(cx, id!(drag), &[1.0]);
                    let v = self.track_value(cx, fe.abs.x);
                    self.track_to(cx, uid, v);
                }
            }
            Hit::FingerMove(fe) if self.tracking => {
                // Held for the whole pull: a captured area is sent moves and
                // not hovers, so the arrows from the hover would go stale the
                // moment the pointer left the row.
                cx.set_cursor(MouseCursor::EwResize);
                let v = self.track_value(cx, fe.abs.x);
                self.track_to(cx, uid, v);
            }
            Hit::FingerUp(_) if self.track => {
                if self.tracking {
                    self.tracking = false;
                    self.draw_bg.set_uniform(cx, id!(drag), &[0.0]);
                    self.ended(cx, uid);
                    self.draw_bg.redraw(cx);
                }
            }
            // The keyboard, once a press has left the focus here: a step an
            // arrow, and Return to type a number without reaching for the
            // pointer at all. This is where the edge arrows went.
            Hit::KeyDown(ke) if self.track && !self.editing => match ke.key_code {
                KeyCode::ArrowLeft | KeyCode::ArrowDown => {
                    self.commit(cx, uid, self.value - self.step);
                    self.ended(cx, uid);
                }
                KeyCode::ArrowRight | KeyCode::ArrowUp => {
                    self.commit(cx, uid, self.value + self.step);
                    self.ended(cx, uid);
                }
                KeyCode::ReturnKey => {
                    self.track_press_value = self.value;
                    self.enter_edit(cx);
                }
                _ => {}
            },
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                self.drag = Some((fe.abs.x, self.value, false));
                cx.set_cursor(MouseCursor::EwResize);
            }
            Hit::FingerMove(fe) => {
                if let Some((x0, v0, moved)) = self.drag {
                    let dx = fe.abs.x - x0;
                    if moved || dx.abs() > CLICK_SLOP {
                        // SCRUB: step per pixel, live.
                        self.drag = Some((x0, v0, true));
                        self.draw_bg.set_uniform(cx, id!(drag), &[1.0]);
                        self.commit(cx, uid, v0 + dx * self.step);
                    }
                }
            }
            Hit::FingerUp(fe) => {
                let Some((_, _, moved)) = self.drag.take() else { return };
                self.draw_bg.set_uniform(cx, id!(drag), &[0.0]);
                if moved || fe.cancelled {
                    self.draw_bg.redraw(cx);
                    return;
                }
                // A plain CLICK: edge bands step, the middle edits.
                let rect = self.draw_bg.area().rect(cx);
                let x = fe.abs.x - rect.pos.x;
                if x <= ARROW_BAND {
                    self.commit(cx, uid, self.value - self.step);
                } else if x >= rect.size.x - ARROW_BAND {
                    self.commit(cx, uid, self.value + self.step);
                } else {
                    self.enter_edit(cx);
                }
            }
            _ => {}
        }
    }
}

impl ValueInputRef {
    /// The new value, when this field changed in `actions`.
    pub fn changed(&self, actions: &Actions) -> Option<f64> {
        if let ValueInputAction::Changed(v) =
            actions.find_widget_action(self.widget_uid())?.cast()
        {
            return Some(v);
        }
        None
    }

    /// The value this field settled on, when a gesture on it ended in
    /// `actions`. Only a track field says this.
    pub fn ended(&self, actions: &Actions) -> Option<f64> {
        if let ValueInputTrackAction::Ended(v) =
            actions.find_widget_action(self.widget_uid())?.cast()
        {
            return Some(v);
        }
        None
    }

    pub fn value(&self) -> f64 {
        self.borrow().map(|inner| inner.value()).unwrap_or(0.0)
    }

    pub fn set_value(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, value);
        }
    }
}

/// A field that LOOKS like a slider, made to behave like one.
///
/// The colour picker's channel rows were filled bars wearing a scrub: the
/// press armed and changed nothing, three points of travel started a relative
/// drag at `step` per point, and crossing a row of 255 values took 255 points
/// of it. Pressing the fill at three quarters therefore did nothing visible
/// at all, which is what "I cannot move the sliders" meant. These say what a
/// track does instead, and the first two fail on the scrub.
#[cfg(test)]
mod track {
    use super::*;
    use crate::event::{ScrollEvent, ScrollPhase};
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: Vec2d = Vec2d { x: 800.0, y: 600.0 };
    const WINDOW: WindowId = WindowId(1, 1);
    const WIDE: f64 = 228.0;

    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx) }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn press(abs: Vec2d, time: f64) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time,
        })
    }

    fn moved(abs: Vec2d, time: f64) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: Vec2d::default(),
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time,
        })
    }

    fn release(abs: Vec2d, time: f64) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time,
        })
    }

    fn wheel(abs: Vec2d, notches: f64) -> Event {
        Event::Scroll(ScrollEvent {
            window_id: WINDOW,
            scroll: dvec2(0.0, -120.0 * notches),
            abs,
            modifiers: KeyModifiers::default(),
            handled_x: Cell::new(false),
            handled_y: Cell::new(false),
            is_mouse: true,
            time: 0.0,
            phase: ScrollPhase::Changed,
        })
    }

    fn key(key_code: KeyCode) -> Event {
        Event::KeyDown(KeyEvent {
            key_code,
            is_repeat: false,
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    /// Key focus is a request until the loop turns over, and the loop only
    /// turns over when there is something in it.
    fn settle_focus(cx: &mut Cx) {
        cx.action(());
        cx.handle_actions();
    }

    fn send(cx: &mut Cx, root: &WidgetRef, event: &Event) -> ActionsBuf {
        cx.capture_actions(|cx| root.handle_event(cx, event, &mut Scope::empty()))
    }

    /// With no event loop here to end a capture on the release, the area a
    /// press captured is let go by hand, or it takes every later press.
    fn claimed(event: &Event) -> Area {
        match event {
            Event::MouseDown(e) => e.handled.get(),
            Event::MouseMove(e) => e.handled.get(),
            _ => Area::Empty,
        }
    }

    fn start(cx: &mut Cx) -> (WidgetRef, WidgetRef, WidgetRef) {
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let root = cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    band := ValueInput{
                        width: 228.
                        min: 0.
                        max: 255.
                        step: 1.
                        precision: 0.
                        track: true
                    }
                    scrub := ValueInput{
                        width: 228.
                        min: 0.
                        max: 255.
                        step: 1.
                        precision: 0.
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let mut target = Target::new(cx);
        target.draw(cx, &root);
        let band = root.widget(cx, ids!(band));
        let scrub = root.widget(cx, ids!(scrub));
        assert!(!band.is_empty() && !scrub.is_empty(), "the scene has both fields");
        (root, band, scrub)
    }

    /// A window point `t` (0..1) along the field's fill — the span the field
    /// paints across and reads a press against.
    fn along(cx: &Cx, field: &WidgetRef, t: f64) -> Vec2d {
        let face = field.area().rect(cx);
        assert_eq!(face.size.x, WIDE, "the field drew at the width asked for");
        let (lo, hi) = track_span(face.size.x);
        dvec2(face.pos.x + lo + t * (hi - lo), face.pos.y + face.size.y * 0.5)
    }

    fn value(field: &WidgetRef) -> f64 {
        field.borrow::<ValueInput>().unwrap().value()
    }

    fn changed(actions: &ActionsBuf, field: &WidgetRef) -> Option<f64> {
        actions
            .filter_widget_actions_cast::<ValueInputAction>(field.widget_uid())
            .find_map(|a| match a {
                ValueInputAction::Changed(v) => Some(v),
                _ => None,
            })
    }

    fn ended(actions: &ActionsBuf, field: &WidgetRef) -> Option<f64> {
        actions
            .filter_widget_actions_cast::<ValueInputTrackAction>(field.widget_uid())
            .find_map(|a| match a {
                ValueInputTrackAction::Ended(v) => Some(v),
                _ => None,
            })
    }

    /// The mapping, with no window anywhere near it: a point along the fill
    /// names the value that point of the fill stands for.
    #[test]
    fn a_point_on_the_track_names_the_value_that_point_means() {
        assert_eq!(track_value_at(16.0, 16.0, 192.0, 0.0, 255.0), 0.0);
        assert_eq!(track_value_at(192.0, 16.0, 192.0, 0.0, 255.0), 255.0);
        assert!((track_value_at(104.0, 16.0, 192.0, 0.0, 255.0) - 127.5).abs() < 1e-9);
        // Past either end is that end: a hand that overshoots meant the limit.
        assert_eq!(track_value_at(-40.0, 16.0, 192.0, 0.0, 255.0), 0.0);
        assert_eq!(track_value_at(900.0, 16.0, 192.0, 0.0, 255.0), 255.0);
        // And the fill's two ends are the field's two edges, less the border
        // the shader keeps clear.
        assert_eq!(track_span(228.0), (1.0, 227.0));
    }

    /// The press lands the value under the pointer at once, the drag keeps it
    /// there, and the release says so once. This is the one the scrub fails.
    #[test]
    fn a_press_on_the_track_lands_the_value_and_the_drag_keeps_it() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &band, 0.5);
        let down = press(at, 0.0);
        let actions = send(&mut cx, &root, &down);
        assert_eq!(
            changed(&actions, &band),
            Some(128.0),
            "the press did not land the value it was pointing at"
        );
        assert_eq!(ended(&actions, &band), None, "the press ended a gesture it had just begun");
        let to = along(&cx, &band, 0.75);
        let actions = send(&mut cx, &root, &moved(to, 0.1));
        assert_eq!(changed(&actions, &band), Some(191.0), "the drag did not follow the pointer");
        let actions = send(&mut cx, &root, &release(to, 0.2));
        assert_eq!(ended(&actions, &band), Some(191.0), "the release did not commit");
        // And once only.
        let actions = send(&mut cx, &root, &release(to, 0.3));
        assert_eq!(ended(&actions, &band), None, "a second release ended the gesture again");
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
    }

    /// The two ends of the fill are the two ends of the range, which is the
    /// whole reason a hand can reach zero and full at all.
    #[test]
    fn the_ends_of_the_fill_are_the_ends_of_the_range() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        for (t, want) in [(1.0, 255.0), (0.0, 0.0)] {
            let at = along(&cx, &band, t);
            let down = press(at, 0.0);
            send(&mut cx, &root, &down);
            send(&mut cx, &root, &release(at, 0.1));
            down.unhandle(&mut cx, &claimed(&down));
            assert_eq!(value(&band), want, "the end of the fill named {want}");
        }
        cx.fingers.first_mouse_button = None;
    }

    /// And the field that did NOT ask for a track is the field it always was:
    /// the press only arms, travel scrubs it at a step per point, a plain
    /// click in the middle opens typing and a click on an end steps it.
    #[test]
    fn a_field_without_a_track_behaves_exactly_as_it_did() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, _band, scrub) = start(&mut cx);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &scrub, 0.5);
        let down = press(at, 0.0);
        let actions = send(&mut cx, &root, &down);
        assert_eq!(changed(&actions, &scrub), None, "the press moved a scrub field");
        assert_eq!(value(&scrub), 0.0);
        // Travel, and only travel, moves it — by the distance travelled.
        let to = dvec2(at.x + 10.0, at.y);
        let actions = send(&mut cx, &root, &moved(to, 0.1));
        assert_eq!(changed(&actions, &scrub), Some(10.0), "the scrub did not follow the travel");
        assert_eq!(ended(&actions, &scrub), None, "a scrub field said a track's end");
        send(&mut cx, &root, &release(to, 0.2));
        down.unhandle(&mut cx, &claimed(&down));

        // A click that did not wander: the middle types, an end steps.
        let face = scrub.area().rect(&cx);
        let middle = face.pos + face.size * 0.5;
        let down = press(middle, 1.0);
        send(&mut cx, &root, &down);
        send(&mut cx, &root, &release(middle, 1.1));
        down.unhandle(&mut cx, &claimed(&down));
        assert!(
            scrub.borrow::<ValueInput>().unwrap().editing,
            "a plain click in the middle no longer opens typing"
        );
        scrub.borrow_mut::<ValueInput>().unwrap().exit_edit(&mut cx);
        let stood = value(&scrub);
        let end = dvec2(face.pos.x + 4.0, middle.y);
        let down = press(end, 2.0);
        send(&mut cx, &root, &down);
        let actions = send(&mut cx, &root, &release(end, 2.1));
        down.unhandle(&mut cx, &claimed(&down));
        assert_eq!(
            changed(&actions, &scrub),
            Some(stood - 1.0),
            "a click on the low end no longer steps the field"
        );
        cx.fingers.first_mouse_button = None;
    }

    /// The wheel over a track still steps it, and the arrows step it once a
    /// press has left the keyboard here — which is where the edge arrows a
    /// track gives up went.
    #[test]
    fn the_wheel_and_the_arrows_still_step_a_track() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        let over = along(&cx, &band, 0.5);
        let actions = send(&mut cx, &root, &wheel(over, 1.0));
        assert_eq!(changed(&actions, &band), Some(1.0), "the wheel did not step the field");
        assert_eq!(ended(&actions, &band), Some(1.0), "a step is its own commit");
        let actions = send(&mut cx, &root, &wheel(over, -1.0));
        assert_eq!(changed(&actions, &band), Some(0.0), "and back again");

        // The keyboard is only reached by a press, and `set_key_focus` records
        // the request rather than granting it, so the actions are dispatched.
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &band, 0.5);
        let down = press(at, 0.0);
        root.handle_event(&mut cx, &down, &mut Scope::empty());
        cx.handle_actions();
        root.handle_event(&mut cx, &release(at, 0.1), &mut Scope::empty());
        cx.handle_actions();
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        assert!(cx.has_key_focus(band.area()), "the press left the keyboard elsewhere");
        let stood = value(&band);
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowRight));
        assert_eq!(changed(&actions, &band), Some(stood + 1.0), "the arrow did not step");
        assert_eq!(ended(&actions, &band), Some(stood + 1.0));
        let actions = send(&mut cx, &root, &key(KeyCode::ArrowLeft));
        assert_eq!(changed(&actions, &band), Some(stood), "and back again");
    }

    /// Typing is still the way in to an exact number, and Return still
    /// commits it. On a track the way to the editor is the keyboard's own
    /// Return, or a second press on the heels of the first.
    #[test]
    fn typing_an_exact_number_still_commits_on_a_track() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &band, 0.25);
        let down = press(at, 0.0);
        root.handle_event(&mut cx, &down, &mut Scope::empty());
        cx.handle_actions();
        root.handle_event(&mut cx, &release(at, 0.1), &mut Scope::empty());
        cx.handle_actions();
        down.unhandle(&mut cx, &claimed(&down));
        cx.fingers.first_mouse_button = None;
        assert_eq!(value(&band), 64.0, "the press did not land a quarter of the way along");

        // Dispatched, not captured: the editor asks for the keyboard with an
        // action, and a captured action never reaches the one it asks.
        root.handle_event(&mut cx, &key(KeyCode::ReturnKey), &mut Scope::empty());
        cx.handle_actions();
        assert!(
            band.borrow::<ValueInput>().unwrap().editing,
            "Return on a focused track did not open the editor"
        );
        // The frame the editor's redraw asks for, which is where it takes the
        // keyboard: a widget that has never been drawn has no area to take it
        // with.
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);
        settle_focus(&mut cx);
        let editor = band.borrow::<ValueInput>().unwrap().text_input.area();
        assert!(cx.has_key_focus(editor), "the editor opened without the keyboard");
        band.borrow_mut::<ValueInput>().unwrap().text_input.set_text(&mut cx, "200");
        let actions = send(&mut cx, &root, &key(KeyCode::ReturnKey));
        assert_eq!(changed(&actions, &band), Some(200.0), "the typed number never committed");
        assert_eq!(ended(&actions, &band), Some(200.0), "and a commit is the end of a gesture");
        assert!(!band.borrow::<ValueInput>().unwrap().editing, "the editor stayed open");
    }

    /// The second press of a double means "type here", and it puts the field
    /// back where the pair found it: reaching for the keyboard leaves no
    /// number behind that nobody asked for.
    ///
    /// The count of taps is the platform's own, kept where no test outside it
    /// can reach, so the rule and what it leads to are taken at the two seams
    /// the press runs through rather than at the press.
    #[test]
    fn a_double_press_opens_typing_and_puts_back_what_it_found() {
        assert!(!track_press_means_typing(1), "a single press means set it here");
        assert!(track_press_means_typing(2), "a second press on its heels means type here");
        assert!(track_press_means_typing(3), "and a third is still the keyboard");

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (root, band, _scrub) = start(&mut cx);
        band.borrow_mut::<ValueInput>().unwrap().set_value(&mut cx, 100.0);
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let at = along(&cx, &band, 0.75);
        let first = press(at, 0.0);
        send(&mut cx, &root, &first);
        send(&mut cx, &root, &release(at, 0.05));
        first.unhandle(&mut cx, &claimed(&first));
        cx.fingers.first_mouse_button = None;
        assert_eq!(value(&band), 191.0, "the first press of the pair still lands the value");

        let uid = band.widget_uid();
        let actions = cx.capture_actions(|cx| {
            band.borrow_mut::<ValueInput>().unwrap().type_here(cx, uid);
        });
        assert_eq!(
            changed(&actions, &band),
            Some(100.0),
            "the second press did not put back the number the pair found"
        );
        assert_eq!(ended(&actions, &band), Some(100.0), "and it ends the gesture it undid");
        assert!(
            band.borrow::<ValueInput>().unwrap().editing,
            "the second press did not open the editor"
        );
    }
}
