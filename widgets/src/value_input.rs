//! ValueInput — a Blender-style scrubbable number field.
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

use crate::{makepad_derive_widget::*, makepad_draw::*, text_input::*, widget::*};

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

        draw_bg +: {
            hover: uniform(0.0)
            drag: uniform(0.0)
            focus: uniform(0.0)
            color: theme.color_inset
            border_color: theme.color_bevel
            arrow_color: #xa9b4bf

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                sdf.box(0.5, 0.5, w - 1.0, h - 1.0, 4.0)
                let lift = 0.06 * max(self.hover, self.drag) + 0.05 * self.focus
                sdf.fill(self.color + vec4(lift, lift, lift, 0.0))
                sdf.stroke(self.border_color, 1.0)
                // Hover reveals the step chevrons at the edges (Blender's
                // affordance); a drag keeps them lit.
                let a = max(self.hover, self.drag) * (1.0 - self.focus)
                if a > 0.01 {
                    let cy = h * 0.5
                    sdf.move_to(9.0, cy - 3.5)
                    sdf.line_to(5.5, cy)
                    sdf.line_to(9.0, cy + 3.5)
                    sdf.close_path()
                    sdf.fill(vec4(self.arrow_color.xyz * a, a))
                    sdf.move_to(w - 9.0, cy - 3.5)
                    sdf.line_to(w - 5.5, cy)
                    sdf.line_to(w - 9.0, cy + 3.5)
                    sdf.close_path()
                    sdf.fill(vec4(self.arrow_color.xyz * a, a))
                }
                return sdf.result
            }
        }
        draw_text +: {
            color: #xf4f7fa
            text_style: theme.font_bold{font_size: 11}
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
    #[rust]
    editing: bool,
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
            self.draw_text.draw_walk(
                cx,
                Walk::fill(),
                Align { x: 0.5, y: 0.5 },
                &self.format(),
            );
        }
        self.draw_bg.end(cx);
        DrawStep::done()
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
                self.draw_bg.set_uniform(cx, id!(hover), &[1.0]);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::EwResize);
            }
            Hit::FingerHoverOut(_) => {
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
