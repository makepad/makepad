//! Rating — a row of marks that shows one number and takes one.
//!
//! Five marks, three of them filled: the smallest control that carries a
//! judgement. It holds ONE number between zero and `count`, and everything
//! about it is in service of making that number easy to see from across the
//! room and easy to change without aiming.
//!
//! # What the row shows while the pointer is on it
//!
//! The value a press would set, in a quieter ink — not the value it holds.
//! The two schools here are "preview only the marks past the value" and
//! "show the whole row as it would be"; the second is the one that answers
//! the question actually being asked, which is *what do I get if I press
//! here*. The quieter ink is what says the row is lying for a moment, and
//! the count beside it goes on showing the value that is really held, so
//! nothing is lost while the preview is up. The pointer leaves and the row
//! comes back.
//!
//! # Clearing
//!
//! Pressing the mark the value already stands on clears the row to zero.
//! Without it there is no way back to "not rated" once a mark has been
//! pressed, and a rating that cannot be taken back is a trap — the finger
//! slips onto four marks and four marks is what the record says forever.
//! `clearable: false` turns it off for a form that requires an answer.
//!
//! # What this deliberately does not do
//!
//! It does not average, count, store or submit anything: it shows a number
//! and reports the number a gesture asked for. It does not name its marks —
//! "poor", "excellent" and the rest are the host's language, and the host
//! draws them beside the row. And it draws no shapes of its own: a mark is
//! a text glyph, so a part mark is the whole glyph cut down to the
//! fraction rather than a shape traced in a shader.
//!
//! The marks default to a filled and a hollow circle because those are
//! carried by the text faces the library ships. A star, a heart or a square
//! is a `glyph` and a `glyph_empty` away, in whatever face the app has.
use crate::{
    animator::{Animate, Animator, AnimatorAction, AnimatorImpl, Play},
    badge::{measure, sized},
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
    widget_async::ScriptAsyncResult,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.RatingBase = #(Rating::register_widget(vm))

    set_type_default() do #(DrawRating::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    /** A row of marks showing a value, and taking one. */
    mod.widgets.Rating = set_type_default() do mod.widgets.RatingBase{
        width: Fit
        height: Fit
        margin: theme.mspace_v_1

        /** how many marks the row has 1..12 step 1 */
        count: 5
        /** the value the row starts at 0..12 step 0.5 */
        default_value: 0.0
        /** the smallest share of one mark a press may land on: 1 whole, 0.5 halves 0.1..1 step 0.05 */
        precision: 1.0
        /** show a value without taking one; any fraction draws */
        read_only: false
        /** a press on the mark already reached clears the row */
        clearable: true
        /** draw the value and the count after the marks */
        show_value: false
        /** free text after the marks — "1204 ratings" */
        text: ""

        /** mark height in pixels 8..64 step 1 */
        mark_size: 18.
        /** room between two marks in pixels 0..24 step 0.5 */
        gap: 3.
        /** room between the marks and the label in pixels 0..40 step 1 */
        label_gap: 8.
        /** how loud the marks a press would set are against the ones held 0..1 step 0.05 */
        preview_weight: 0.55

        /** the mark for one the value has reached */
        glyph: "\u{25cf}"
        /** the mark for one it has not */
        glyph_empty: "\u{25cb}"
        /** the mark for a part one; empty cuts the whole mark down to the fraction */
        glyph_half: ""

        draw_full +: {
            color: theme.color_primary
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
        draw_empty +: {
            color: theme.color_outline
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
        draw_text +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }

        draw_bg +: {
            /** keyboard-focus mix 0..1 step 0.01 */
            focus: instance(0.0)
            /** disabled mix 0..1 step 0.01 */
            disabled: instance(0.0)

            /** focus ring thickness in pixels 0..4 step 0.5 */
            border_size: uniform(theme.size_focus_ring)
            /** corner rounding radius 0..24 step 0.5 */
            border_radius: uniform(theme.corner_radius)

            border_color: uniform(theme.color_u_hidden)
            border_color_focus: uniform(theme.color_primary)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                // The ring goes round the marks alone. The count beside
                // them is not a target, and a ring round both would say
                // it was.
                sdf.box(
                    self.border_size * 0.5
                    self.border_size * 0.5
                    max(self.marks_px - self.border_size, 1.0)
                    self.rect_size.y - self.border_size
                    self.border_radius
                )
                sdf.stroke(
                    self.border_color.mix(self.border_color_focus, self.focus * (1.0 - self.disabled))
                    self.border_size
                )
                return sdf.result
            }
        }

        animator: Animator{
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {focus: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {focus: 1.0}}
                }
            }
            disabled: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_bg: {disabled: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: {draw_bg: {disabled: 1.0}}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRating {
    #[deref]
    draw_super: DrawQuad,
    /// How wide the marks are, so the focus ring can hug them and stop
    /// short of the label. Rust measures the glyphs, so Rust owns it.
    #[live]
    marks_px: f32,
}

/// The row's geometry and the rules a gesture follows, apart from the
/// widget that draws it. It is its own type for two reasons: it can be
/// tested without a script heap, and the hit test and the drawing take
/// their numbers from the same place, so what is drawn is what is pressed.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Marks {
    count: usize,
    /// The smallest share of one mark a gesture may land on: 1 for whole
    /// marks, 0.5 for halves.
    quantum: f64,
    /// One mark's drawn width and the room after it, in layout points.
    mark: f64,
    gap: f64,
}

impl Marks {
    fn stride(&self) -> f64 {
        self.mark + self.gap
    }

    /// The marks alone, without the label after them.
    fn width(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.count as f64 * self.mark + (self.count - 1) as f64 * self.gap
        }
    }

    fn clamp(&self, value: f64) -> f64 {
        value.clamp(0.0, self.count as f64)
    }

    /// Where mark `index` starts, from the left edge of the row.
    fn mark_x(&self, index: usize) -> f64 {
        index as f64 * self.stride()
    }

    /// The value a press `x` into the row asks for.
    ///
    /// The room between two marks belongs to the mark BEFORE it, so a
    /// widely spaced row has no dead columns down it; and the left edge of
    /// the first mark already asks for one step, so zero is reached by the
    /// clear rule and never by a press that fell short.
    fn value_at(&self, x: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let marks_in = x.max(0.0) / self.stride();
        self.clamp(((marks_in / self.quantum).floor() + 1.0) * self.quantum)
    }

    /// How much of mark `index` a value fills, 0..1.
    fn fill_of(&self, value: f64, index: usize) -> f64 {
        (value - index as f64).clamp(0.0, 1.0)
    }

    /// What a press asking for `asked` leaves the value at.
    fn press(&self, value: f64, asked: f64, clearable: bool) -> f64 {
        if clearable && (asked - value).abs() < self.quantum * 0.5 {
            0.0
        } else {
            asked
        }
    }

    /// One step of the arrow keys. A value handed in from outside need not
    /// sit on the grid, so the step snaps it onto one on the way.
    fn nudge(&self, value: f64, steps: f64) -> f64 {
        let on_grid = (self.clamp(value) / self.quantum).round() + steps;
        self.clamp(on_grid * self.quantum)
    }

    /// Whether the finger has gone far enough since the press for this to
    /// be a drag along the row rather than a press that wobbled. Without
    /// the threshold a shaky hand undoes its own clear: the press empties
    /// the row and the next jittered pixel fills it straight back in.
    fn is_drag(&self, from_x: f64, to_x: f64) -> bool {
        (to_x - from_x).abs() > self.mark * 0.5
    }
}

/// How far below the top of its line box a glyph's ink starts, as a share
/// of the font size. `draw_abs` takes the line box; centring the line box
/// leaves the ink riding high.
const INK_DROP: f64 = 0.30;

/// The row is taller than its glyphs so the focus ring clears the ink.
const ROW_HEIGHT: f64 = 1.4;

/// Disabled is the same ink at a third of the weight: the shape still
/// reads, and the row is plainly not for pressing.
const DISABLED_WEIGHT: f32 = 0.35;

fn dim(color: Vec4f, weight: f32) -> Vec4f {
    Vec4f { w: color.w * weight, ..color }
}

/// Draw one glyph, `glyph_w` wide, centred in a box `width` wide whose left
/// edge is `x`. The width is handed in rather than measured here: every mark
/// in a row is one of two glyphs, and measuring each of them once a frame is
/// enough.
fn draw_centred(
    draw: &mut DrawText,
    cx: &mut Cx2d,
    x: f64,
    y: f64,
    width: f64,
    glyph_w: f64,
    glyph: &str,
) {
    draw.draw_abs(cx, dvec2(x + (width - glyph_w) * 0.5, y), glyph);
}

#[derive(Clone, Debug, Default)]
pub enum RatingAction {
    /// The value a press, a drag or a key settled on.
    Changed(f64),
    /// The value the pointer is over, while it is over the row.
    Preview(f64),
    /// The pointer left; the row shows what it holds again.
    PreviewEnd,
    #[default]
    None,
}

#[derive(Script, Widget, Animator)]
pub struct Rating {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_bg: DrawRating,
    /// The two faces of one mark. They share a box the width of the wider,
    /// so a mark keeps its place when it fills.
    #[live]
    draw_full: DrawText,
    #[live]
    draw_empty: DrawText,
    /// The count and any free text after the marks.
    #[live]
    draw_text: DrawText,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[apply_default]
    animator: Animator,

    #[live(5)]
    pub count: usize,
    /// Where the row starts. The value itself is not a live property: a
    /// live reload would put a rating back to what the file said and throw
    /// away what the person actually chose.
    #[live]
    default_value: f64,
    /// The smallest share of one mark a gesture may land on. One for whole
    /// marks, 0.5 for halves; anything in between is honoured, and a
    /// read-only row shows any fraction whatever this says.
    #[live(1.0)]
    pub precision: f64,
    /// Show a value without taking one. Such a row takes no hits at all,
    /// so a table row or a link under it still answers.
    #[live]
    pub read_only: bool,
    /// Whether a press on the mark the value stands on clears the row.
    #[live(true)]
    pub clearable: bool,
    /// Draw "3 / 5" after the marks.
    #[live]
    pub show_value: bool,
    /// Free text after the marks and the count: how many people voted,
    /// what the number means, whatever the host has to say.
    #[live]
    pub text: String,

    #[live(18.0)]
    pub mark_size: f64,
    #[live(3.0)]
    pub gap: f64,
    #[live(8.0)]
    pub label_gap: f64,
    /// How loud the previewed marks are against the held ones.
    #[live(0.55)]
    pub preview_weight: f64,

    #[live("\u{25cf}".to_string())]
    pub glyph: String,
    #[live("\u{25cb}".to_string())]
    pub glyph_empty: String,
    /// A mark for a part one. Empty — the default — cuts the whole mark
    /// down to the fraction instead, which is the only form that can show
    /// a 4.3 as well as a 4.5.
    #[live]
    pub glyph_half: String,

    #[rust]
    value: f64,
    /// One mark's drawn width, measured at draw time. The hit test needs
    /// it and has no text engine to ask.
    #[rust]
    mark_w: f64,
    /// The value the pointer is over, or nothing.
    #[rust]
    preview: Option<f64>,
    /// Where the press that is still down landed, so a wobble is not read
    /// as a drag.
    #[rust]
    press_x: Option<f64>,
}

impl ScriptHook for Rating {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        self.value = self.default_value.clamp(0.0, self.count as f64);
    }
}

impl Rating {
    /// The grid a gesture may land on. Zero or a negative would divide the
    /// row into nothing, and more than one mark per step is not a step.
    fn quantum(&self) -> f64 {
        if self.precision > 0.0 {
            self.precision.min(1.0)
        } else {
            1.0
        }
    }

    /// The row as it stands, built fresh each time: every number in it is a
    /// live property and the tweaker may have moved any of them since the
    /// last draw.
    fn marks(&self) -> Marks {
        Marks {
            count: self.count,
            quantum: self.quantum(),
            mark: self.mark_w.max(1.0),
            gap: self.gap,
        }
    }

    /// What the marks draw: the preview while there is one, else the value.
    fn shown(&self) -> f64 {
        self.preview.unwrap_or(self.value)
    }

    /// The value as a number. A whole value on a whole-mark row has no
    /// decimals to show; anything else does, since "4" for a 4.3 is a
    /// different claim from the one the row is making.
    fn value_text(&self) -> String {
        if self.quantum() >= 1.0 && (self.value - self.value.round()).abs() < 0.001 {
            format!("{:.0}", self.value)
        } else {
            format!("{:.1}", self.value)
        }
    }

    /// What is drawn after the marks, or nothing.
    fn label(&self) -> String {
        let mut out = String::new();
        if self.show_value {
            out.push_str(&format!("{} / {}", self.value_text(), self.count));
        }
        if !self.text.is_empty() {
            if !out.is_empty() {
                out.push_str("  ");
            }
            out.push_str(&self.text);
        }
        out
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// A value handed in is shown as it is. A read-only row averaging other
    /// people's votes is a fraction, and rounding it onto whatever grid
    /// this row happens to take would be a lie about the number; only a
    /// gesture quantizes.
    pub fn set_value(&mut self, cx: &mut Cx, value: f64) {
        self.value = value.clamp(0.0, self.count as f64);
        self.preview = None;
        self.draw_bg.redraw(cx);
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.set_value(cx, 0.0);
    }

    /// Take a value a gesture settled on and say so.
    fn commit(&mut self, cx: &mut Cx, value: f64) {
        if (value - self.value).abs() > f64::EPSILON {
            self.value = value;
            cx.widget_action(self.uid, RatingAction::Changed(value));
        }
        self.draw_bg.redraw(cx);
    }
}

impl Widget for Rating {
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
        if method == live_id!(set_value) {
            if let Some(args_obj) = args.as_object() {
                let trap = vm.bx.threads.cur().trap.pass();
                let value = vm.bx.heap.vec_value(args_obj, 0, trap).as_f64();
                if let Some(value) = value {
                    vm.with_cx_mut(|cx| self.set_value(cx, value));
                }
            }
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(clear) {
            vm.with_cx_mut(|cx| self.clear(cx));
            return ScriptAsyncResult::Return(NIL);
        }
        if method == live_id!(value) {
            return ScriptAsyncResult::Return(ScriptValue::from_f64(self.value));
        }
        ScriptAsyncResult::MethodNotFound
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.animator_handle_event(cx, event);
        // A read-only row is a picture of a number. It takes no hits, so
        // the row of a table or the link it sits inside still answers.
        if self.read_only || self.animator_in_state(cx, ids!(disabled.on)) {
            return;
        }
        let uid = self.uid;

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerHoverOut(_) => {
                if self.preview.take().is_some() {
                    cx.widget_action(uid, RatingAction::PreviewEnd);
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverOver(fe) => {
                let x = fe.abs.x - fe.rect.pos.x;
                let at = self.marks().value_at(x);
                if self.preview != Some(at) {
                    self.preview = Some(at);
                    cx.widget_action(uid, RatingAction::Preview(at));
                    self.draw_bg.redraw(cx);
                }
                cx.set_cursor(MouseCursor::Hand);
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                let x = fe.abs.x - fe.rect.pos.x;
                let m = self.marks();
                let landed = m.press(self.value, m.value_at(x), self.clearable);
                self.press_x = Some(x);
                // The preview goes down with the press. A preview still
                // showing the mark under the finger, on a row the press
                // has just cleared, would say the press had not worked.
                self.preview = None;
                self.commit(cx, landed);
            }
            Hit::FingerMove(fe) => {
                let Some(from) = self.press_x else {
                    return;
                };
                let x = fe.abs.x - fe.rect.pos.x;
                let m = self.marks();
                // A drag along the row is how a touch screen previews:
                // there is no pointer to hover with, so the value follows
                // the finger and is settled by lifting it.
                if m.is_drag(from, x) {
                    self.commit(cx, m.value_at(x));
                }
            }
            Hit::FingerUp(_) => {
                self.press_x = None;
            }
            Hit::KeyFocus(_) => {
                self.animator_play(cx, ids!(focus.on));
            }
            Hit::KeyFocusLost(_) => {
                self.animator_play(cx, ids!(focus.off));
            }
            Hit::KeyDown(ke) => {
                let m = self.marks();
                let next = match ke.key_code {
                    KeyCode::ArrowLeft | KeyCode::ArrowDown => Some(m.nudge(self.value, -1.0)),
                    KeyCode::ArrowRight | KeyCode::ArrowUp => Some(m.nudge(self.value, 1.0)),
                    // The bottom of the row is no marks at all where the
                    // row may be cleared, and one mark where it may not.
                    KeyCode::Home => Some(if self.clearable { 0.0 } else { m.quantum }),
                    KeyCode::End => Some(m.clamp(self.count as f64)),
                    _ => None,
                };
                if let Some(next) = next {
                    self.commit(cx, next);
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // The count is a live property and can be turned down under a value
        // that has already been set; without this the row would read "5 / 3"
        // and draw three full marks to prove it.
        self.value = self.value.min(self.count as f64);

        let size = self.mark_size as f32;
        self.draw_full.text_style.font_size = size;
        self.draw_empty.text_style.font_size = size;

        let full_glyph = self.glyph.clone();
        let empty_glyph = self.glyph_empty.clone();
        let half_glyph = self.glyph_half.clone();
        let full_w = measure(&self.draw_full, cx, &full_glyph);
        let empty_w = measure(&self.draw_empty, cx, &empty_glyph);
        let half_w = if half_glyph.is_empty() {
            0.0
        } else {
            measure(&self.draw_full, cx, &half_glyph)
        };
        self.mark_w = full_w.max(empty_w);

        let m = self.marks();
        let label = self.label();
        let label_w = if label.is_empty() {
            0.0
        } else {
            measure(&self.draw_text, cx, &label)
        };
        let marks_w = m.width();
        let width = marks_w + if label_w > 0.0 { self.label_gap + label_w } else { 0.0 };

        self.draw_bg.marks_px = marks_w as f32;
        let rect = self
            .draw_bg
            .draw_walk(cx, sized(walk, width, self.mark_size * ROW_HEIGHT));

        // Set, draw, RESTORE. The preview and the disabled state are how
        // the row is drawn this frame, not what its ink is.
        let disabled = self.animator_in_state(cx, ids!(disabled.on));
        let rest_full = self.draw_full.color;
        let rest_empty = self.draw_empty.color;
        let rest_label = self.draw_text.color;
        if disabled {
            self.draw_full.color = dim(rest_full, DISABLED_WEIGHT);
            self.draw_empty.color = dim(rest_empty, DISABLED_WEIGHT);
            self.draw_text.color = dim(rest_label, DISABLED_WEIGHT);
        } else if self.preview.is_some() {
            // Only the marks go quiet: the label goes on saying what the
            // value IS, so the row can still be read while the marks are
            // showing what a press would do to it.
            self.draw_full.color = dim(rest_full, self.preview_weight as f32);
        }

        let shown = self.shown();
        let y = rect.pos.y + (rect.size.y - self.mark_size) * 0.5 - self.mark_size * INK_DROP;
        for index in 0..self.count {
            let x = rect.pos.x + m.mark_x(index);
            let fill = m.fill_of(shown, index);
            if fill >= 1.0 {
                draw_centred(&mut self.draw_full, cx, x, y, self.mark_w, full_w, &full_glyph);
            } else if fill <= 0.0 {
                draw_centred(&mut self.draw_empty, cx, x, y, self.mark_w, empty_w, &empty_glyph);
            } else if !half_glyph.is_empty() {
                draw_centred(&mut self.draw_full, cx, x, y, self.mark_w, half_w, &half_glyph);
            } else {
                draw_centred(&mut self.draw_empty, cx, x, y, self.mark_w, empty_w, &empty_glyph);
                // The part mark is the whole one cut down to the fraction.
                // It is not traced as a shape: a small mark drawn as a path
                // does not paint reliably, and a clip costs one pair of
                // list entries and shows a 4.3 as well as a 4.5.
                cx.push_clip_rect(Rect {
                    pos: dvec2(x, rect.pos.y),
                    size: dvec2(self.mark_w * fill, rect.size.y),
                });
                draw_centred(&mut self.draw_full, cx, x, y, self.mark_w, full_w, &full_glyph);
                cx.pop_clip_rect();
            }
        }

        if !label.is_empty() {
            let font = self.draw_text.text_style.font_size as f64;
            let label_y = rect.pos.y + (rect.size.y - font) * 0.5 - font * INK_DROP;
            self.draw_text.draw_abs(
                cx,
                dvec2(rect.pos.x + marks_w + self.label_gap, label_y),
                &label,
            );
        }

        self.draw_full.color = rest_full;
        self.draw_empty.color = rest_empty;
        self.draw_text.color = rest_label;

        if !disabled && !self.read_only {
            cx.add_nav_stop(self.draw_bg.area(), NavRole::Slider, Inset::default());
        }
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.value_text()
    }
}

impl RatingRef {
    /// The value it holds, never the one being previewed.
    pub fn value(&self) -> f64 {
        self.borrow().map(|inner| inner.value()).unwrap_or(0.0)
    }

    pub fn set_value(&self, cx: &mut Cx, value: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, value);
        }
    }

    pub fn clear(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear(cx);
        }
    }

    /// The value a gesture settled on.
    pub fn changed(&self, actions: &Actions) -> Option<f64> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            RatingAction::Changed(value) => Some(value),
            _ => None,
        }
    }

    /// The value a press would set, while the pointer is over the row. For
    /// a host that names its marks: the word beside the row follows the
    /// pointer, and [`RatingRef::preview_ended`] puts it back.
    pub fn previewed(&self, actions: &Actions) -> Option<f64> {
        let item = actions.find_widget_action(self.widget_uid())?;
        match item.cast() {
            RatingAction::Preview(value) => Some(value),
            _ => None,
        }
    }

    pub fn preview_ended(&self, actions: &Actions) -> bool {
        actions
            .find_widget_action(self.widget_uid())
            .map(|item| matches!(item.cast(), RatingAction::PreviewEnd))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row with the geometry the preset gives it: five marks 14 points
    /// wide with 3 points between them, so a mark's stride is 17.
    fn row(count: usize, quantum: f64) -> Marks {
        Marks { count, quantum, mark: 14.0, gap: 3.0 }
    }

    #[test]
    fn a_press_anywhere_on_a_mark_asks_for_that_mark() {
        let m = row(5, 1.0);
        assert_eq!(m.value_at(0.0), 1.0, "the very first pixel is one mark");
        assert_eq!(m.value_at(13.0), 1.0);
        assert_eq!(m.value_at(m.mark_x(2) + 1.0), 3.0);
        assert_eq!(m.value_at(m.mark_x(4) + 13.0), 5.0);
    }

    #[test]
    fn half_precision_splits_each_mark_down_the_middle() {
        let m = row(5, 0.5);
        assert_eq!(m.value_at(1.0), 0.5, "the left of the first mark is a half");
        assert_eq!(m.value_at(11.0), 1.0, "and the right of it a whole");
        assert_eq!(m.value_at(m.mark_x(3) + 2.0), 3.5);
        assert_eq!(m.value_at(m.mark_x(3) + 12.0), 4.0);
    }

    #[test]
    fn the_room_between_two_marks_belongs_to_the_one_before_it() {
        // Otherwise a widely spaced row has a dead column down every gap,
        // and a press that lands in one either does nothing or jumps.
        let m = row(5, 1.0);
        assert_eq!(m.value_at(14.5), 1.0);
        assert_eq!(m.value_at(16.9), 1.0);
        assert_eq!(m.value_at(17.0), 2.0, "the next mark starts at its own edge");
    }

    #[test]
    fn a_press_past_the_end_of_the_row_is_the_last_mark() {
        let m = row(5, 1.0);
        assert_eq!(m.value_at(10_000.0), 5.0);
        assert_eq!(m.value_at(-40.0), 1.0, "and one short of it is the first");
    }

    #[test]
    fn pressing_the_mark_the_value_stands_on_clears_the_row() {
        let m = row(5, 1.0);
        assert_eq!(m.press(3.0, 3.0, true), 0.0);
        assert_eq!(m.press(3.0, 4.0, true), 4.0, "any other mark is just set");
        assert_eq!(m.press(0.0, 1.0, true), 1.0);
    }

    #[test]
    fn a_row_that_may_not_be_cleared_keeps_its_value() {
        // A form that requires an answer cannot offer a gesture that takes
        // the answer away.
        let m = row(5, 1.0);
        assert_eq!(m.press(3.0, 3.0, false), 3.0);
    }

    #[test]
    fn a_half_press_clears_only_the_half_it_landed_on() {
        let m = row(5, 0.5);
        assert_eq!(m.press(3.5, 3.5, true), 0.0);
        assert_eq!(m.press(3.5, 3.0, true), 3.0, "the other half of the same mark sets");
    }

    #[test]
    fn a_fraction_fills_one_mark_partly() {
        let m = row(5, 1.0);
        assert_eq!(m.fill_of(3.7, 0), 1.0);
        assert_eq!(m.fill_of(3.7, 2), 1.0);
        assert!((m.fill_of(3.7, 3) - 0.7).abs() < 0.0001, "the fourth is most of the way");
        assert_eq!(m.fill_of(3.7, 4), 0.0);
    }

    #[test]
    fn the_arrows_snap_an_odd_fraction_onto_the_grid() {
        // A read-only average handed in as 4.2 becomes editable the moment
        // the row takes focus, and the first arrow has to land somewhere
        // the row can actually show.
        let m = row(5, 0.5);
        assert_eq!(m.nudge(4.2, 1.0), 4.5);
        assert_eq!(m.nudge(4.2, -1.0), 3.5);
        assert_eq!(m.nudge(3.0, 1.0), 3.5);
    }

    #[test]
    fn the_arrows_stop_at_both_ends() {
        let m = row(5, 1.0);
        assert_eq!(m.nudge(5.0, 1.0), 5.0);
        assert_eq!(m.nudge(0.0, -1.0), 0.0);
    }

    #[test]
    fn a_wobble_under_the_finger_is_not_a_drag() {
        let m = row(5, 1.0);
        assert!(!m.is_drag(40.0, 44.0), "four points is a hand, not a gesture");
        assert!(m.is_drag(40.0, 60.0));
    }

    #[test]
    fn the_row_is_as_wide_as_its_marks_and_the_room_between_them() {
        let m = row(5, 1.0);
        assert_eq!(m.width(), 5.0 * 14.0 + 4.0 * 3.0);
        assert_eq!(row(1, 1.0).width(), 14.0, "one mark has no room after it");
        assert_eq!(row(0, 1.0).width(), 0.0);
    }

    #[test]
    fn a_row_with_no_marks_answers_zero_rather_than_dividing_by_it() {
        let m = row(0, 1.0);
        assert_eq!(m.value_at(50.0), 0.0);
    }
}
