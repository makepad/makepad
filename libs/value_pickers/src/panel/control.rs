//! The panel's value controls: one widget, drawn by hand in one pass (no
//! child layout to redo while a value moves), in the forms a value takes:
//! a slider, a knob, a number, an angle dial, a range, an XY pad, a vec3, a
//! toggle, segmented choices, a seed with its re-roll, an ease.
//!
//! Each is bound to a path (`path: "WORD_SPARKS.count"`): it shows what
//! the host reads there, and every step of a gesture sets it there
//! ([`super::host`]). The value keeps its written form: `0.12s` stays in
//! seconds, `vec2(…)` stays a vec2, `@ease_out` a name.
//!
//! Every number is a VALUE FIELD (the slider, the number, a range's ends, a
//! vector's parts, the readouts of knobs, faders and dials): one compact
//! field with its label and value inside it. Drag anywhere in it to change
//! the value (the pointer hides and never runs out of screen; Shift fine,
//! Ctrl/Cmd snaps to round steps); a click without a drag types into it (an
//! expression, `+0.5` relative, a unit: `200ms`, `90deg`; Enter commits,
//! Esc cancels, Tab goes on to the next field). The range (`min`/`max`, or
//! made up from the value) is SOFT: it sets the slider's fill and how fast a
//! drag moves, and grows to a round number whenever a value goes past it.
//! Only `hard_min`/`hard_max` clamp, where the value's domain demands it.
//! Over a field: Backspace puts back the value the panel opened with,
//! Minus negates, Ctrl/Cmd+C / V copy and paste, Ctrl+wheel steps, Enter
//! types, I keys it; a right click is its menu ([`field`] is the arithmetic).

use super::field;
use super::host::{self, fmt_number, numbers_in, with_numbers, KeyButton, PanelValue};
use makepad_widgets::badge::advance;
use makepad_widgets::makepad_script::ScriptFnRef;
use makepad_widgets::*;
use std::cell::{Cell, RefCell};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    let PanelKind = set_type_default() do #(PanelKind::script_api(vm))
    mod.widgets.PanelKind = PanelKind

    mod.widgets.DrawPanelShapeBase = #(DrawPanelShape::script_component(vm))
    /** The panel's one shape: a rounded box (its corners at most half its
     * short side, so a small box is a pill or a dot) with an optional edge,
     * or a chevron (`shape` 9: `value` 0 points right, 1 down). */
    set_type_default() do #(DrawPanelShape::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            if self.shape < 8.5 {
                let r = min(self.radius, min(w, h) * 0.5)
                if self.stroke > 0.0 {
                    let i = self.stroke * 0.5
                    sdf.box(i, i, w - self.stroke, h - self.stroke, max(r - i, 0.0))
                    sdf.fill_keep(self.fill)
                    sdf.stroke(self.edge, self.stroke)
                } else {
                    sdf.box(0.0, 0.0, w, h, r)
                    sdf.fill(self.fill)
                }
            } else {
                let t = self.value
                sdf.move_to(mix(w * 0.36, w * 0.24, t), mix(h * 0.22, h * 0.36, t))
                sdf.line_to(mix(w * 0.66, w * 0.5, t), mix(h * 0.5, h * 0.66, t))
                sdf.line_to(mix(w * 0.36, w * 0.76, t), mix(h * 0.78, h * 0.36, t))
                sdf.stroke(self.fill, 1.4)
            }
            return sdf.result
        }
    }

    mod.widgets.DrawPanelKeyBase = #(DrawPanelKey::script_component(vm))
    /** A control's key button (a diamond) and its note. */
    set_type_default() do #(DrawPanelKey::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            if self.shape < 0.5 {
                sdf.move_to(w * 0.5, 0.5)
                sdf.line_to(w - 0.5, h * 0.5)
                sdf.line_to(w * 0.5, h - 0.5)
                sdf.line_to(0.5, h * 0.5)
                sdf.close_path()
                sdf.fill_keep(self.fill)
                sdf.stroke(self.edge, 1.0)
            } else if self.shape > 1.5 {
                // An animated value's cue: a small eased curve.
                sdf.move_to(1.0, h - 2.0)
                sdf.line_to(w * 0.3, h - 2.5)
                sdf.line_to(w * 0.5, h * 0.5)
                sdf.line_to(w * 0.7, 2.5)
                sdf.line_to(w - 1.0, 2.0)
                sdf.stroke(self.fill, 1.3)
            } else {
                sdf.box(0.5, 0.5, w - 1.0, h - 1.0, 4.0)
                sdf.fill_keep(self.fill)
                sdf.stroke(self.edge, 1.0)
            }
            return sdf.result
        }
    }

    mod.widgets.PanelControlBase = #(PanelControl::register_widget(vm))

    /** A value control bound to a path in the scene's code. */
    mod.widgets.PanelControl = set_type_default() do mod.widgets.PanelControlBase{
        width: Fill height: Fit
        kind: PanelKind.Slider
        draw_shape +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                if self.shape < 0.5 {
                    // A rounded box.
                    sdf.box(0.5, 0.5, w - 1.0, h - 1.0, min(self.radius, min(w, h) * 0.5 - 0.5))
                    sdf.fill_keep(self.fill)
                    sdf.stroke(self.edge, self.stroke)
                } else if self.shape < 1.5 {
                    // A knob: a body lit from above, its track and the
                    // value's arc (from 7 o'clock round to 5), a thin outer
                    // arc for what modulates it (value to value2), and a
                    // pointer.
                    let c = vec2(w * 0.5, h * 0.5)
                    let r = min(w, h) * 0.5 - 2.0
                    let a0 = 0.75 * 3.1415926
                    let span = 1.5 * 3.1415926
                    sdf.circle(c.x, c.y, r - 6.0)
                    let lit = 1.0 - self.pos.y * 0.6
                    sdf.fill(mix(self.body, self.body + vec4(0.08, 0.08, 0.09, 0.0), lit))
                    sdf.arc_round_caps(c.x, c.y, r - 1.5, a0, a0 + span, 3.0)
                    sdf.fill(self.edge)
                    sdf.arc_round_caps(c.x, c.y, r - 1.5, a0, a0 + span * max(self.value, 0.001), 3.0)
                    sdf.fill(self.fill)
                    if abs(self.value2 - self.value) > 0.001 {
                        let lo = min(self.value, self.value2)
                        let hi = max(self.value, self.value2)
                        sdf.arc_round_caps(c.x, c.y, r + 1.0, a0 + span * lo, a0 + span * hi, 1.2)
                        sdf.fill(self.fill * vec4(1.0, 1.0, 1.0, 0.7))
                    }
                    let a = a0 + span * self.value
                    sdf.move_to(c.x + cos(a) * (r * 0.18), c.y + sin(a) * (r * 0.18))
                    sdf.line_to(c.x + cos(a) * (r - 8.0), c.y + sin(a) * (r - 8.0))
                    sdf.stroke(vec4(0.95, 0.95, 0.95, 1.0), 2.0)
                } else if self.shape < 2.5 {
                    // A dial: its face and a needle at `value` radians (0
                    // to the right, turning clockwise as the screen's y).
                    let c = vec2(w * 0.5, h * 0.5)
                    let r = min(w, h) * 0.5 - 1.5
                    sdf.circle(c.x, c.y, r)
                    sdf.fill_keep(self.edge)
                    sdf.stroke(self.fill * 0.6, 1.0)
                    sdf.move_to(c.x, c.y)
                    sdf.line_to(c.x + cos(self.value) * (r - 2.0), c.y + sin(self.value) * (r - 2.0))
                    sdf.stroke(self.fill, 2.0)
                    sdf.circle(c.x, c.y, 2.5)
                    sdf.fill(self.fill)
                } else if self.shape < 3.5 {
                    // A toggle: its pill and the knob at `value`.
                    sdf.box(0.5, 0.5, w - 1.0, h - 1.0, h * 0.5 - 0.5)
                    sdf.fill(mix(self.edge, self.fill, self.value))
                    sdf.circle(h * 0.5 + (w - h) * self.value, h * 0.5, h * 0.5 - 2.5)
                    sdf.fill(vec4(0.95, 0.95, 0.95, 1.0))
                } else if self.shape < 4.5 {
                    // A puck: a light ring in a dark one, legible on any fill.
                    let c = vec2(w * 0.5, h * 0.5)
                    sdf.circle(c.x, c.y, min(w, h) * 0.5 - 1.0)
                    sdf.stroke(vec4(0.04, 0.04, 0.04, 0.9), 1.4)
                    sdf.circle(c.x, c.y, min(w, h) * 0.5 - 2.5)
                    sdf.stroke(self.fill, 1.6)
                } else if self.shape < 5.5 {
                    // A field's step arrow: a small chevron pointing left
                    // (`value` 0) or right (1).
                    let x0 = mix(w * 0.62, w * 0.38, self.value)
                    let x1 = mix(w * 0.38, w * 0.62, self.value)
                    sdf.move_to(x0, h * 0.3)
                    sdf.line_to(x1, h * 0.5)
                    sdf.line_to(x0, h * 0.7)
                    sdf.stroke(self.fill, 1.3)
                } else {
                    // A fader: a vertical groove with tick marks and a cap.
                    let cx = w * 0.5
                    sdf.box(cx - 2.0, 4.0, 4.0, h - 8.0, 2.0)
                    sdf.fill(self.edge)
                    let cy = 4.0 + (h - 8.0) * (1.0 - self.value)
                    sdf.box(cx - 2.0, cy, 4.0, h - 4.0 - cy, 2.0)
                    sdf.fill(self.fill)
                    sdf.box(cx - 9.0, cy - 5.0, 18.0, 10.0, 2.5)
                    sdf.fill_keep(mix(self.body + vec4(0.12, 0.12, 0.13, 0.0), self.body, self.pos.y))
                    sdf.stroke(self.fill, 1.0)
                }
                return sdf.result
            }
        }
        draw_bg +: {color: #x00000000}
        draw_label +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_value +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_menu_text +: {text_style: theme.font_regular{font_size: 8.5}}
        /** What a field types into (only while it is typed in). */
        input: TextInput{width: Fill height: Fill min_height: 0 margin: 0 padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
            empty_text: "" is_numeric_only: false label_align: Align{x: 0.0 y: 0.5}
            draw_bg +: {color: #x00000000 color_hover: #x00000000 color_focus: #x00000000 color_down: #x00000000 color_empty: #x00000000 border_size: 0.0 border_radius: 0.0}
            draw_text +: {text_style: theme.font_regular{font_size: 8.5} color: #xf4f7fa color_hover: #xf4f7fa color_focus: #xf4f7fa color_down: #xf4f7fa color_empty: #xf4f7fa color_empty_focus: #xf4f7fa}}
    }

    mod.widgets.PanelSlider = mod.widgets.PanelControl{kind: PanelKind.Slider}
    mod.widgets.PanelKnob = mod.widgets.PanelControl{kind: PanelKind.Knob width: 76}
    mod.widgets.PanelNumber = mod.widgets.PanelControl{kind: PanelKind.Number}
    mod.widgets.PanelAngle = mod.widgets.PanelControl{kind: PanelKind.Angle width: 76}
    mod.widgets.PanelRange = mod.widgets.PanelControl{kind: PanelKind.Range}
    mod.widgets.PanelXY = mod.widgets.PanelControl{kind: PanelKind.Xy}
    mod.widgets.PanelVec3 = mod.widgets.PanelControl{kind: PanelKind.Vec3}
    mod.widgets.PanelToggle = mod.widgets.PanelControl{kind: PanelKind.Toggle}
    mod.widgets.PanelChoice = mod.widgets.PanelControl{kind: PanelKind.Choice}
    mod.widgets.PanelSeed = mod.widgets.PanelControl{kind: PanelKind.Seed}
    mod.widgets.PanelEase = mod.widgets.PanelControl{kind: PanelKind.Ease}
    mod.widgets.PanelFader = mod.widgets.PanelControl{kind: PanelKind.Fader width: 60}
    /** Controls side by side (knobs, faders, dials). */
    mod.widgets.PanelRow = View{width: Fill height: Fit flow: Right spacing: 6}
}

/// Which form a control takes.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum PanelKind {
    #[pick]
    Slider = 0,
    Knob = 1,
    Number = 2,
    Angle = 3,
    Range = 4,
    Xy = 5,
    Vec3 = 6,
    Toggle = 7,
    Choice = 8,
    Seed = 9,
    Ease = 10,
    Fader = 11,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPanelShape {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub fill: Vec4f,
    #[live]
    pub edge: Vec4f,
    #[live]
    pub stroke: f32,
    #[live]
    pub radius: f32,
    #[live]
    pub value: f32,
    /// A second value: what modulates it reaches (a ring, a span).
    #[live]
    pub value2: f32,
    #[live]
    pub shape: f32,
    /// The control's body (a knob's face, a thumb's edge).
    #[live]
    pub body: Vec4f,
}

/// The panel's corner radii: a control (a well, a chip, a button), a group's
/// inset, a module's card.
pub const R_CONTROL: f32 = 4.0;
pub const R_GROUP: f32 = 5.0;
pub const R_CARD: f32 = 6.0;

impl DrawPanelShape {
    /// A rounded box filled with `fill`, its corners `radius`.
    pub fn fill_round(&mut self, cx: &mut Cx2d, r: Rect, fill: Vec4f, radius: f32) {
        self.edged_round(cx, r, fill, fill, 0.0, radius);
    }

    /// A rounded box with an edge `stroke` wide.
    pub fn edged_round(&mut self, cx: &mut Cx2d, r: Rect, fill: Vec4f, edge: Vec4f, stroke: f32, radius: f32) {
        self.shape = 0.0;
        self.fill = fill;
        self.edge = edge;
        self.stroke = stroke;
        self.radius = radius;
        self.draw_abs(cx, r);
    }

    /// A chevron in `r`: pointing right (folded) or down (open). Drawn by
    /// the type's own shader (a control's `draw_shape` draws its own forms).
    pub fn chevron(&mut self, cx: &mut Cx2d, r: Rect, ink: Vec4f, open: bool) {
        self.shape = 9.0;
        self.fill = ink;
        self.value = if open { 1.0 } else { 0.0 };
        self.draw_abs(cx, r);
    }
}

/// `text` cut to `width` with an ellipsis (whole when it fits), and whether
/// it was cut.
pub fn fit_text(draw: &DrawText, cx: &mut Cx2d, text: &str, width: f64) -> (String, bool) {
    if advance(draw, cx, text) <= width {
        return (text.to_string(), false);
    }
    let mut out: String = text.to_string();
    while !out.is_empty() && advance(draw, cx, &format!("{}…", out.trim_end())) > width {
        out.pop();
    }
    (format!("{}…", out.trim_end()), true)
}

/// A key button's diamond (`shape` 0) and the box of its note (1).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPanelKey {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub fill: Vec4f,
    #[live]
    pub edge: Vec4f,
    #[live]
    pub shape: f32,
}

/// Draw a key button with its diamond at `at` inside `within` (the
/// control): filled when the value is keyed, hollow when it can be,
/// faint when it cannot; its note while the pointer is on it (why it
/// cannot), or "Remove animation" when offered.
pub fn draw_key(cx: &mut Cx2d, key: &mut host::KeyButton, at: DVec2, within: Rect, shape: &mut DrawPanelKey, text: &mut DrawText) {
    let Some(state) = key.state.clone() else { return };
    let p = makepad_look::palette::get(cx);
    // Quiet: nothing for a value that is not animated (hover it and press
    // I, or right-click it, to animate); an animated one a small curve
    // (a click on it shows its curve).
    if state == Ok(true) {
        key.rect = Rect { pos: at - dvec2(2.0, 2.0), size: dvec2(14.0, 14.0) };
        shape.shape = 2.0;
        shape.fill = color(p.accent, if key.hot { 1.0 } else { 0.8 });
        shape.edge = color(0, 0.0);
        shape.draw_abs(cx, Rect { pos: at, size: dvec2(11.0, 10.0) });
    } else {
        key.rect = Rect::default();
    }
    let note = if key.offering {
        Some(match &state {
            Ok(true) => "Remove animation".to_string(),
            Ok(false) => "Animate (I)".to_string(),
            Err(why) => why.clone(),
        })
    } else if key.hot && state == Ok(true) {
        Some("Animated: its curve (I keys, Alt+I unkeys)".to_string())
    } else {
        None
    };
    let Some(mut note) = note else { return };
    let mut w = advance(text, cx, &note) + 12.0;
    if key.offering && w > within.size.x {
        note = "Remove".to_string();
        w = (advance(text, cx, &note) + 12.0).min(within.size.x);
    }
    let right = at.x + 14.0;
    // An offer sits at the label column's end (the label stays readable).
    let x = if key.offering { (within.pos.x + within.size.x - w).max(within.pos.x) } else { right };
    let r = Rect { pos: dvec2(x, at.y - 4.0), size: dvec2(w, 18.0) };
    if key.offering {
        key.remove = r;
    }
    // Drawn by the module, over its neighbours; else here.
    if host::tip(r, note.clone(), key.offering) {
        return;
    }
    shape.shape = 1.0;
    shape.fill = color(if key.offering { p.raised } else { p.menu }, 1.0);
    shape.edge = color(p.line, 1.0);
    shape.draw_abs(cx, r);
    text.new_draw_call(cx);
    text.color = color(if key.offering { p.text_hi } else { p.text }, 1.0);
    text.draw_abs(cx, dvec2(r.pos.x + 6.0, r.pos.y + 4.0), &note);
}

/// What a control says to its host page (when it has no path).
#[derive(Clone, Debug, Default)]
pub enum PanelControlAction {
    #[default]
    None,
    /// The value moved (each step) and whether the gesture ended.
    Changed { value: PanelValue, done: bool },
}

/// The named eases a control offers, as Motion names them.
const EASES: [&str; 8] = ["linear", "ease_in", "ease_out", "ease_in_out", "ease_out_cubic", "ease_in_out_cubic", "ease_out_back", "ease_out_elastic"];

fn ease_at(name: &str, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match name {
        "ease_in" => t * t,
        "ease_out" => 1.0 - (1.0 - t) * (1.0 - t),
        "ease_in_out" => if t < 0.5 { 2.0 * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(2) / 2.0 },
        "ease_out_cubic" => 1.0 - (1.0 - t).powi(3),
        "ease_in_out_cubic" => if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 },
        "ease_out_back" => {
            let (c1, c3) = (1.70158, 2.70158);
            1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
        }
        "ease_out_elastic" => {
            if t == 0.0 || t == 1.0 { t } else { 2f64.powf(-10.0 * t) * ((t * 10.0 - 0.75) * (2.0 * std::f64::consts::PI / 3.0)).sin() + 1.0 }
        }
        _ => t,
    }
}

const EASE_H: f64 = 22.0;
const EASE_GAP: f64 = 4.0;

/// The eases' grid in `width`: per row, rows, and each cell's width. All in
/// one row when a cell keeps 30 px, else rows of four (or as many as fit).
fn ease_grid(width: f64, count: usize) -> (usize, usize, f64) {
    let count = count.max(1);
    let cell = |n: usize| (width - EASE_GAP * (n as f64 - 1.0)) / n as f64;
    let n = if cell(count) >= 30.0 { count } else { (1..=4.min(count)).rev().find(|&n| cell(n) >= 26.0).unwrap_or(1) };
    (n, count.div_ceil(n), cell(n).max(10.0))
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

/// A part of a control under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    /// The graphic of a value (a knob, a fader, a dial, a pad).
    Value(usize),
    /// Value field `i` (the number, a range's end, a vector's part, a
    /// knob's readout).
    Field(usize),
    /// A field's step arrow: field `i`, -1 or +1.
    Step(usize, i8),
    /// Choice / ease `i`.
    Pick(usize),
    /// A seed's re-roll.
    Reroll,
    Toggle,
}

impl Part {
    /// The field a part belongs to, if it is one.
    fn field(self) -> Option<usize> {
        match self {
            Part::Field(i) | Part::Step(i, _) => Some(i),
            _ => None,
        }
    }
}

const ROW: f64 = 24.0;
/// A value field's height.
const FIELD_H: f64 = ROW - 4.0;
/// A field's text inset from its ends (and the step arrows' zones).
const FIELD_PAD: f64 = 7.0;
const ARROW_W: f64 = 14.0;
/// The label column: every row's label is cut to it (an ellipsis, the whole
/// label on hover) and its control starts after it.
pub const LABEL: f64 = 104.0;
/// Between the label column and the control.
const GAP: f64 = 8.0;
/// How far a press moves before it is a drag (less: a click, which types).
const DRAG_THRESHOLD: f64 = 3.0;

/// A press on a control, and the drag it may become.
#[derive(Clone, Debug)]
struct Press {
    part: Part,
    origin: DVec2,
    /// The numbers when it began (Escape puts them back).
    start: Vec<f64>,
    /// Past the threshold: a drag (else a click on release).
    engaged: bool,
    /// The numbers it moves (a vector's part, or all of them).
    comps: Vec<usize>,
    /// Where the drag measures from (moved when Shift changes or a hard
    /// limit is met), the numbers there, and where they are now unsnapped.
    anchor_at: f64,
    anchor: Vec<f64>,
    raw: Vec<f64>,
    fine: bool,
    /// Value per point of travel (fixed for the gesture), and the smallest
    /// step a value is written at.
    rate: f64,
    quantum: f64,
    /// Moved up or down first on a single field: picking the fields a
    /// sideways drag then moves together (a multi-edit), and those fields.
    selecting: bool,
    members: Vec<Member>,
}

/// Another field moved by a multi-edit: by the same part of its own soft
/// range as the field dragged.
#[derive(Clone, Debug)]
struct Member {
    uid: u64,
    path: String,
    code: String,
    span: f64,
    hard: (Option<f64>, Option<f64>),
    whole: bool,
    step: f64,
    anchor: f64,
    raw: f64,
}

/// A value field as it last drew: where (Tab's order, a multi-edit's
/// reach) and what it holds (what a multi-edit writes).
#[derive(Clone, Debug)]
struct FieldPlace {
    uid: u64,
    rect: Rect,
    /// Its first field's rect.
    field: Rect,
    fields: usize,
    area: Area,
    redraw: u64,
    path: String,
    code: String,
    numbers: Vec<f64>,
    soft: (f64, f64),
    hard: (Option<f64>, Option<f64>),
    whole: bool,
    step: f64,
}

/// A field's context menu item.
#[derive(Clone, Copy, Debug, PartialEq)]
enum MenuAct {
    Type,
    Reset,
    Copy,
    Paste,
    Animate,
    InsertKey,
    RemoveKey,
    RemoveAnimation,
    ShowCurve,
    /// Said, not done (why it cannot be animated).
    Note,
}

#[derive(Clone, Debug)]
struct Menu {
    /// Where it hangs, from the control's origin.
    offset: DVec2,
    /// Each item: what it says, its key, what it does.
    items: Vec<(String, &'static str, MenuAct)>,
    /// Each item's rect from the menu's origin, as last drawn.
    rects: Vec<Rect>,
    size: DVec2,
    hot: Option<usize>,
    /// The field it was opened on.
    comp: usize,
}

const MENU_ROW: f64 = 22.0;

thread_local! {
    /// The value field under the pointer (keys over a field act on it).
    static HOVERED: Cell<Option<u64>> = const { Cell::new(None) };
    /// The panel control that last took the keyboard, and its area as it
    /// last drew (an area is renewed by each redraw).
    static FOCUSED: Cell<Option<(u64, Area)>> = const { Cell::new(None) };
    /// Fields as drawn: uid, rect, how many fields, area, redraw: Tab's order.
    static FIELDS: RefCell<Vec<FieldPlace>> = const { RefCell::new(Vec::new()) };
    /// The fields a multi-edit holds now (drawn picked).
    static MULTI: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    /// A field asked to start typing (Tab from the one before it).
    static NEXT_EDIT: Cell<Option<(u64, usize)>> = const { Cell::new(None) };
    /// A value copied in the panel (also put on the system clipboard).
    static CLIP: RefCell<Option<String>> = const { RefCell::new(None) };
}

#[derive(Script, ScriptHook, Widget)]
pub struct PanelControl {
    /// When in the film it acts ("beats 9-16"): the host reads it from the
    /// panel's code.
    #[live]
    pub when: String,
    #[rust]
    place: Area,
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    /// Under the whole control (what the pointer hits).
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_shape: DrawPanelShape,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_value: DrawText,
    /// The key button (numbers and vectors).
    #[live]
    draw_key: DrawPanelKey,
    /// The context menu's box and words (drawn over the panel).
    #[live]
    draw_menu: DrawPanelShape,
    #[live]
    draw_menu_text: DrawText,
    /// What a field types into, while it is typed in.
    #[live]
    input: TextInput,
    #[rust]
    menu_list: Option<DrawList2d>,
    #[rust]
    menu: Option<Menu>,
    #[rust]
    key: KeyButton,
    /// The playhead a keyed value was last read at.
    #[rust]
    read_at: f64,
    /// The path in the scene's code it is bound to.
    #[live]
    pub path: String,
    /// What it is called (else the path's last part).
    #[live]
    pub label: String,
    #[live]
    pub kind: PanelKind,
    /// The soft range a number starts with (both 0: made up from the value).
    /// It sets the fill and the drag's speed, and grows past a value.
    #[live]
    pub min: f64,
    #[live]
    pub max: f64,
    /// Hard limits, only where the value's domain has them (a radius: 0).
    #[live]
    pub hard_min: Option<f64>,
    #[live]
    pub hard_max: Option<f64>,
    /// The step a value moves on (0: made up from the range; a number
    /// written whole stays whole).
    #[live]
    pub step: f64,
    /// The choices of a Choice, separated by spaces or commas (else the
    /// eases for an Ease).
    #[live]
    pub choices: String,
    /// The value shown without a path (a number, or code as written).
    #[live]
    pub value: String,
    /// Called on each change with the new value.
    #[live]
    on_change: ScriptFnRef,
    /// Called when a gesture ends with the value it left.
    #[live]
    on_commit: ScriptFnRef,
    #[live(true)]
    #[visible]
    visible: bool,

    /// The value as written (the form new values are written in), its
    /// numbers, and the one found when the panel opened (reset).
    #[rust]
    code: String,
    #[rust]
    numbers: Vec<f64>,
    #[rust]
    original: Option<String>,
    /// The soft range now (grown past every value it has held).
    #[rust]
    soft: Option<(f64, f64)>,
    /// Which change of the scene it last read.
    #[rust]
    seen: u64,
    #[rust]
    spots: Vec<(Rect, Part)>,
    /// Where the control's rect was when it drew its parts.
    #[rust]
    drawn_at: DVec2,
    #[rust]
    press: Option<Press>,
    #[rust]
    hot: Option<Part>,
    #[rust]
    focus: usize,
    #[rust]
    pastes: Vec<(u64, usize, String, String)>,
    /// The field being typed in, whether the input still needs the
    /// keyboard, and what was wrong with the last entry.
    #[rust]
    editing: Option<usize>,
    #[rust]
    edit_focus: bool,
    #[rust]
    edit_error: Option<String>,
    /// The label as drawn: its rect, and the whole label when it was cut.
    #[rust]
    label_cut: Option<(Rect, String)>,
    #[rust]
    label_hot: bool,
    /// The pointer is over the control (I and Alt+I key it).
    #[rust]
    hovered: bool,
}

impl PanelControl {
    fn read(&mut self) {
        let rev = host::revision();
        // A keyed value is its value at the playhead.
        let head = if self.key.keyed() { host::playhead().map(|p| p.0) } else { None };
        let moved = head.is_some_and(|t| t != self.read_at);
        if let Some(t) = head {
            self.read_at = t;
        }
        if self.seen == rev && !self.code.is_empty() && !moved {
            return;
        }
        self.seen = rev;
        let found = host::read(&self.path).map(|v| v.code()).or_else(|| (!self.value.is_empty()).then(|| self.value.clone()));
        let Some(code) = found else { return };
        // A gesture holds its own value.
        if self.press.as_ref().is_some_and(|p| p.engaged) {
            return;
        }
        self.numbers = numbers_in(&code);
        if self.original.is_none() {
            self.original = Some(code.clone());
        }
        self.code = code;
        self.grow_soft();
    }

    fn unit(&self) -> &str {
        host::number_with_unit(&self.code).map(|(_, u)| u).map_or("", |u| match u.as_str() {
            "s" => "s",
            "ms" => "ms",
            "deg" => "deg",
            "rad" => "rad",
            "px" => "px",
            _ => "",
        })
    }

    /// The range a number starts with: `min`/`max` when given, else made
    /// up from the value and its unit.
    fn base_range(&self) -> (f64, f64) {
        if self.max > self.min {
            return (self.min, self.max);
        }
        let v = self.original.as_deref().map(numbers_in).and_then(|n| n.first().copied()).unwrap_or(0.0);
        match self.unit() {
            "deg" => if v < 0.0 { (-180.0, 180.0) } else { (0.0, 360.0) },
            "rad" => (-std::f64::consts::PI, std::f64::consts::PI),
            _ if self.kind == PanelKind::Angle => (0.0, 360.0),
            _ if (0.0..=1.0).contains(&v) => (0.0, 1.0),
            _ if v < 0.0 => (-2.0 * v.abs(), 2.0 * v.abs()),
            _ => (0.0, field::nice_ceil((2.0 * v).max(1.0))),
        }
    }

    /// The soft range now.
    fn range(&self) -> (f64, f64) {
        self.soft.unwrap_or_else(|| self.base_range())
    }

    /// The soft range grown to hold every number the value has.
    fn grow_soft(&mut self) {
        if !self.numeric() {
            return;
        }
        let mut r = self.range();
        for &v in &self.numbers {
            r = field::stretch(r, v);
        }
        // Inside the hard limits.
        if let Some(lo) = self.hard_min {
            r.0 = r.0.max(lo);
        }
        if let Some(hi) = self.hard_max {
            r.1 = r.1.min(hi);
        }
        if r.1 <= r.0 {
            r.1 = r.0 + 1.0;
        }
        self.soft = Some(r);
    }

    fn hard(&self) -> (Option<f64>, Option<f64>) {
        (self.hard_min, self.hard_max)
    }

    /// A count written whole (`count: 16`, `seed: 17`) stays whole.
    fn whole(&self) -> bool {
        self.original.as_deref().is_some_and(|o| !o.contains('.') && !numbers_in(o).is_empty() && self.kind != PanelKind::Angle && self.unit() != "s")
    }

    /// One step of the arrows, the wheel and the arrow keys.
    fn step(&self) -> f64 {
        if self.step > 0.0 {
            return self.step;
        }
        let (lo, hi) = self.range();
        field::increment(hi - lo, self.whole())
    }

    fn decimals(&self) -> usize {
        field::decimals_of(self.step())
    }

    /// Decimals a value is shown with: its step's, or more when the code
    /// writes more (a typed 0.125 shows as 0.125).
    fn shown_decimals(&self) -> usize {
        let code_decimals = self.code.split(|c: char| !(c.is_ascii_digit() || c == '.')).map(field::written_decimals).max().unwrap_or(0);
        field::decimals_of(self.step()).max(code_decimals).min(6)
    }

    /// Whether the value is numbers (a field kind).
    fn numeric(&self) -> bool {
        !matches!(self.kind, PanelKind::Toggle | PanelKind::Choice | PanelKind::Ease)
    }

    fn choice_list(&self) -> Vec<String> {
        if self.kind == PanelKind::Ease && self.choices.trim().is_empty() {
            return EASES.iter().map(|e| e.to_string()).collect();
        }
        self.choices.split([' ', ',']).map(str::trim).filter(|c| !c.is_empty()).map(str::to_string).collect()
    }

    /// The current choice (a name without its `@`, a string's words).
    fn chosen(&self) -> String {
        self.code.trim().trim_start_matches('@').trim_matches('"').to_string()
    }

    /// Write `code` as the value: set at the path (or said as an action),
    /// and the script's callbacks told.
    fn emit(&mut self, cx: &mut Cx, code: String, done: bool) {
        self.numbers = numbers_in(&code);
        self.code = code.clone();
        self.grow_soft();
        // Written as the control writes it (at its step's decimals), not
        // re-formatted by the host: a number goes as code too.
        let value = match code.trim() {
            "true" => PanelValue::Bool(true),
            "false" => PanelValue::Bool(false),
            _ => PanelValue::Code(code.clone()),
        };
        if host::hosted() && !self.path.is_empty() {
            host::set(&self.path, value.clone());
        } else if !host::bound(&self.on_change) {
            cx.widget_action(self.uid, PanelControlAction::Changed { value: value.clone(), done });
        }
        let arg = match &value {
            PanelValue::Code(c) if c.trim().parse::<f64>().is_ok() => ScriptValue::from_f64(c.trim().parse::<f64>().unwrap_or(0.0)),
            PanelValue::Number(v) => ScriptValue::from_f64(*v),
            PanelValue::Bool(b) => ScriptValue::from_bool(*b),
            PanelValue::Code(c) => cx.with_vm(|vm| vm.bx.heap_mut().new_string_from_str(c)),
        };
        if host::bound(&self.on_change) {
            cx.widget_to_script_call(self.uid, NIL, self.source.clone(), self.on_change.clone(), &[arg]);
        }
        if done && host::bound(&self.on_commit) {
            cx.widget_to_script_call(self.uid, NIL, self.source.clone(), self.on_commit.clone(), &[arg]);
        }
        self.draw_bg.redraw(cx);
    }

    /// The value with its numbers set to `values` (each inside the hard
    /// limits; a whole count stays whole), written at `decimals`.
    fn emit_values(&mut self, cx: &mut Cx, values: Vec<f64>, decimals: usize, done: bool) {
        let whole = self.whole();
        let hard = self.hard();
        let values: Vec<f64> = values.into_iter().map(|v| {
            let v = field::clamp_hard(v, hard);
            if whole { v.round() } else { v }
        }).collect();
        let decimals = if whole { 0 } else { decimals };
        let template = if self.code.is_empty() { fmt_number(values.first().copied().unwrap_or(0.0), 4) } else { self.code.clone() };
        let code = if numbers_in(&template).is_empty() { fmt_number(values.first().copied().unwrap_or(0.0), decimals) } else { with_numbers(&template, &values, decimals) };
        self.emit(cx, code, done);
    }

    /// The value with its numbers moved to `values`, on the step.
    fn emit_numbers(&mut self, cx: &mut Cx, values: Vec<f64>, done: bool) {
        let step = self.step();
        let values = values.into_iter().map(|v| field::snap(v, step)).collect();
        let d = self.decimals();
        self.emit_values(cx, values, d, done);
    }

    fn reset(&mut self, cx: &mut Cx) {
        if let Some(original) = self.original.clone() {
            self.emit(cx, original, true);
        }
    }

    /// Whether it is a value keys can move (numbers, vectors).
    fn animatable(&self) -> bool {
        self.numeric()
    }

    /// `abs` where the control drew its parts: a layout that aligns the
    /// control after it drew (a row aligned right or centred) moves it,
    /// not the rects it recorded.
    fn local(&self, cx: &Cx, abs: DVec2) -> DVec2 {
        let now = self.draw_bg.area().rect(cx).pos;
        abs - (now - self.drawn_at)
    }

    fn part_at(&self, cx: &Cx, abs: DVec2) -> Option<Part> {
        let abs = self.local(cx, abs);
        self.spots.iter().rev().find(|(r, _)| r.contains(abs)).map(|(_, p)| *p)
    }

    fn spot_of(&self, part: Part) -> Option<Rect> {
        self.spots.iter().find(|(_, p)| *p == part).map(|(r, _)| *r)
    }

    /// How many fields the value has (a vector's parts, a range's ends).
    fn field_count(&self) -> usize {
        match self.kind {
            PanelKind::Vec3 => self.numbers.len().clamp(2, 3),
            PanelKind::Range | PanelKind::Xy => 2,
            PanelKind::Toggle | PanelKind::Choice | PanelKind::Ease => 0,
            _ => 1,
        }
    }

    // ------------------------------------------------------------ dragging

    /// The press's travel along its axis: sideways in a field, up (and a
    /// little sideways) on a knob, up on a fader.
    fn axis(&self, part: Part, abs: DVec2, origin: DVec2) -> f64 {
        match (self.kind, part) {
            (PanelKind::Knob, Part::Value(_)) => (origin.y - abs.y) + (abs.x - origin.x) * 0.25,
            (PanelKind::Fader, Part::Value(_)) => origin.y - abs.y,
            _ => abs.x - origin.x,
        }
    }

    /// A drag step with the pointer at `abs` (`done`: the release).
    fn drag_to(&mut self, cx: &mut Cx, abs: DVec2, mods: KeyModifiers, done: bool) {
        let Some(mut pr) = self.press.take() else { return };
        match (self.kind, pr.part) {
            // A dial points where the pointer is, a pad puts its puck there.
            (PanelKind::Angle, Part::Value(_)) | (PanelKind::Xy, Part::Value(_)) => {
                self.absolute_to(cx, pr.part, abs, done);
                self.press = if done { None } else { Some(pr) };
                return;
            }
            _ => {}
        }
        let at = self.axis(pr.part, abs, pr.origin);
        if !pr.engaged {
            if (abs - pr.origin).length() < DRAG_THRESHOLD {
                self.press = Some(pr);
                return;
            }
            let d = abs - pr.origin;
            // Up or down first on a one-number field: pick the fields the
            // pointer passes over; the sideways drag after moves them all.
            if !pr.selecting && self.field_count() == 1 && pr.part.field().is_some() && !self.path.is_empty() && d.y.abs() > d.x.abs() {
                pr.selecting = true;
            }
            if pr.selecting {
                self.pick_members(cx, &mut pr, abs);
                if (abs.x - pr.origin.x).abs() < 2.0 * DRAG_THRESHOLD {
                    self.press = Some(pr);
                    return;
                }
                pr.selecting = false;
            }
            pr.engaged = true;
            let n = self.numbers.len().max(1);
            let d = abs - pr.origin;
            // Moved up or down first on a vector, or Alt: all its parts.
            let all = self.field_count() > 1 && (mods.alt || (pr.part.field().is_some() && d.y.abs() > d.x.abs()));
            pr.comps = if all { (0..n).collect() } else { vec![pr.part.field().unwrap_or(0).min(n - 1)] };
            pr.anchor_at = at;
            pr.anchor = pr.start.clone();
            pr.raw = pr.start.clone();
            pr.fine = mods.shift;
            self.set_rate(&mut pr);
            // The pointer hides where it is and never runs out of screen.
            if pr.part.field().is_some() {
                cx.pin_pointer_capture();
            }
            self.draw_bg.redraw(cx);
        }
        if mods.shift != pr.fine {
            // Shift re-anchors: the value holds still, only the speed changes.
            pr.fine = mods.shift;
            pr.anchor = pr.raw.clone();
            pr.anchor_at = at;
            for m in &mut pr.members {
                m.anchor = m.raw;
            }
            self.set_rate(&mut pr);
        }
        let delta = (at - pr.anchor_at) * pr.rate;
        let mut values = if self.numbers.is_empty() { vec![self.range().0] } else { self.numbers.clone() };
        if values.len() < pr.start.len() {
            values.resize(pr.start.len(), 0.0);
        }
        let (lo, hi) = self.range();
        let snap = if mods.control || mods.logo { field::snap_rung(hi - lo, mods.shift) } else { pr.quantum };
        let hard = self.hard();
        for &c in &pr.comps {
            if c >= values.len() || c >= pr.anchor.len() {
                continue;
            }
            let raw = pr.anchor[c] + delta;
            let clamped = field::clamp_hard(raw, hard);
            if clamped != raw {
                // At a hard limit the drag measures from here: turning back
                // moves the value at once.
                pr.anchor[c] = clamped;
                for k in 0..pr.anchor.len() {
                    if k != c && pr.comps.contains(&k) {
                        pr.anchor[k] = pr.raw.get(k).copied().unwrap_or(pr.anchor[k]);
                    }
                }
                pr.anchor_at = at;
            }
            pr.raw[c] = clamped;
            values[c] = field::snap(clamped, snap);
        }
        let decimals = field::decimals_of(snap).max(if self.step > 0.0 { field::decimals_of(self.step) } else { 0 });
        if !pr.members.is_empty() {
            // The others move by the same part of their own soft ranges.
            let frac = delta / (hi - lo).abs().max(1e-9);
            let ctrl = mods.control || mods.logo;
            for m in &mut pr.members {
                let raw = field::clamp_hard(m.anchor + frac * m.span, m.hard);
                m.raw = raw;
                let q = if ctrl {
                    field::snap_rung(m.span, mods.shift)
                } else if m.step > 0.0 {
                    m.step
                } else if m.whole {
                    1.0
                } else {
                    10f64.powf((pr.rate / (hi - lo).abs().max(1e-9) * m.span).max(1e-9).log10().floor())
                };
                let v = if m.whole { field::snap(raw, q).round() } else { field::snap(raw, q) };
                let code = with_numbers(&m.code, &[v], if m.whole { 0 } else { field::decimals_of(q) });
                host::set(&m.path, PanelValue::Code(code));
            }
        }
        self.emit_values(cx, values, decimals, done);
        self.press = if done { None } else { Some(pr) };
    }

    /// A multi-edit's fields: every one-number field the pointer has passed
    /// over, straight above or below where the press began.
    fn pick_members(&mut self, cx: &mut Cx, pr: &mut Press, abs: DVec2) {
        let me = self.widget_uid().0;
        let (y0, y1) = (pr.origin.y.min(abs.y), pr.origin.y.max(abs.y));
        let x = pr.origin.x;
        let picked: Vec<FieldPlace> = FIELDS.with(|f| {
            let f = f.borrow();
            let latest = f.iter().map(|e| e.redraw).max().unwrap_or(0);
            f.iter()
                .filter(|e| e.uid != me && e.redraw == latest && e.fields == 1 && e.numbers.len() == 1 && !e.path.is_empty())
                .filter(|e| e.field.pos.x <= x && x <= e.field.pos.x + e.field.size.x && e.field.pos.y <= y1 && e.field.pos.y + e.field.size.y >= y0)
                .cloned()
                .collect()
        });
        let was: Vec<u64> = pr.members.iter().map(|m| m.uid).collect();
        pr.members = picked
            .iter()
            .map(|e| Member { uid: e.uid, path: e.path.clone(), code: e.code.clone(), span: e.soft.1 - e.soft.0, hard: e.hard, whole: e.whole, step: e.step, anchor: e.numbers[0], raw: e.numbers[0] })
            .collect();
        let now: Vec<u64> = pr.members.iter().map(|m| m.uid).collect();
        if now != was {
            MULTI.with(|m| *m.borrow_mut() = if now.is_empty() { Vec::new() } else { std::iter::once(me).chain(now.iter().copied()).collect() });
            for e in &picked {
                cx.redraw_area(e.area);
            }
            self.draw_bg.redraw(cx);
            // (Those no longer picked draw unpicked too.)
            FIELDS.with(|f| {
                for e in f.borrow().iter().filter(|e| was.contains(&e.uid)) {
                    cx.redraw_area(e.area);
                }
            });
        }
    }

    /// A multi-edit ended: no field is drawn picked.
    fn clear_multi(&mut self, cx: &mut Cx) {
        let had: Vec<u64> = MULTI.with(|m| std::mem::take(&mut *m.borrow_mut()));
        if had.is_empty() {
            return;
        }
        FIELDS.with(|f| {
            for e in f.borrow().iter().filter(|e| had.contains(&e.uid)) {
                cx.redraw_area(e.area);
            }
        });
    }

    /// The speed and the smallest step of a drag: the field's width is the
    /// soft range on a slider, 400 points of it on a number; a knob turns
    /// the range in 160 points, a fader in its travel.
    fn set_rate(&self, pr: &mut Press) {
        let (lo, hi) = self.range();
        let span = hi - lo;
        let width = pr.part.field().and_then(|i| self.spot_of(Part::Field(i))).map_or(200.0, |r| r.size.x);
        pr.rate = match (self.kind, pr.part) {
            (PanelKind::Knob, Part::Value(_)) => span / 160.0 * if pr.fine { 0.1 } else { 1.0 },
            (PanelKind::Fader, Part::Value(_)) => span / 108.0 * if pr.fine { 0.1 } else { 1.0 },
            (PanelKind::Slider | PanelKind::Range, _) => field::drag_rate(span, width, true, pr.fine),
            _ => field::drag_rate(span, width, false, pr.fine),
        };
        pr.quantum = if self.step > 0.0 {
            self.step
        } else if self.whole() {
            1.0
        } else {
            // A point's worth, as a round decimal.
            10f64.powf(pr.rate.max(1e-9).log10().floor())
        };
    }

    /// A dial or a pad: the value is where the pointer is.
    fn absolute_to(&mut self, cx: &mut Cx, part: Part, abs: DVec2, done: bool) {
        let Some(rect) = self.spot_of(part) else { return };
        let abs = self.local(cx, abs);
        let (lo, hi) = self.range();
        let mut values = if self.numbers.is_empty() { vec![lo] } else { self.numbers.clone() };
        if self.kind == PanelKind::Angle {
            let c = rect.pos + rect.size * 0.5;
            let a = (abs.y - c.y).atan2(abs.x - c.x).to_degrees();
            let a = if self.unit() == "rad" { a.to_radians() } else { (a + 360.0) % 360.0 };
            values[0] = if lo < 0.0 && self.unit() != "rad" && a > 180.0 { a - 360.0 } else { a };
        } else {
            let tx = ((abs.x - rect.pos.x) / rect.size.x).clamp(0.0, 1.0);
            let ty = 1.0 - ((abs.y - rect.pos.y) / rect.size.y).clamp(0.0, 1.0);
            if values.len() < 2 {
                values.resize(2, lo);
            }
            values[0] = lo + (hi - lo) * tx;
            values[1] = lo + (hi - lo) * ty;
        }
        self.emit_numbers(cx, values, done);
    }

    /// Escape or a right click in a drag: the value it started with.
    fn cancel_drag(&mut self, cx: &mut Cx) {
        self.clear_multi(cx);
        if let Some(pr) = self.press.take() {
            for m in &pr.members {
                host::set(&m.path, PanelValue::Code(m.code.clone()));
            }
            if pr.engaged {
                cx.unpin_pointer_capture();
                let d = self.shown_decimals();
                self.emit_values(cx, pr.start, d.max(6), false);
            }
            host::hold(false);
        }
    }

    // ------------------------------------------------------------ typing

    /// Field `i` becomes a text box holding its number.
    fn begin_edit(&mut self, cx: &mut Cx, i: usize) {
        if !self.numeric() || i >= self.field_count().max(1) {
            return;
        }
        self.press = None;
        self.menu = None;
        self.editing = Some(i);
        self.edit_focus = true;
        self.edit_error = None;
        let v = self.numbers.get(i).copied().unwrap_or(0.0);
        let unit = if self.field_count() == 1 { self.unit().to_string() } else { String::new() };
        let text = format!("{}{unit}", fmt_number(v, 6));
        self.input.set_text(cx, &text);
        self.draw_bg.redraw(cx);
    }

    /// The typed entry: its value written (one undo step), or what is
    /// wrong with it (it stays open).
    fn commit_edit(&mut self, cx: &mut Cx, text: &str) -> bool {
        let Some(i) = self.editing else { return true };
        let current = self.numbers.get(i).copied().unwrap_or(0.0);
        let unit = self.unit().to_string();
        match field::eval(text, current, &unit) {
            Ok(v) => {
                let mut values = if self.numbers.is_empty() { vec![0.0] } else { self.numbers.clone() };
                if i >= values.len() {
                    values.resize(i + 1, 0.0);
                }
                values[i] = v;
                self.editing = None;
                self.edit_error = None;
                self.emit_values(cx, values, 6, true);
                true
            }
            Err(why) => {
                self.edit_error = Some(why);
                self.draw_bg.redraw(cx);
                false
            }
        }
    }

    fn end_edit(&mut self, cx: &mut Cx) {
        self.editing = None;
        self.edit_error = None;
        self.edit_focus = false;
        cx.set_key_focus(self.draw_bg.area());
        FOCUSED.with(|f| f.set(Some((self.widget_uid().0, self.draw_bg.area()))));
        self.draw_bg.redraw(cx);
    }

    /// Tab: the next field (this control's next part, else the next
    /// control's first), Shift+Tab the one before.
    fn tab(&mut self, cx: &mut Cx, back: bool) {
        let Some(i) = self.editing else { return };
        let text = self.input.text();
        if !self.commit_edit(cx, &text) {
            return;
        }
        self.end_edit(cx);
        let n = self.field_count();
        if !back && i + 1 < n {
            self.begin_edit(cx, i + 1);
            return;
        }
        if back && i > 0 {
            self.begin_edit(cx, i - 1);
            return;
        }
        let me = self.widget_uid().0;
        let next = FIELDS.with(|f| {
            let f = f.borrow();
            let latest = f.iter().map(|e| e.redraw).max().unwrap_or(0);
            let mut live: Vec<&FieldPlace> = f.iter().filter(|e| e.redraw == latest && e.fields > 0).collect();
            live.sort_by(|a, b| a.rect.pos.y.total_cmp(&b.rect.pos.y).then(a.rect.pos.x.total_cmp(&b.rect.pos.x)));
            let at = live.iter().position(|e| e.uid == me)?;
            let k = if back { at.checked_sub(1)? } else { at + 1 };
            live.get(k).map(|e| (e.uid, if back { e.fields - 1 } else { 0 }, e.area))
        });
        if let Some((uid, comp, area)) = next {
            NEXT_EDIT.with(|n| n.set(Some((uid, comp))));
            cx.redraw_area(area);
        }
    }

    // ------------------------------------------------------------ keys and menu

    fn key_step(&mut self, cx: &mut Cx, dir: f64, fine: bool) {
        match self.kind {
            PanelKind::Choice | PanelKind::Ease => {
                let list = self.choice_list();
                if list.is_empty() {
                    return;
                }
                let at = list.iter().position(|c| *c == self.chosen()).unwrap_or(0) as i64;
                let n = list.len() as i64;
                self.pick(cx, ((at + dir as i64).rem_euclid(n)) as usize);
            }
            PanelKind::Toggle => {
                let on = self.code.trim() == "true";
                self.emit(cx, (!on).to_string(), true);
            }
            _ => self.step_field(cx, self.focus, dir, fine),
        }
    }

    /// Field `i` one step up or down (a tenth of one with `fine`).
    fn step_field(&mut self, cx: &mut Cx, i: usize, dir: f64, fine: bool) {
        let step = self.step() * if fine { 0.1 } else { 1.0 };
        let mut values = self.numbers.clone();
        if values.is_empty() {
            values.push(self.range().0);
        }
        let i = i.min(values.len() - 1);
        values[i] = field::snap(values[i] + dir * step, step);
        let d = field::decimals_of(step).max(self.shown_decimals());
        self.emit_values(cx, values, d, true);
    }

    fn negate(&mut self, cx: &mut Cx) {
        if !self.numeric() || self.numbers.is_empty() {
            return;
        }
        let values: Vec<f64> = self.numbers.iter().map(|v| -v).collect();
        let d = self.shown_decimals().max(6);
        self.emit_values(cx, values, d, true);
    }

    fn copy(&mut self, cx: &mut Cx) {
        let code = self.code.clone();
        cx.copy_to_clipboard(&code);
        CLIP.with(|c| *c.borrow_mut() = Some(code));
    }

    /// A copied value written here, when it fits: as many numbers (or one
    /// number for a vector's part), a bool on a toggle, a listed choice.
    fn paste(&mut self, cx: &mut Cx, comp: usize) {
        let request = cx.read_clipboard();
        self.pastes.push((request, comp, self.path.clone(), self.when.clone()));
    }

    fn paste_text(&mut self, cx: &mut Cx, comp: usize, text: &str) {
        let text = text.trim().to_string();
        match self.kind {
            PanelKind::Toggle => {
                if text == "true" || text == "false" {
                    self.emit(cx, text, true);
                }
            }
            PanelKind::Choice | PanelKind::Ease => {
                let bare = text.trim_start_matches('@').trim_matches('"').to_string();
                if let Some(i) = self.choice_list().iter().position(|c| *c == bare) {
                    self.pick(cx, i);
                }
            }
            _ => {
                let nums = numbers_in(&text);
                if nums.len() == self.numbers.len() && nums.len() > 1 {
                    self.emit_values(cx, nums, 6, true);
                } else {
                    let current = self.numbers.get(comp).copied().unwrap_or(0.0);
                    let unit = self.unit().to_string();
                    if let Ok(v) = field::eval(&text, current, &unit).or_else(|_| nums.first().copied().ok_or(())) {
                        let mut values = if self.numbers.is_empty() { vec![0.0] } else { self.numbers.clone() };
                        let c = comp.min(values.len() - 1);
                        values[c] = v;
                        self.emit_values(cx, values, 6, true);
                    }
                }
            }
        }
    }

    fn open_menu(&mut self, cx: &mut Cx, abs: DVec2, comp: usize) {
        let mut items: Vec<(String, &'static str, MenuAct)> = Vec::new();
        if self.numeric() {
            items.push(("Type a value".into(), "Enter", MenuAct::Type));
        }
        items.push(("Reset to default".into(), "Backspace", MenuAct::Reset));
        items.push(("Copy value".into(), "Ctrl+C", MenuAct::Copy));
        items.push(("Paste value".into(), "Ctrl+V", MenuAct::Paste));
        if self.animatable() {
            match self.key.state.clone() {
                Some(Ok(false)) => items.push(("Animate".into(), "I", MenuAct::Animate)),
                Some(Ok(true)) => {
                    items.push(("Insert key".into(), "I", MenuAct::InsertKey));
                    items.push(("Remove key".into(), "Alt+I", MenuAct::RemoveKey));
                    items.push(("Remove animation".into(), "", MenuAct::RemoveAnimation));
                    items.push(("Show its curve".into(), "", MenuAct::ShowCurve));
                }
                Some(Err(why)) => items.push((why, "", MenuAct::Note)),
                None => {}
            }
        }
        let origin = self.draw_bg.area().rect(cx).pos;
        self.menu = Some(Menu { offset: abs - origin + dvec2(2.0, 2.0), items, rects: Vec::new(), size: DVec2::default(), hot: None, comp });
        cx.sweep_lock(self.draw_bg.area());
        self.draw_bg.redraw(cx);
    }

    fn close_menu(&mut self, cx: &mut Cx) {
        if self.menu.take().is_some() {
            cx.sweep_unlock(self.draw_bg.area());
            self.draw_bg.redraw(cx);
        }
    }

    /// The menu item at `abs`, if any.
    fn menu_item_at(&self, cx: &Cx, abs: DVec2) -> Option<usize> {
        let menu = self.menu.as_ref()?;
        let at = self.draw_bg.area().rect(cx).pos + menu.offset;
        menu.rects.iter().position(|r| Rect { pos: r.pos + at, size: r.size }.contains(abs))
    }

    /// Whether the menu's box holds `abs`.
    fn menu_contains(&self, cx: &Cx, abs: DVec2) -> bool {
        self.menu.as_ref().is_some_and(|m| Rect { pos: self.draw_bg.area().rect(cx).pos + m.offset, size: m.size }.contains(abs))
    }

    fn menu_act(&mut self, cx: &mut Cx, act: MenuAct, comp: usize) {
        match act {
            MenuAct::Type => self.begin_edit(cx, comp),
            MenuAct::Reset => self.reset(cx),
            MenuAct::Copy => self.copy(cx),
            MenuAct::Paste => self.paste(cx, comp),
            MenuAct::Animate | MenuAct::InsertKey => host::animate(&self.path, true),
            MenuAct::RemoveKey => host::animate(&self.path, false),
            MenuAct::RemoveAnimation => host::animate_all(&self.path, false),
            MenuAct::ShowCurve => host::animate(&format!("?{}", self.path), true),
            MenuAct::Note => {}
        }
        self.draw_bg.redraw(cx);
    }

    /// Whether keys pressed now are this control's: the pointer is over it,
    /// or it has the keyboard and the pointer is over no other field; never
    /// while a text box elsewhere has the keyboard.
    fn keys_mine(&self, cx: &Cx) -> bool {
        let focus = cx.key_focus();
        let free = focus == Area::Empty || focus == self.draw_bg.area() || FOCUSED.with(|f| f.get()).is_some_and(|(_, a)| a == focus);
        if !free {
            return false;
        }
        let me = self.widget_uid().0;
        match HOVERED.with(|h| h.get()) {
            Some(uid) => uid == me && self.hovered,
            None => cx.has_key_focus(self.draw_bg.area()),
        }
    }

    fn pick(&mut self, cx: &mut Cx, i: usize) {
        let list = self.choice_list();
        let Some(choice) = list.get(i) else { return };
        let was = self.code.trim();
        let code = if was.starts_with('"') {
            format!("{choice:?}")
        } else if was.starts_with('@') || self.kind == PanelKind::Ease || was.is_empty() {
            format!("@{choice}")
        } else {
            choice.clone()
        };
        self.emit(cx, code, true);
    }

    fn reroll(&mut self, cx: &mut Cx) {
        let seed = (Cx::time_now() * 1000003.0) as u64;
        // A few rounds of a plain hash: a seed in 0..10000.
        let mut x = seed ^ 0x9e3779b97f4a7c15;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51afd7ed558ccd);
        x ^= x >> 33;
        let v = (x % 10000) as f64;
        let template = if self.code.is_empty() { "0".to_string() } else { self.code.clone() };
        let code = with_numbers(&template, &[v], 0);
        self.emit(cx, code, true);
    }

    // ------------------------------------------------------------ drawing

    /// Value field `i` in `r`: its well, the soft range's fill (`fill`), the
    /// label and value inside it, the step arrows on hover (`arrows`), or
    /// the text box while it is typed in.
    #[allow(clippy::too_many_arguments)]
    fn draw_field(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette, r: Rect, i: usize, label: &str, fill: bool, arrows: bool, live: Option<f64>) {
        let hot = self.hot.and_then(Part::field) == Some(i);
        let dragging = self.press.as_ref().is_some_and(|pr| pr.engaged && pr.comps.contains(&i) && pr.part.field().is_some());
        let editing = self.editing == Some(i);
        let me = self.widget_uid().0;
        let picked = i == 0 && MULTI.with(|m| m.borrow().contains(&me));
        let keyed = self.key.keyed();
        let changed = self.original.as_ref().is_some_and(|o| *o != self.code);
        let well = if editing { p.well } else if dragging { p.control_down } else if hot { p.raised } else { p.well };
        let edge = if editing {
            if self.edit_error.is_some() { p.accent } else { p.border_strong }
        } else if picked {
            p.accent
        } else if hot || dragging {
            p.border_hi
        } else {
            p.line
        };
        let well = if keyed && !editing { mix_colour(well, p.amber, 0.14) } else { well };
        self.draw_shape.shape = 0.0;
        self.draw_shape.fill = color(well, 1.0);
        self.draw_shape.edge = color(edge, 1.0);
        self.draw_shape.stroke = 1.0;
        self.draw_shape.radius = R_CONTROL;
        self.draw_shape.draw_abs(cx, r);
        self.spots.push((r, Part::Field(i)));
        let v = self.numbers.get(i).copied().unwrap_or(0.0);
        let (lo, hi) = self.range();
        let t_of = |v: f64| if hi > lo { ((v - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
        if fill && !editing {
            // The value's place in the soft range, from the left.
            let t = t_of(v);
            let fw = (r.size.x - 2.0) * t;
            if fw > 0.5 {
                self.draw_shape.stroke = 0.0;
                self.draw_shape.fill = color(if keyed { p.amber } else { p.accent }, if hot || dragging { 0.42 } else { 0.32 });
                self.draw_shape.edge = self.draw_shape.fill;
                self.draw_shape.radius = R_CONTROL - 1.0;
                self.draw_shape.draw_abs(cx, Rect { pos: r.pos + dvec2(1.0, 1.0), size: dvec2(fw.max(2.0), r.size.y - 2.0) });
            }
            // What the running film holds there now (a modulated value).
            if let Some(l) = live.filter(|_| !dragging) {
                if (l - v).abs() > 1e-9 {
                    let x = r.pos.x + 1.0 + (r.size.x - 2.0) * t_of(l);
                    self.draw_shape.fill = color(p.accent_tint, 0.9);
                    self.draw_shape.edge = self.draw_shape.fill;
                    self.draw_shape.radius = 0.0;
                    self.draw_shape.draw_abs(cx, Rect { pos: dvec2(x - 1.0, r.pos.y + 2.0), size: dvec2(2.0, r.size.y - 4.0) });
                }
            }
        }
        let text_y = r.pos.y + (r.size.y - 12.0) * 0.5;
        if editing {
            let walk = Walk::fixed(r.size.x - 2.0 * FIELD_PAD, r.size.y).with_abs_pos(dvec2(r.pos.x + FIELD_PAD, r.pos.y));
            let _ = self.input.draw_walk(cx, &mut Scope::empty(), walk);
            if self.edit_focus {
                self.edit_focus = false;
                self.input.set_key_focus(cx);
                self.input.select_all(cx);
            }
            if let Some(why) = self.edit_error.clone() {
                let w = advance(&self.draw_value, cx, &why) + 12.0;
                host::tip(Rect { pos: dvec2(r.pos.x, r.pos.y + r.size.y + 2.0), size: dvec2(w, 18.0) }, why, false);
            }
            return;
        }
        let pad = if arrows { ARROW_W } else { FIELD_PAD };
        if arrows {
            let left = Rect { pos: r.pos, size: dvec2(ARROW_W, r.size.y) };
            let right = Rect { pos: dvec2(r.pos.x + r.size.x - ARROW_W, r.pos.y), size: dvec2(ARROW_W, r.size.y) };
            self.spots.push((left, Part::Step(i, -1)));
            self.spots.push((right, Part::Step(i, 1)));
            if hot && !dragging {
                for (k, a) in [left, right].into_iter().enumerate() {
                    let on = self.hot == Some(Part::Step(i, if k == 0 { -1 } else { 1 }));
                    self.draw_shape.shape = 5.0;
                    self.draw_shape.value = k as f32;
                    self.draw_shape.fill = color(if on { p.text_bright } else { p.muted }, 1.0);
                    self.draw_shape.draw_abs(cx, Rect { pos: a.pos + dvec2(2.0, 3.0), size: dvec2(10.0, a.size.y - 6.0) });
                }
            }
        }
        let unit = if self.field_count() == 1 { self.unit().to_string() } else { String::new() };
        let deg = if self.kind == PanelKind::Angle && unit.is_empty() { "°" } else { "" };
        let said = format!("{}{unit}{deg}", fmt_number(v, self.shown_decimals()));
        let vw = advance(&self.draw_value, cx, &said);
        // The value right, the label left (a short field: the value centred).
        let key_in = if keyed && i == 0 && !label.is_empty() { 14.0 } else { 0.0 };
        let room = r.size.x - 2.0 * pad - vw - 6.0 - key_in;
        self.draw_value.color = color(p.text_bright, 1.0);
        if label.is_empty() || room < 12.0 {
            self.draw_value.draw_abs(cx, dvec2(r.pos.x + (r.size.x - vw) * 0.5, text_y), &said);
            return;
        }
        self.draw_value.draw_abs(cx, dvec2(r.pos.x + r.size.x - pad - vw, text_y), &said);
        let (shown_label, cut) = fit_text(&self.draw_label, cx, label, room);
        let lx = r.pos.x + pad + key_in;
        self.draw_label.color = color(if changed { p.text_hi } else { p.label }, 1.0);
        self.draw_label.draw_abs(cx, dvec2(lx, text_y), &shown_label);
        if i == 0 {
            let label_rect = Rect { pos: dvec2(lx, r.pos.y), size: dvec2(room, r.size.y) };
            self.label_cut = cut.then(|| (label_rect, label.to_string()));
            if cut && self.label_hot {
                let w = advance(&self.draw_label, cx, label) + 12.0;
                host::tip(Rect { pos: dvec2(lx, r.pos.y + r.size.y + 2.0), size: dvec2(w, 18.0) }, label.to_string(), false);
            }
        }
    }

    /// The context menu, over the panel.
    fn draw_context_menu(&mut self, cx: &mut Cx2d) {
        let Some(mut menu) = self.menu.take() else { return };
        let p = makepad_look::palette::get(cx);
        let mut list = self.menu_list.take().unwrap_or_else(|| DrawList2d::new(cx));
        let widest = menu.items.iter().map(|(t, _, _)| advance(&self.draw_menu_text, cx, t)).fold(0.0, f64::max);
        let keys = menu.items.iter().map(|(_, k, _)| advance(&self.draw_menu_text, cx, k)).fold(0.0, f64::max);
        let w = widest + keys + 24.0 + if keys > 0.0 { 24.0 } else { 0.0 };
        let h = menu.items.len() as f64 * MENU_ROW + 8.0;
        menu.size = dvec2(w, h);
        list.begin_overlay_reuse(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());
        cx.walk_turtle(Walk::fixed(w, h));
        self.draw_menu.edged_round(cx, Rect { pos: DVec2::default(), size: menu.size }, color(p.menu, 1.0), color(p.border, 1.0), 1.0, R_CONTROL);
        menu.rects.clear();
        for (k, (text, key, act)) in menu.items.iter().enumerate() {
            let r = Rect { pos: dvec2(4.0, 4.0 + k as f64 * MENU_ROW), size: dvec2(w - 8.0, MENU_ROW) };
            if menu.hot == Some(k) && *act != MenuAct::Note {
                self.draw_menu.fill_round(cx, r, color(p.control_hover, 1.0), R_CONTROL);
            }
            self.draw_menu_text.color = color(if *act == MenuAct::Note { p.muted } else { p.text }, 1.0);
            self.draw_menu_text.draw_abs(cx, dvec2(r.pos.x + 8.0, r.pos.y + 5.0), text);
            if !key.is_empty() {
                let kw = advance(&self.draw_menu_text, cx, key);
                self.draw_menu_text.color = color(p.muted, 1.0);
                self.draw_menu_text.draw_abs(cx, dvec2(r.pos.x + r.size.x - 8.0 - kw, r.pos.y + 5.0), key);
            }
            menu.rects.push(r);
        }
        cx.end_pass_sized_turtle_with_shift(self.draw_bg.area(), menu.offset);
        list.end(cx);
        self.menu_list = Some(list);
        self.menu = Some(menu);
    }
}

/// `a` moved toward `b` by `t` (packed colours).
fn mix_colour(a: u32, b: u32, t: f32) -> u32 {
    let ca = Vec4f::from_u32(a);
    let cb = Vec4f::from_u32(b);
    let m = |x: f32, y: f32| ((x + (y - x) * t).clamp(0.0, 1.0) * 255.0).round() as u32;
    (m(ca.x, cb.x) << 24) | (m(ca.y, cb.y) << 16) | (m(ca.z, cb.z) << 8) | m(ca.w, ca.w)
}

impl Widget for PanelControl {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Clipboard(reply) = event {
            if let Some(index) = self.pastes.iter().position(|paste| paste.0 == reply.request) {
                let (_, comp, path, when) = self.pastes.remove(index);
                if self.visible && self.path == path && self.when == when {
                    let text = reply.content.text.clone().or_else(|| {
                        (!reply.content.supported).then(|| CLIP.with(|c| c.borrow().clone())).flatten()
                    });
                    if let Some(text) = text { self.paste_text(cx, comp, &text); }
                }
            }
        }
        if !self.visible {
            return;
        }
        let me = self.widget_uid().0;
        // The menu owns the pointer while it is open.
        if self.menu.is_some() {
            match event {
                Event::MouseMove(e) => {
                    let hot = self.menu_item_at(cx, e.abs);
                    if let Some(m) = self.menu.as_mut() {
                        if m.hot != hot {
                            m.hot = hot;
                            self.draw_bg.redraw(cx);
                        }
                    }
                    return;
                }
                Event::MouseDown(e) => {
                    let item = self.menu_item_at(cx, e.abs);
                    let inside = self.menu_contains(cx, e.abs);
                    let (act, comp) = match (item, self.menu.as_ref()) {
                        (Some(k), Some(m)) => (Some(m.items[k].2), m.comp),
                        _ => (None, 0),
                    };
                    if inside && act == Some(MenuAct::Note) {
                        return;
                    }
                    self.close_menu(cx);
                    if let Some(act) = act {
                        self.menu_act(cx, act, comp);
                        return;
                    }
                    if inside {
                        return;
                    }
                }
                Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => {
                    self.close_menu(cx);
                    return;
                }
                _ => {}
            }
        }
        // Typing in a field: the text box has the keys.
        if self.editing.is_some() {
            if let Event::KeyDown(ke) = event {
                if ke.key_code == KeyCode::Tab && cx.has_key_focus(self.input.area()) {
                    self.tab(cx, ke.modifiers.shift);
                    return;
                }
            }
            let actions = cx.capture_actions(|cx| self.input.handle_event(cx, event, scope));
            for action in actions {
                match action.as_widget_action().cast() {
                    TextInputAction::Returned(text, _) => {
                        if self.commit_edit(cx, &text) {
                            self.end_edit(cx);
                        }
                    }
                    TextInputAction::Escaped => self.end_edit(cx),
                    TextInputAction::KeyFocusLost => {
                        if self.editing.is_some() {
                            // A click elsewhere commits what is typed (an
                            // entry that does not read is dropped).
                            let text = self.input.text();
                            let _ = self.commit_edit(cx, &text);
                            self.editing = None;
                            self.edit_error = None;
                            self.draw_bg.redraw(cx);
                        }
                    }
                    TextInputAction::Changed(_) => {
                        if self.edit_error.take().is_some() {
                            self.draw_bg.redraw(cx);
                        }
                    }
                    TextInputAction::KeyDownUnhandled(ke) if ke.key_code == KeyCode::Escape => self.end_edit(cx),
                    _ => {}
                }
            }
            // The keys were the text box's (an Enter that committed must
            // not open it again below).
            if matches!(event, Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_)) {
                return;
            }
        }
        // A drag in flight: Escape or a right click puts the value back.
        if self.press.as_ref().is_some_and(|p| p.engaged) {
            match event {
                Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => {
                    self.cancel_drag(cx);
                    return;
                }
                Event::MouseDown(me) if me.button.is_secondary() => {
                    self.cancel_drag(cx);
                    return;
                }
                _ => {}
            }
        }
        // Keys over a field (or the one with the keyboard).
        if let Event::KeyDown(ke) = event {
            if self.editing.is_none() && self.keys_mine(cx) {
                let cmd = ke.modifiers.control || ke.modifiers.logo;
                match ke.key_code {
                    // I keys its value at the playhead (animating it first),
                    // Alt+I takes that key away (the last one: the animation).
                    KeyCode::KeyI if self.hovered && !cmd && self.animatable() && self.key.state.as_ref().is_some_and(|s| s.is_ok()) => {
                        host::animate(&self.path, !ke.modifiers.alt);
                        self.draw_bg.redraw(cx);
                        return;
                    }
                    KeyCode::KeyC if cmd => {
                        self.copy(cx);
                        return;
                    }
                    KeyCode::KeyV if cmd => {
                        let comp = self.hot.and_then(Part::field).unwrap_or(self.focus);
                        self.paste(cx, comp);
                        return;
                    }
                    KeyCode::Backspace | KeyCode::Delete if !cmd => {
                        self.reset(cx);
                        return;
                    }
                    KeyCode::Minus | KeyCode::NumpadSubtract if !cmd && self.numeric() => {
                        self.negate(cx);
                        return;
                    }
                    KeyCode::ReturnKey | KeyCode::NumpadEnter if self.numeric() && self.kind != PanelKind::Seed => {
                        let comp = self.hot.and_then(Part::field).unwrap_or(self.focus);
                        self.begin_edit(cx, comp);
                        return;
                    }
                    _ => {}
                }
            }
        }
        // Ctrl/Cmd + wheel over a field steps it (a plain wheel scrolls).
        if let Event::Scroll(e) = event {
            if (e.modifiers.control || e.modifiers.logo) && !e.handled_y.get() && e.scroll.y.abs() > f64::EPSILON && self.editing.is_none() {
                if let Some(i) = self.part_at(cx, e.abs).and_then(Part::field) {
                    let dir = if e.scroll.y < 0.0 { 1.0 } else { -1.0 };
                    self.step_field(cx, i, dir, e.modifiers.shift);
                    e.handled_y.set(true);
                    return;
                }
            }
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                self.hovered = true;
                HOVERED.with(|h| h.set(Some(me)));
                let at = self.local(cx, fe.abs);
                let on_key = self.key.contains(at);
                if on_key != self.key.hot {
                    self.key.hot = on_key;
                    self.draw_bg.redraw(cx);
                }
                if on_key {
                    cx.set_cursor(MouseCursor::Hand);
                    return;
                }
                let on_label = self.label_cut.as_ref().is_some_and(|(r, _)| r.contains(at));
                if on_label != self.label_hot {
                    self.label_hot = on_label;
                    self.draw_bg.redraw(cx);
                }
                let hot = self.part_at(cx, fe.abs);
                if self.press.as_ref().is_none_or(|p| !p.engaged) {
                    cx.set_cursor(match (self.kind, hot) {
                        (_, Some(Part::Field(_))) if self.editing.is_some() && hot.and_then(Part::field) == self.editing => MouseCursor::Text,
                        (_, Some(Part::Field(_))) => MouseCursor::EwResize,
                        (_, Some(Part::Step(..))) => MouseCursor::Hand,
                        (PanelKind::Knob | PanelKind::Fader, Some(Part::Value(_))) => MouseCursor::NsResize,
                        (_, Some(_)) => MouseCursor::Hand,
                        _ => MouseCursor::Default,
                    });
                }
                if hot != self.hot {
                    self.hot = hot;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                self.hovered = false;
                HOVERED.with(|h| {
                    if h.get() == Some(me) {
                        h.set(None);
                    }
                });
                self.key.hot = false;
                self.label_hot = false;
                self.hot = None;
                self.draw_bg.redraw(cx);
            }
            // A secondary click: the menu.
            Hit::FingerDown(fe) if !fe.is_primary_hit() => {
                if self.press.as_ref().is_some_and(|p| p.engaged) {
                    return;
                }
                let comp = self.part_at(cx, fe.abs).and_then(Part::field).unwrap_or(self.focus);
                self.open_menu(cx, fe.abs, comp);
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                // A press in the field being typed in is the text box's.
                if self.editing.is_some() && self.part_at(cx, fe.abs).and_then(Part::field) == self.editing {
                    return;
                }
                cx.set_key_focus(self.draw_bg.area());
                FOCUSED.with(|f| f.set(Some((self.widget_uid().0, self.draw_bg.area()))));
                // The key button acts on the release.
                if self.key.contains(self.local(cx, fe.abs)) {
                    return;
                }
                let Some(part) = self.part_at(cx, fe.abs) else { return };
                // A double click on a knob, fader or dial puts back the value
                // the panel opened with (a field's second click is in its
                // text box).
                if fe.tap_count >= 2 && matches!(part, Part::Value(_)) && matches!(self.kind, PanelKind::Knob | PanelKind::Fader | PanelKind::Angle) {
                    self.reset(cx);
                    return;
                }
                if let Some(i) = part.field() {
                    self.focus = i;
                }
                if let Part::Value(i) = part {
                    self.focus = i;
                }
                // The gesture's sets are held live and written at the
                // release, one undo step, wherever this control is.
                host::hold(true);
                self.press = Some(Press { part, origin: fe.abs, start: self.numbers.clone(), engaged: false, comps: Vec::new(), anchor_at: 0.0, anchor: Vec::new(), raw: Vec::new(), fine: false, rate: 0.0, quantum: 0.0, selecting: false, members: Vec::new() });
                // A dial and a pad answer the press at once.
                if matches!((self.kind, part), (PanelKind::Angle | PanelKind::Xy, Part::Value(_))) {
                    self.drag_to(cx, fe.abs, fe.modifiers, false);
                }
            }
            Hit::FingerMove(fe) => {
                if self.press.is_some() {
                    self.drag_to(cx, fe.abs, fe.modifiers, false);
                    if self.press.as_ref().is_some_and(|p| p.engaged && p.part.field().is_some()) {
                        cx.set_cursor(MouseCursor::Hidden);
                    }
                }
            }
            Hit::FingerUp(fe) => {
                if self.press.is_some() {
                    host::hold(false);
                }
                if self.press.is_none() && fe.is_over && fe.is_primary_hit() {
                    let at = self.local(cx, fe.abs);
                    if self.key.click(&self.path, at) {
                        self.draw_bg.redraw(cx);
                    }
                    return;
                }
                let Some(pr) = self.press.clone() else { return };
                self.clear_multi(cx);
                if pr.engaged || matches!((self.kind, pr.part), (PanelKind::Angle | PanelKind::Xy, Part::Value(_))) {
                    // The release writes the gesture's last value: one undo step.
                    let values = self.numbers.clone();
                    let d = if pr.engaged { field::decimals_of(pr.quantum).max(self.shown_decimals()) } else { self.decimals() };
                    self.press = None;
                    self.emit_values(cx, values, d, true);
                    return;
                }
                self.press = None;
                if !fe.is_over {
                    return;
                }
                match pr.part {
                    // A click without a drag: type into it.
                    Part::Field(i) => self.begin_edit(cx, i),
                    Part::Step(i, d) => self.step_field(cx, i, d as f64, fe.modifiers.shift),
                    Part::Value(_) if matches!(self.kind, PanelKind::Toggle) => self.key_step(cx, 1.0, false),
                    Part::Pick(i) => self.pick(cx, i),
                    Part::Reroll => self.reroll(cx),
                    Part::Toggle => self.key_step(cx, 1.0, false),
                    _ => {}
                }
            }
            Hit::KeyDown(ke) if self.editing.is_none() => {
                let fine = ke.modifiers.shift;
                match ke.key_code {
                    KeyCode::ArrowRight | KeyCode::ArrowUp => self.key_step(cx, 1.0, fine),
                    KeyCode::ArrowLeft | KeyCode::ArrowDown => self.key_step(cx, -1.0, fine),
                    KeyCode::ReturnKey | KeyCode::Space => match self.kind {
                        PanelKind::Seed => self.reroll(cx),
                        PanelKind::Toggle | PanelKind::Choice | PanelKind::Ease => self.key_step(cx, 1.0, false),
                        _ => {}
                    },
                    KeyCode::Tab if self.field_count() > 1 => {
                        self.focus = (self.focus + 1) % self.field_count();
                        self.draw_bg.redraw(cx);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        self.read();
        // Tab from the field before: this one types.
        let me = self.widget_uid().0;
        if let Some((uid, comp)) = NEXT_EDIT.with(|n| n.get()) {
            if uid == me {
                NEXT_EDIT.with(|n| n.set(None));
                self.begin_edit(cx, comp);
            }
        }
        // What it is, for the module it is in (presets, morph, randomise).
        let kind = match self.kind {
            PanelKind::Toggle => host::BoundKind::Bool,
            PanelKind::Choice | PanelKind::Ease => host::BoundKind::Choice(self.choice_list()),
            _ => {
                let (lo, hi) = self.range();
                host::BoundKind::Numbers { lo, hi, step: self.step() }
            }
        };
        host::register(host::Bound { path: self.path.clone(), kind });
        let p = makepad_look::palette::get(cx);
        // A fixed narrow width: the label goes under the control.
        let narrow = matches!(walk.width, Size::Fixed(w) if w < 150.0);
        let height = match self.kind {
            PanelKind::Xy => 132.0,
            PanelKind::Knob | PanelKind::Angle => if narrow { 78.0 } else { 46.0 },
            PanelKind::Fader => 156.0,
            PanelKind::Ease => 60.0,
            _ => ROW,
        };
        // Choices wrap: as many rows as the width takes.
        let height = if self.kind == PanelKind::Choice {
            let width = (cx.turtle().inner_width() - LABEL - GAP).max(40.0);
            let (mut x, mut rows) = (0.0, 1.0);
            for c in self.choice_list() {
                let cw = advance(&self.draw_label, cx, &c) + 18.0;
                if x + cw > width && x > 0.0 {
                    x = 0.0;
                    rows += 1.0;
                }
                x += cw;
            }
            rows * ROW
        } else if self.kind == PanelKind::Ease {
            let width = (cx.turtle().inner_width() - LABEL - GAP).max(40.0);
            let (_, rows, _) = ease_grid(width, self.choice_list().len());
            rows as f64 * (EASE_H + 4.0)
        } else {
            height
        };
        let walk = Walk { height: Size::Fixed(height), ..walk };
        let rect = cx.walk_turtle(walk);
        // (A host's own field, without a path, is on no path of the scene.)
        let paths = if self.path.is_empty() { Vec::new() } else { vec![self.path.clone()] };
        let uid = self.widget_uid().0;
        host::placed(cx, &mut self.place, uid, rect, paths);
        self.draw_bg.draw_abs(cx, rect);
        self.drawn_at = rect.pos;
        if FOCUSED.with(|f| f.get()).is_some_and(|(u, _)| u == me) {
            let area = self.draw_bg.area();
            FOCUSED.with(|f| f.set(Some((me, area))));
        }
        self.spots.clear();
        self.label_cut = None;
        let label = if self.label.is_empty() { host::label_of(&self.path) } else { self.label.clone() };
        let changed = self.original.as_ref().is_some_and(|o| *o != self.code);
        // Narrow (a knob, a fader or a dial in a row of them): the label under
        // the control, the control at the left.
        let stacked = (narrow || rect.size.x < 150.0) && matches!(self.kind, PanelKind::Knob | PanelKind::Fader | PanelKind::Angle);
        // The label inside the field (a slider, a number, a seed).
        let inside = matches!(self.kind, PanelKind::Slider | PanelKind::Number | PanelKind::Seed);
        // The key button before the label (numbers and vectors).
        if self.animatable() {
            self.key.refresh(&self.path);
        }
        let keyed_in = if self.animatable() && self.key.keyed() { 14.0 } else { 0.0 };
        let label_at = if stacked {
            dvec2(rect.pos.x + keyed_in, rect.pos.y + rect.size.y - 12.0)
        } else if inside {
            dvec2(rect.pos.x + if matches!(self.kind, PanelKind::Number | PanelKind::Seed) { ARROW_W } else { FIELD_PAD }, rect.pos.y + 6.0)
        } else {
            dvec2(rect.pos.x + keyed_in, rect.pos.y + 6.0)
        };
        if !inside {
            // The label in its column, cut with an ellipsis (the whole of it
            // shown on hover); room kept for the changed mark.
            let room = if stacked { rect.size.x - keyed_in } else { LABEL - keyed_in - if changed { 10.0 } else { 4.0 } };
            let (shown_label, cut) = fit_text(&self.draw_label, cx, &label, room.max(12.0));
            self.draw_label.color = color(if changed { p.text_hi } else { p.label }, 1.0);
            self.draw_label.draw_abs(cx, label_at, &shown_label);
            let label_rect = Rect { pos: dvec2(label_at.x, rect.pos.y), size: dvec2(room.max(12.0), if stacked { rect.size.y } else { ROW }) };
            self.label_cut = cut.then(|| (label_rect, label.clone()));
            if cut && self.label_hot {
                let w = advance(&self.draw_label, cx, &label) + 12.0;
                host::tip(Rect { pos: dvec2(label_at.x, label_at.y + 14.0), size: dvec2(w, 18.0) }, label.clone(), false);
            }
            if changed && !stacked {
                // A dot: this control's value is not what the panel found
                // (Backspace puts that back).
                self.draw_shape.shape = 0.0;
                self.draw_shape.fill = color(p.accent, 1.0);
                self.draw_shape.edge = color(p.accent, 1.0);
                self.draw_shape.stroke = 0.0;
                self.draw_shape.radius = 2.5;
                self.draw_shape.draw_abs(cx, Rect { pos: dvec2(rect.pos.x + LABEL - 7.0, rect.pos.y + 10.0), size: dvec2(5.0, 5.0) });
            }
        }
        let x0 = if stacked || inside { rect.pos.x } else { rect.pos.x + LABEL + GAP };
        let w = if stacked || inside { rect.size.x } else { (rect.size.x - LABEL - GAP).max(40.0) };
        let (lo, hi) = self.range();
        let t_of = |v: f64| if hi > lo { ((v - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
        let v0 = self.numbers.first().copied().unwrap_or(lo);
        let unit = self.unit().to_string();
        // What the running film holds there now (a modulated value moves
        // away from what is written): the ring or mark at it, followed
        // frame by frame while the film plays.
        let live = host::live(&self.path);
        if live.is_some() && host::playhead().is_some_and(|p| p.2) {
            cx.redraw_area_in_draw(self.draw_bg.area());
        }
        let live_t = live.map(|l| t_of(l) as f32);
        let hot = |part: Part, s: &Self| s.hot == Some(part) || s.press.as_ref().is_some_and(|pr| pr.part == part);
        let box_at = |s: &mut Self, cx: &mut Cx2d, r: Rect, fill: Vec4f, edge: Vec4f| {
            s.draw_shape.shape = 0.0;
            s.draw_shape.fill = fill;
            s.draw_shape.edge = edge;
            s.draw_shape.stroke = 1.0;
            s.draw_shape.radius = R_CONTROL;
            s.draw_shape.draw_abs(cx, r);
        };
        let row = |y: f64| Rect { pos: dvec2(x0, y + 2.0), size: dvec2(w, FIELD_H) };
        match self.kind {
            PanelKind::Slider => {
                self.draw_field(cx, &p, row(rect.pos.y), 0, &label, true, false, live);
            }
            PanelKind::Number => {
                self.draw_field(cx, &p, row(rect.pos.y), 0, &label, false, true, live);
            }
            PanelKind::Seed => {
                let r = Rect { pos: dvec2(x0, rect.pos.y + 2.0), size: dvec2((w - 70.0).max(60.0), FIELD_H) };
                self.draw_field(cx, &p, r, 0, &label, false, true, None);
                let b = Rect { pos: dvec2(r.pos.x + r.size.x + 6.0, rect.pos.y + 2.0), size: dvec2(64.0, FIELD_H) };
                box_at(self, cx, b, color(if hot(Part::Reroll, self) { p.control_hover } else { p.raised }, 1.0), color(p.line, 1.0));
                self.draw_label.color = color(p.text, 1.0);
                self.draw_label.draw_abs(cx, dvec2(b.pos.x + 8.0, rect.pos.y + 6.0), "Shuffle");
                self.spots.push((b, Part::Reroll));
            }
            PanelKind::Range => {
                // Its two ends, each a field filled to its place.
                let fw = ((w - 6.0) * 0.5).max(30.0);
                for (i, name) in ["from", "to"].into_iter().enumerate() {
                    let r = Rect { pos: dvec2(x0 + i as f64 * (fw + 6.0), rect.pos.y + 2.0), size: dvec2(fw, FIELD_H) };
                    self.draw_field(cx, &p, r, i, name, true, false, None);
                }
            }
            PanelKind::Vec3 => {
                let names = ["X", "Y", "Z"];
                let n = self.field_count();
                let cw = (w - 4.0 * (n as f64 - 1.0)) / n as f64;
                for i in 0..n {
                    let r = Rect { pos: dvec2(x0 + i as f64 * (cw + 4.0), rect.pos.y + 2.0), size: dvec2(cw, FIELD_H) };
                    self.draw_field(cx, &p, r, i, names[i], false, cw >= 70.0, None);
                }
            }
            PanelKind::Xy => {
                // The pad, and its two numbers as fields beside it.
                let side = (w.min(rect.size.y - 4.0)).max(40.0);
                let pad = Rect { pos: dvec2(x0, rect.pos.y + 2.0), size: dvec2(side, side - 4.0) };
                box_at(self, cx, pad, color(p.well, 1.0), color(if hot(Part::Value(0), self) { p.accent } else { p.line }, 1.0));
                // The cross at the range's middle.
                self.draw_shape.shape = 0.0;
                self.draw_shape.stroke = 0.0;
                self.draw_shape.fill = color(p.line_faint, 1.0);
                self.draw_shape.edge = color(p.line_faint, 1.0);
                self.draw_shape.radius = 0.0;
                self.draw_shape.draw_abs(cx, Rect { pos: dvec2(pad.pos.x + pad.size.x * 0.5, pad.pos.y), size: dvec2(1.0, pad.size.y) });
                self.draw_shape.draw_abs(cx, Rect { pos: dvec2(pad.pos.x, pad.pos.y + pad.size.y * 0.5), size: dvec2(pad.size.x, 1.0) });
                let (x, y) = (self.numbers.first().copied().unwrap_or(lo), self.numbers.get(1).copied().unwrap_or(lo));
                let c = dvec2(pad.pos.x + pad.size.x * t_of(x), pad.pos.y + pad.size.y * (1.0 - t_of(y)));
                self.draw_shape.shape = 4.0;
                self.draw_shape.fill = color(p.accent, 1.0);
                self.draw_shape.draw_abs(cx, Rect { pos: c - dvec2(7.0, 7.0), size: dvec2(14.0, 14.0) });
                self.spots.push((pad, Part::Value(0)));
                let fx = pad.pos.x + pad.size.x + 8.0;
                let fw = (x0 + w - fx).min(140.0);
                if fw >= 50.0 {
                    for (i, name) in ["X", "Y"].into_iter().enumerate() {
                        let r = Rect { pos: dvec2(fx, rect.pos.y + 2.0 + i as f64 * ROW), size: dvec2(fw, FIELD_H) };
                        self.draw_field(cx, &p, r, i, name, false, fw >= 90.0, None);
                    }
                }
            }
            PanelKind::Knob | PanelKind::Angle => {
                let r = Rect { pos: dvec2(x0, rect.pos.y + 2.0), size: dvec2(40.0, 40.0) };
                if self.kind == PanelKind::Knob {
                    self.draw_shape.shape = 1.0;
                    self.draw_shape.body = color(p.raised, 1.0);
                    self.draw_shape.value2 = live_t.unwrap_or(t_of(v0) as f32);
                    self.draw_shape.fill = color(if self.key.keyed() { p.amber } else { p.accent }, 1.0);
                    self.draw_shape.edge = color(if hot(Part::Value(0), self) { p.border_hi } else { p.line }, 1.0);
                    self.draw_shape.value = t_of(v0) as f32;
                } else {
                    self.draw_shape.shape = 2.0;
                    self.draw_shape.fill = color(p.accent, 1.0);
                    self.draw_shape.edge = color(p.well, 1.0);
                    self.draw_shape.value = if unit == "rad" { v0 as f32 } else { v0.to_radians() as f32 };
                }
                self.draw_shape.draw_abs(cx, r);
                self.spots.push((r, Part::Value(0)));
                // The readout: a field (drag it, or click and type).
                let readout = if stacked {
                    Rect { pos: dvec2(x0, rect.pos.y + 45.0), size: dvec2(w.max(40.0), 18.0) }
                } else {
                    Rect { pos: dvec2(x0 + 48.0, rect.pos.y + 12.0), size: dvec2((w - 48.0).clamp(40.0, 110.0), FIELD_H) }
                };
                self.draw_field(cx, &p, readout, 0, "", false, false, None);
            }
            PanelKind::Fader => {
                let r = Rect { pos: dvec2(x0, rect.pos.y + 2.0), size: dvec2(30.0, 116.0) };
                self.draw_shape.shape = 6.0;
                self.draw_shape.fill = color(p.accent, 1.0);
                self.draw_shape.edge = color(p.line, 1.0);
                self.draw_shape.body = color(p.raised, 1.0);
                self.draw_shape.value = t_of(v0) as f32;
                self.draw_shape.draw_abs(cx, r);
                // Ticks: every eighth of the travel.
                self.draw_shape.shape = 0.0;
                self.draw_shape.stroke = 0.0;
                self.draw_shape.radius = 0.0;
                self.draw_shape.fill = color(p.line, 1.0);
                for i in 0..=8 {
                    let ty = r.pos.y + 4.0 + (r.size.y - 8.0) * i as f64 / 8.0;
                    self.draw_shape.draw_abs(cx, Rect { pos: dvec2(r.pos.x + r.size.x + 2.0, ty), size: dvec2(if i % 4 == 0 { 6.0 } else { 3.0 }, 1.0) });
                }
                self.spots.push((r, Part::Value(0)));
                let readout = Rect { pos: dvec2(x0, rect.pos.y + 121.0), size: dvec2(w.clamp(40.0, 80.0), 18.0) };
                self.draw_field(cx, &p, readout, 0, "", false, false, None);
            }
            PanelKind::Toggle => {
                let r = Rect { pos: dvec2(x0, rect.pos.y + 4.0), size: dvec2(32.0, ROW - 8.0) };
                self.draw_shape.shape = 3.0;
                self.draw_shape.fill = color(p.accent, 1.0);
                self.draw_shape.edge = color(p.well, 1.0);
                self.draw_shape.value = if self.code.trim() == "true" { 1.0 } else { 0.0 };
                self.draw_shape.draw_abs(cx, r);
                self.spots.push((Rect { pos: rect.pos, size: dvec2(LABEL + GAP + 40.0, ROW) }, Part::Toggle));
            }
            PanelKind::Choice => {
                let list = self.choice_list();
                let chosen = self.chosen();
                let mut x = x0;
                let mut y = rect.pos.y + 2.0;
                for (i, c) in list.iter().enumerate() {
                    let cw = advance(&self.draw_label, cx, c) + 14.0;
                    if x + cw > rect.pos.x + rect.size.x && x > x0 {
                        x = x0;
                        y += ROW;
                    }
                    let r = Rect { pos: dvec2(x, y), size: dvec2(cw, ROW - 4.0) };
                    let on = *c == chosen;
                    let look = p.pick(on, hot(Part::Pick(i), self), p.raised);
                    box_at(self, cx, r, color(look.fill, 1.0), color(look.edge, 1.0));
                    self.draw_label.color = color(look.ink, 1.0);
                    self.draw_label.draw_abs(cx, dvec2(x + 7.0, y + 4.0), c);
                    self.spots.push((r, Part::Pick(i)));
                    x += cw + 4.0;
                }
            }
            PanelKind::Ease => {
                // The eases in a grid that fits the row (one row of all
                // when they are wide enough, else two of four): the chosen
                // one washed and edged in the accent, its curve the accent.
                let list = self.choice_list();
                let chosen = self.chosen();
                let (n, _, cw) = ease_grid(w, list.len());
                for (i, name) in list.iter().enumerate() {
                    let (col, row) = (i % n, i / n);
                    let r = Rect { pos: dvec2(x0 + col as f64 * (cw + EASE_GAP), rect.pos.y + row as f64 * (EASE_H + 4.0)), size: dvec2(cw, EASE_H) };
                    let on = *name == chosen;
                    let look = p.pick(on, hot(Part::Pick(i), self), p.well);
                    self.draw_shape.shape = 0.0;
                    self.draw_shape.fill = color(look.fill, 1.0);
                    self.draw_shape.edge = color(look.edge, 1.0);
                    self.draw_shape.stroke = if on { 1.5 } else { 1.0 };
                    self.draw_shape.radius = R_CONTROL;
                    self.draw_shape.draw_abs(cx, r);
                    // The curve, as a run of small dots.
                    self.draw_shape.stroke = 0.0;
                    self.draw_shape.radius = 1.0;
                    self.draw_shape.fill = color(if on { p.accent } else { p.text_soft }, 1.0);
                    self.draw_shape.edge = self.draw_shape.fill;
                    let (px, py) = (5.0, 5.0);
                    for k in 0..=32 {
                        let t = k as f64 / 32.0;
                        let v = ease_at(name, t);
                        let pt = dvec2(r.pos.x + px + t * (r.size.x - 2.0 * px), r.pos.y + r.size.y - py - v * (r.size.y - 2.0 * py));
                        self.draw_shape.draw_abs(cx, Rect { pos: pt - dvec2(1.0, 1.0), size: dvec2(2.0, 2.0) });
                    }
                    self.spots.push((r, Part::Pick(i)));
                }
            }
        }
        // Where its fields are, for Tab.
        let fields = self.field_count();
        if fields > 0 {
            let entry = FieldPlace {
                uid: me,
                rect,
                field: self.spot_of(Part::Field(0)).unwrap_or(rect),
                fields,
                area: self.draw_bg.area(),
                redraw: cx.redraw_id(),
                path: self.path.clone(),
                code: self.code.clone(),
                numbers: self.numbers.clone(),
                soft: self.range(),
                hard: self.hard(),
                whole: self.whole(),
                step: self.step,
            };
            FIELDS.with(|f| {
                let mut f = f.borrow_mut();
                match f.iter_mut().find(|e| e.uid == me) {
                    Some(e) => *e = entry,
                    None => f.push(entry),
                }
            });
        }
        // The key button: in the field before its label, at the label's
        // start, or beside the label under a knob, fader or dial; while
        // keyed and playing the value follows the playhead each frame.
        if self.animatable() {
            let at = if inside { dvec2(label_at.x, rect.pos.y + 7.0) } else { dvec2(label_at.x - keyed_in, label_at.y + 1.0) };
            let within = if stacked || inside { rect } else { Rect { pos: rect.pos, size: dvec2(LABEL - 4.0, rect.size.y) } };
            draw_key(cx, &mut self.key, at, within, &mut self.draw_key, &mut self.draw_value);
            if self.key.keyed() && host::playhead().is_some_and(|p| p.2) {
                cx.redraw_area_in_draw(self.draw_bg.area());
            }
        }
        self.draw_context_menu(cx);
        DrawStep::done()
    }
}

impl Drop for PanelControl {
    fn drop(&mut self) {
        let me = self.uid.0;
        FIELDS.with(|f| f.borrow_mut().retain(|e| e.uid != me));
        HOVERED.with(|h| {
            if h.get() == Some(me) {
                h.set(None);
            }
        });
    }
}

impl PanelControl {
    /// What a control without a path shows (a host's own field: a curve
    /// key's time): `code` as written (`1.25s`, `0.4`). Ignored while the
    /// person is dragging or typing in it.
    pub fn set_value(&mut self, cx: &mut Cx, code: &str) {
        if self.press.as_ref().is_some_and(|p| p.engaged) || self.editing.is_some() || self.value == code {
            return;
        }
        self.value = code.to_string();
        self.code = code.to_string();
        self.numbers = numbers_in(code);
        self.original = Some(code.to_string());
        self.soft = None;
        self.grow_soft();
        self.draw_bg.redraw(cx);
    }
}

impl PanelControlRef {
    /// A control without a path shows `code` ([`PanelControl::set_value`]).
    pub fn set_value(&self, cx: &mut Cx, code: &str) {
        if let Some(mut c) = self.borrow_mut() {
            c.set_value(cx, code);
        }
    }

    /// The value changes this control said (when it has no path).
    pub fn changed(&self, actions: &Actions) -> Option<(PanelValue, bool)> {
        actions.filter_widget_actions_cast::<PanelControlAction>(self.widget_uid()).find_map(|a| match a {
            PanelControlAction::Changed { value, done } => Some((value, done)),
            PanelControlAction::None => None,
        })
    }
}
