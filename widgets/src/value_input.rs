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

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

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

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                sdf.box(0.5, 0.5, w - 1.0, h - 1.0, 4.0)
                let lift = 0.06 * max(self.hover, self.drag) + 0.05 * self.focus
                sdf.fill(self.color + vec4(lift, lift, lift, 0.0))
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
    draw_bg: DrawQuad,
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

    #[rust]
    value: f64,
    /// The colour of the step marks. On the widget, not on `draw_bg`,
    /// because the marks are glyphs now and Rust draws them.
    #[live]
    pub arrow_color: Vec4f,

    #[rust]
    editing: bool,
    /// The pointer is over the field, so the marks show.
    #[rust]
    hovered: bool,
    /// A press in flight: (start x, value at press, wandered-past-slop).
    #[rust]
    drag: Option<(f64, f64, bool)>,
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
        if self.editing || self.drag.is_some() {
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

    fn enter_edit(&mut self, cx: &mut Cx) {
        self.editing = true;
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
        self.draw_bg.begin(cx, walk, self.layout);
        if self.editing {
            let _ = self.text_input.draw_walk(cx, scope, Walk::fill());
        } else {
            let rect = cx.turtle().rect();
            let size = self.draw_text.text_style.font_size as f64;
            let y = rect.pos.y + (rect.size.y - size) * 0.5 - size * INK_TOP;
            let text = self.format();
            let tw = measure(&self.draw_text, cx, &text);
            self.draw_text
                .draw_abs(cx, dvec2(rect.pos.x + (rect.size.x - tw) * 0.5, y), &text);
            if self.hovered || self.drag.is_some() {
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
                    }
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hovered = false;
                self.draw_bg.set_uniform(cx, id!(hover), &[0.0]);
                self.draw_bg.redraw(cx);
            }
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
                if moved {
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

    pub fn value(&self) -> f64 {
        self.borrow().map(|inner| inner.value()).unwrap_or(0.0)
    }

    pub fn set_value(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, value);
        }
    }
}
