//! Lane D. The drag-numeric field — Fab's most-used control and the one
//! thing `widgets/` has no equivalent for. `Slider` is close (it already
//! hides a `TextInput` behind a drag), but it maps the pointer to an
//! absolute position in a range; a number field is relative and modal.
//!
//! The behaviour contract (see `header_drag_math` for the pure core):
//!
//! * **Press** records an anchor and changes nothing. **Drag** engages after
//!   3 horizontal pixels, re-anchored at the engage point so the first pixel
//!   of a drag is a small change, not a jump. Vertical motion never counts.
//! * Two mappings. A **bounded** field (`show_fill: true`) sweeps its soft
//!   range across its own width, relative to the press point — the fill bar
//!   shows the position, and pressing never jumps the value to the cursor.
//!   An **unbounded** field moves by pixels × step — one arrow-click per
//!   pixel dragged, never a proportion of a range that means nothing.
//! * **Clamping shifts the anchor** instead of clipping: overshoot a limit
//!   and the value leaves it the instant the pointer comes back, with no
//!   dead zone. Cyclic fields (`wrap: true`) wrap instead — the hour comes
//!   round at 24, an angle at 360.
//! * **Shift is fine** (×0.1 field, ×0.05 bounded), **Ctrl snaps** to a
//!   range-sized increment, **Ctrl+Shift snaps finer** — all live mid-drag,
//!   and a modifier change re-anchors so the value never jumps, only the
//!   rate changes.
//! * **Every drag step publishes immediately** (`Changed`), so the scene
//!   follows the dial; release publishes `Ended` once. Escape or a
//!   right-button press mid-drag restores the pressed value.
//! * **A click without drag** opens text entry with the full-precision value
//!   selected; Enter commits, Escape cancels, focus loss commits. A click on
//!   the hover arrows at the ends steps by one increment; Ctrl+Wheel does
//!   the same. The whole row is the hit target.

use makepad_widgets::*;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    set_type_default() do #(DrawDragNum::script_shader(vm)){
        ..mod.draw.DrawQuad

        // These are `#[live]` fields on DrawDragNum, so they are already
        // instances; `instance(..)` here would hand the f32 field an object.
        hover: 0.0
        down: 0.0
        focus: 0.0
        disabled: 0.0
        fill: -1.0
        flat: 0.0

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            sdf.box(0.5, 0.5, w - 1.0, h - 1.0, fab.radius)
            let reveal = mix(1.0, max(self.hover, max(self.down, self.focus)), self.flat)
            let mut base = fab.color_num.mix(fab.color_num_hover, self.hover).mix(fab.color_input_active, self.down)
            base = vec4(base.xyz, base.w * reveal)
            sdf.fill_keep(base)
            let mut border = fab.color_border.mix(fab.color_focus_ring, self.focus)
            border = vec4(border.xyz, border.w * reveal)
            sdf.stroke(border, 1.0)
            if self.fill >= 0.0 {
                sdf.box(1.0, 1.0, max(2.0, (w - 2.0) * self.fill), h - 2.0, fab.radius)
                sdf.fill(vec4(fab.color_num_fill.xyz, 0.85))
            }
            // Hover arrows in the end zones; they retire while the field is
            // a text editor (focus carries the editing state).
            if self.hover > 0.01 {
                if self.focus < 0.5 {
                    let cy = h * 0.5
                    let a = vec4(fab.color_num_arrow.xyz, self.hover)
                    sdf.move_to(9.0, cy - 3.5)
                    sdf.line_to(5.5, cy)
                    sdf.line_to(9.0, cy + 3.5)
                    sdf.stroke(a, 1.25)
                    sdf.move_to(w - 9.0, cy - 3.5)
                    sdf.line_to(w - 5.5, cy)
                    sdf.line_to(w - 9.0, cy + 3.5)
                    sdf.stroke(a, 1.25)
                }
            }
            return sdf.result
        }
    }

    mod.widgets.FabDragNumberBase = #(FabDragNumber::register_widget(vm))
    mod.widgets.FabDragNumber = set_type_default() do mod.widgets.FabDragNumberBase{
        width: Fill
        height: fab.row_height
        flow: Right
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
        margin: Inset{top: 0 bottom: 0 left: 0 right: 0}

        label: ""
        min: 0.0
        max: 0.0
        step: 0.01
        snap: 0.0
        precision: 2
        suffix: ""
        value: 0.0
        wrap: false
        show_fill: false
        quantize: false
        wheel: false
        edit_on_double_click: false
        time_of_day: false

        draw_text +: {
            ink_centered: true
            color: fab.color_text_dim
            text_overflow: TextOverflow.Ellipsis
            text_style: theme.font_regular{
                font_size: fab.font_size_ui
            }
        }
        text_input: TextInput{
            width: Fill
            height: Fill
            // Read-only display may carry a unit suffix. Editing switches
            // this back to numeric-only in Rust.
            is_numeric_only: false
            padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
            margin: Inset{top: 0 bottom: 0 left: 0 right: 0}
            label_align: Align{x: 1.0 y: 0.5}
            draw_bg +: {
                color: vec4(0.0, 0.0, 0.0, 0.0)
                border_radius: 0.0
            }
            draw_text +: {
                ink_centered: true
                color: fab.color_text
                text_style: theme.font_regular{
                    font_size: fab.font_size_ui
                }
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {hover: 0.0, down: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {hover: 1.0, down: 0.0} }
                }
                down: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {hover: 1.0, down: 1.0} }
                }
            }
            focus: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: fab.anim_fast}}
                    apply: { draw_bg: {focus: 0.0} }
                }
                on: AnimatorState{
                    from: {all: Snap}
                    apply: { draw_bg: {focus: 1.0} }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawDragNum {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    down: f32,
    #[live]
    focus: f32,
    #[live]
    disabled: f32,
    #[live]
    fill: f32,
    /// Hide the idle chip; hover/down/focus still reveal the editor surface.
    #[live]
    flat: f32,
}

#[derive(Clone, Debug, Default)]
pub enum DragNumberAction {
    /// Live while dragging or after a typed entry.
    Changed(f64),
    /// The gesture finished (mouse up / Enter) — commit points.
    Ended(f64),
    #[default]
    None,
}

// ===========================================================================
// The pure drag core — every mapping decision, no Cx anywhere
// ===========================================================================

/// The numeric contract one field carries into a drag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragParams {
    pub min: f64,
    pub max: f64,
    /// One arrow-click / wheel-step increment.
    pub step: f64,
    /// Cyclic: the value comes round at the ends instead of clamping.
    pub wrap: bool,
    /// Bounded mapping: the field's width sweeps the whole range.
    pub bounded: bool,
    /// Explicit Ctrl-snap increment; `0` picks a rung from the range.
    pub snap_override: f64,
}

impl DragParams {
    pub fn range(&self) -> f64 {
        self.max - self.min
    }
    fn has_range(&self) -> bool {
        self.max > self.min
    }
}

/// Where a drag measures from. Clamping and modifier changes move the
/// anchor rather than the value — that is what keeps the value attached to
/// the hand at the limits and across a Shift press.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragAnchor {
    pub x: f64,
    pub value: f64,
}

/// A press engages into a drag only past this much horizontal travel;
/// below it, the release is a click.
pub const DRAG_THRESHOLD: f64 = 3.0;

/// Value change per pixel for the current mapping and modifiers.
/// Bounded: the range across `width`, ×0.05 fine. Unbounded: one arrow-step
/// per pixel — the drag is the coarse gesture, Shift (×0.1) the fine one.
///
/// The rate used to be step×0.1 ("ten pixels per arrow click"), which made
/// every drag ten times finer than a click. On real fields that meant the
/// scene barely changed across a whole pull — Hour at step 0.06 crawled at
/// 0.006/px, four thousand pixels to cross one day — and the sun "did not
/// update" while dragging even though every step re-encoded and repainted.
/// A drag is the coarse gesture: one step per pixel keeps a 100 px pull
/// visibly moving the thing the dial controls, and Shift still recovers the
/// old fine rate exactly.
pub fn drag_rate(p: &DragParams, width: f64, shift: bool) -> f64 {
    if p.bounded && p.has_range() {
        let rate = p.range() / width.max(1.0);
        if shift {
            rate * 0.05
        } else {
            rate
        }
    } else {
        let rate = p.step;
        if shift {
            rate * 0.1
        } else {
            rate
        }
    }
}

/// The Ctrl-snap increment: an explicit override wins, otherwise a rung
/// sized to the range (a tenth, one, or ten), and Ctrl+Shift takes the
/// next finer rung. Rounds to the nearest multiple — never truncated
/// toward zero.
pub fn snap_increment(p: &DragParams, fine: bool) -> f64 {
    let base = if p.snap_override > 0.0 {
        p.snap_override
    } else {
        let range = if p.has_range() { p.range() } else { 21.0 };
        if range < 2.1 {
            0.1
        } else if range < 21.0 {
            1.0
        } else {
            10.0
        }
    };
    if fine {
        base * 0.1
    } else {
        base
    }
}

/// One step of the drag mapping: pointer at `x`, modifiers as held right
/// now. Returns the value to publish and the anchor to carry forward
/// (shifted when a limit was hit). Both ends stay reachable under snap.
pub fn drag_map(
    p: &DragParams,
    anchor: DragAnchor,
    x: f64,
    width: f64,
    shift: bool,
    ctrl: bool,
) -> (f64, DragAnchor) {
    let rate = drag_rate(p, width, shift);
    let raw = anchor.value + (x - anchor.x) * rate;

    // Range handling first, on the raw value, so the anchor tracks the
    // clamped/wrapped position.
    let (ranged, anchor) = if p.has_range() {
        if p.wrap {
            let wrapped = p.min + (raw - p.min).rem_euclid(p.range());
            // Re-anchor at every wrap so the anchor math never accumulates
            // whole turns.
            if (wrapped - raw).abs() > f64::EPSILON {
                (wrapped, DragAnchor { x, value: wrapped })
            } else {
                (raw, anchor)
            }
        } else {
            let clamped = raw.clamp(p.min, p.max);
            if (clamped - raw).abs() > f64::EPSILON {
                // Anchor shift: measure the rest of the drag from the limit.
                (clamped, DragAnchor { x, value: clamped })
            } else {
                (raw, anchor)
            }
        }
    } else {
        (raw, anchor)
    };

    // Snap the published value only; the anchor stays on the unsnapped
    // track so releasing Ctrl lands back on the pointer's own value.
    let mut publish = ranged;
    if ctrl {
        let inc = snap_increment(p, shift);
        if inc > 0.0 {
            publish = (ranged / inc).round() * inc;
            if p.has_range() && !p.wrap {
                publish = publish.clamp(p.min, p.max);
                // The exact ends stay reachable regardless of the snap grid.
                if ranged <= p.min {
                    publish = p.min;
                } else if ranged >= p.max {
                    publish = p.max;
                }
            }
        }
    }
    (publish, anchor)
}

/// Re-anchor for a modifier change: the value stays put at the current
/// pointer position, only the rate changes from here on.
pub fn reanchor(current_value: f64, x: f64) -> DragAnchor {
    DragAnchor {
        x,
        value: current_value,
    }
}

/// The three zones of the row: the stepping arrows at the ends and the
/// drag/edit surface between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldZone {
    Decrement,
    Middle,
    Increment,
}

/// Zone for a pointer at `x` within a row of `width`×`height`: each arrow
/// zone is a third of the width, capped at 0.7× the height.
pub fn field_zone(x: f64, width: f64, height: f64) -> FieldZone {
    let zone = (width / 3.0).min(height * 0.7);
    if x < zone {
        FieldZone::Decrement
    } else if x > width - zone {
        FieldZone::Increment
    } else {
        FieldZone::Middle
    }
}

// ===========================================================================
// The widget
// ===========================================================================

#[derive(Clone, Copy, Debug)]
struct DragState {
    /// Pointer x at press — the click-vs-drag threshold measures from here.
    press_x: f64,
    /// The value at press, restored on cancel.
    press_value: f64,
    /// The zone the press landed in, for the click path on release.
    #[allow(dead_code)] // recorded at press; the release path re-derives it today
    zone: FieldZone,
    /// Row width at press, for the bounded mapping.
    width: f64,
    /// Threshold crossed: the press is a drag now, never a click.
    engaged: bool,
    /// Where the mapping measures from (shifts at limits and on modifier
    /// changes).
    anchor: DragAnchor,
    /// The modifier factor last used, to detect mid-drag changes.
    shift: bool,
    /// The unsnapped value the anchor math tracks (published value may be
    /// snapped away from it while Ctrl is held).
    raw_value: f64,
}

#[derive(Script, Widget, Animator)]
pub struct FabDragNumber {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[apply_default]
    animator: Animator,
    #[redraw]
    #[live]
    draw_bg: DrawDragNum,
    #[live]
    draw_text: DrawText,
    #[live]
    text_input: TextInput,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[live]
    label: String,
    #[live]
    min: f64,
    #[live]
    max: f64,
    #[live(0.01)]
    step: f64,
    /// Explicit Ctrl-snap increment; `0` derives one from the range.
    #[live]
    snap: f64,
    #[live(2)]
    precision: usize,
    #[live]
    suffix: String,
    #[live]
    value: f64,
    #[live]
    wrap: bool,
    /// Bounded: the fill bar shows the value's place in the range and a
    /// drag sweeps the range across the row's width. Leave off for values
    /// whose range carries no meaning (a coordinate, a year).
    #[live]
    show_fill: bool,
    #[live]
    quantize: bool,
    /// Deprecated: Ctrl+Wheel always steps; plain wheel always scrolls.
    #[live]
    wheel: bool,
    /// Deprecated: a click without a drag always opens text entry.
    #[live]
    edit_on_double_click: bool,
    #[live]
    time_of_day: bool,

    #[rust]
    drag: Option<DragState>,
    #[rust]
    editing: bool,
}

impl ScriptHook for FabDragNumber {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        let text = self.format();
        vm.with_cx_mut(|cx| {
            self.text_input.set_is_numeric_only(cx, false);
            self.text_input.set_text(cx, &text);
            self.text_input.set_is_read_only(cx, true);
        });
    }
}

impl FabDragNumber {
    fn params(&self) -> DragParams {
        DragParams {
            min: self.min,
            max: self.max,
            step: self.step,
            wrap: self.wrap,
            bounded: self.show_fill,
            snap_override: self.snap,
        }
    }

    fn format(&self) -> String {
        if self.time_of_day {
            let minute = (self.value * 60.0).round().rem_euclid(24.0 * 60.0) as u32;
            return format!("{:02}:{:02}", minute / 60, minute % 60);
        }
        let v = match self.precision {
            0 => format!("{:.0}", self.value),
            1 => format!("{:.1}", self.value),
            2 => format!("{:.2}", self.value),
            3 => format!("{:.3}", self.value),
            _ => format!("{}", self.value),
        };
        if self.suffix.is_empty() {
            v
        } else {
            format!("{v}{}", self.suffix)
        }
    }

    /// The string offered for editing: full precision, trailing zeros
    /// trimmed, so opening and committing an edit can never silently round
    /// the stored value.
    fn format_full(&self) -> String {
        if self.time_of_day {
            return self.format();
        }
        let mut v = format!("{:.6}", self.value);
        if v.contains('.') {
            while v.ends_with('0') {
                v.pop();
            }
            if v.ends_with('.') {
                v.pop();
            }
        }
        v
    }

    fn normalize(&self, mut value: f64) -> f64 {
        if self.quantize && self.step > 0.0 {
            value = self.min + ((value - self.min) / self.step).round() * self.step;
        }
        if self.max <= self.min {
            return value;
        }
        if self.wrap {
            self.min + (value - self.min).rem_euclid(self.max - self.min)
        } else {
            value.clamp(self.min, self.max)
        }
    }

    fn parse(&self, text: &str) -> Option<f64> {
        if self.time_of_day {
            if let Some((hours, minutes)) = text.trim().split_once(':') {
                let hours = hours.trim().parse::<u32>().ok()?;
                let minutes = minutes.trim().parse::<u32>().ok()?;
                return (minutes < 60).then_some(hours as f64 + minutes as f64 / 60.0);
            }
        }
        let cleaned: String = text
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect();
        cleaned.parse::<f64>().ok()
    }

    fn sync_text(&mut self, cx: &mut Cx) {
        let t = self.format();
        self.text_input.set_text(cx, &t);
    }

    pub fn set_value(&mut self, cx: &mut Cx, v: f64) {
        let v = self.normalize(v);
        if (v - self.value).abs() > f64::EPSILON {
            self.value = v;
            self.sync_text(cx);
            self.draw_bg.redraw(cx);
        }
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// Publish a live value change (the same path a committed edit takes).
    fn publish(&mut self, cx: &mut Cx, uid: WidgetUid, v: f64, ended: bool) {
        if (v - self.value).abs() > f64::EPSILON {
            self.value = v;
            self.sync_text(cx);
            self.draw_bg.redraw(cx);
            cx.widget_action(uid, DragNumberAction::Changed(self.value));
        }
        if ended {
            cx.widget_action(uid, DragNumberAction::Ended(self.value));
        }
    }

    /// Step once (arrow click / Ctrl+Wheel), fine with Shift, clamped or
    /// wrapped by the field's own rules. A no-op at a limit publishes
    /// nothing.
    fn step_once(&mut self, cx: &mut Cx, uid: WidgetUid, direction: f64, shift: bool) {
        let step = if shift { self.step * 0.1 } else { self.step };
        let v = self.normalize(self.value + direction * step.max(f64::EPSILON));
        if (v - self.value).abs() > f64::EPSILON {
            self.publish(cx, uid, v, true);
        }
    }

    pub fn begin_edit(&mut self, cx: &mut Cx) {
        self.drag = None;
        self.editing = true;
        let full = self.format_full();
        self.text_input.set_is_numeric_only(cx, true);
        self.text_input.set_text(cx, &full);
        self.text_input.set_is_read_only(cx, false);
        self.text_input.set_key_focus(cx);
        self.text_input.select_all(cx);
        self.animator_play(cx, ids!(focus.on));
        self.draw_bg.redraw(cx);
    }

    fn end_edit(&mut self, cx: &mut Cx) {
        self.editing = false;
        self.text_input.set_is_read_only(cx, true);
        self.text_input.set_is_numeric_only(cx, false);
        self.sync_text(cx);
        self.animator_play(cx, ids!(focus.off));
        self.draw_bg.redraw(cx);
    }

    fn commit_edit_text(&mut self, cx: &mut Cx, uid: WidgetUid, text: &str) {
        if let Some(parsed) = self.parse(text) {
            let v = self.normalize(parsed);
            self.publish(cx, uid, v, true);
        }
        self.end_edit(cx);
    }

    fn cancel_drag(&mut self, cx: &mut Cx, uid: WidgetUid) {
        if let Some(drag) = self.drag.take() {
            if drag.engaged {
                self.publish(cx, uid, drag.press_value, false);
            }
            self.animator_play(cx, ids!(hover.off));
        }
    }
}

impl Widget for FabDragNumber {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // The fill claims "this range means something": only bounded fields
        // paint one.
        self.draw_bg.fill = if self.show_fill && self.max > self.min {
            (((self.value - self.min) / (self.max - self.min)) as f32).clamp(0.0, 1.0)
        } else {
            -1.0
        };
        self.draw_bg.begin(cx, walk, self.layout);
        if !self.label.is_empty() {
            // The label always spans exactly the space the value does not
            // need: the value lands right-anchored with real air between
            // them, and a tight row elides the label — the number is never
            // the one to give way.
            let row = cx.turtle().rect().size.x;
            let pad = self.layout.padding.left + self.layout.padding.right;
            let fs = self.draw_text.text_style.font_size as f64;
            let value_reserve =
                (self.format().chars().count() as f64 + 0.5) * fs * 0.72 + 6.0;
            let label_w = (row - pad - value_reserve).max(0.0);
            let mut label_walk = Walk::fit();
            label_walk.width = Size::Fixed(label_w);
            self.draw_text
                .draw_walk(cx, label_walk, Align::default(), &self.label);
        }
        let iw = self.text_input.walk(cx);
        let _ = self.text_input.draw_walk(cx, &mut Scope::empty(), iw);
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);

        // Ctrl+Wheel nudges by one step; a plain wheel keeps scrolling the
        // panel underneath.
        if let Event::Scroll(e) = event {
            if e.modifiers.control || e.modifiers.logo {
                if !e.handled_y.get()
                    && e.scroll.y.abs() > f64::EPSILON
                    && self.draw_bg.area().rect(cx).contains(e.abs)
                {
                    let direction = if e.scroll.y < 0.0 { 1.0 } else { -1.0 };
                    self.step_once(cx, uid, direction, e.modifiers.shift);
                    e.handled_y.set(true);
                }
            }
        }

        // Escape or a right-button press cancels an in-flight drag and
        // restores the pressed value.
        if self.drag.is_some() {
            match event {
                Event::KeyDown(ke) if ke.key_code == KeyCode::Escape => {
                    self.cancel_drag(cx, uid);
                    return;
                }
                Event::MouseDown(me) if me.button.is_secondary() => {
                    self.cancel_drag(cx, uid);
                    return;
                }
                _ => {}
            }
        }

        // The embedded input is a display until a click opens it: while it
        // is not editing it receives no events at all — otherwise it claims
        // the press for text selection and the drag never sees a single
        // FingerMove (the "dragging barely moves the value" bug).
        if self.editing {
            // Focus ownership is the state boundary, not merely an action we
            // hope to capture. A parent can consume the TextInput's emitted
            // KeyFocusLost action while focus itself has already moved; in
            // that case the old code left `editing` latched forever. Commit
            // valid text (invalid text naturally restores `self.value`) and
            // return to the read-only drag display immediately.
            let input_area = self.text_input.area();
            if input_area != Area::Empty && !cx.has_key_focus(input_area) {
                let text = self.text_input.text().to_string();
                self.commit_edit_text(cx, uid, &text);
                return;
            }
            for action in cx.capture_actions(|cx| self.text_input.handle_event(cx, event, scope)) {
                match action.as_widget_action().cast() {
                    TextInputAction::KeyFocus => {
                        self.animator_play(cx, ids!(focus.on));
                    }
                    TextInputAction::KeyFocusLost => {
                        // Clicking elsewhere commits, like Enter; only
                        // Escape cancels.
                        if self.editing {
                            let text = self.text_input.text().to_string();
                            self.commit_edit_text(cx, uid, &text);
                        }
                    }
                    TextInputAction::Returned(v, _) => {
                        if self.editing {
                            self.commit_edit_text(cx, uid, &v);
                            cx.revert_key_focus();
                        }
                    }
                    TextInputAction::Escaped => {
                        if self.editing {
                            self.end_edit(cx);
                            cx.revert_key_focus();
                        }
                    }
                    _ => {}
                }
            }
        }

        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) => {
                let rect = self.draw_bg.area().rect(cx);
                let zone = field_zone(fe.abs.x - rect.pos.x, rect.size.x, rect.size.y);
                cx.set_cursor(match zone {
                    FieldZone::Middle => MouseCursor::EwResize,
                    _ => MouseCursor::Default,
                });
                self.animator_play(cx, ids!(hover.on));
            }
            Hit::FingerHoverOver(fe) => {
                if self.drag.is_none() && !self.editing {
                    let rect = self.draw_bg.area().rect(cx);
                    let zone = field_zone(fe.abs.x - rect.pos.x, rect.size.x, rect.size.y);
                    cx.set_cursor(match zone {
                        FieldZone::Middle => MouseCursor::EwResize,
                        _ => MouseCursor::Default,
                    });
                }
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() && !self.editing => {
                let rect = self.draw_bg.area().rect(cx);
                let zone = field_zone(fe.abs.x - rect.pos.x, rect.size.x, rect.size.y);
                // Press changes nothing: it only arms. The text input is
                // left alone so the pointer capture stays quiet until the
                // release decides click or the threshold decides drag.
                self.drag = Some(DragState {
                    press_x: fe.abs.x,
                    press_value: self.value,
                    zone,
                    width: rect.size.x,
                    engaged: false,
                    anchor: DragAnchor {
                        x: fe.abs.x,
                        value: self.value,
                    },
                    shift: fe.modifiers.shift,
                    raw_value: self.value,
                });
                self.animator_play(cx, ids!(hover.down));
            }
            Hit::FingerMove(fe) => {
                let Some(mut drag) = self.drag else {
                    return;
                };
                if !drag.engaged {
                    if (fe.abs.x - drag.press_x).abs() < DRAG_THRESHOLD {
                        return;
                    }
                    // Engage at the pointer, discarding the threshold
                    // distance: the first dragged pixel is a small change.
                    drag.engaged = true;
                    drag.anchor = reanchor(self.value, fe.abs.x);
                    drag.raw_value = self.value;
                }
                let mods = cx.keyboard.modifiers();
                if mods.shift != drag.shift {
                    // A modifier change re-anchors: the value holds still,
                    // only the rate changes from here.
                    drag.shift = mods.shift;
                    drag.anchor = reanchor(drag.raw_value, fe.abs.x);
                }
                let params = self.params();
                let (publish, anchor) = drag_map(
                    &params,
                    drag.anchor,
                    fe.abs.x,
                    drag.width,
                    mods.shift,
                    mods.control | mods.logo,
                );
                drag.raw_value =
                    anchor.value + (fe.abs.x - anchor.x) * drag_rate(&params, drag.width, mods.shift);
                if params.has_range() && !params.wrap {
                    drag.raw_value = drag.raw_value.clamp(params.min, params.max);
                }
                drag.anchor = anchor;
                self.drag = Some(drag);
                let v = self.normalize(publish);
                self.publish(cx, uid, v, false);
            }
            Hit::FingerUp(fe) => {
                let Some(drag) = self.drag.take() else {
                    return;
                };
                if drag.engaged {
                    cx.widget_action(uid, DragNumberAction::Ended(self.value));
                } else {
                    // A click. The zone at release decides: arrows step,
                    // the middle opens text entry with the value selected.
                    let rect = self.draw_bg.area().rect(cx);
                    let zone = field_zone(fe.abs.x - rect.pos.x, rect.size.x, rect.size.y);
                    match zone {
                        FieldZone::Decrement => self.step_once(cx, uid, -1.0, fe.modifiers.shift),
                        FieldZone::Increment => self.step_once(cx, uid, 1.0, fe.modifiers.shift),
                        FieldZone::Middle => self.begin_edit(cx),
                    }
                    let _ = drag;
                }
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, ids!(hover.on));
                } else {
                    self.animator_play(cx, ids!(hover.off));
                }
            }
            _ => {}
        }
    }
}

impl FabDragNumberRef {
    pub fn changed(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let DragNumberAction::Changed(v) = item.cast() {
                return Some(v);
            }
        }
        None
    }

    pub fn ended(&self, actions: &Actions) -> Option<f64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let DragNumberAction::Ended(v) = item.cast() {
                return Some(v);
            }
        }
        None
    }

    pub fn set_value(&self, cx: &mut Cx, v: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_value(cx, v);
        }
    }

    pub fn value(&self) -> f64 {
        self.borrow().map_or(0.0, |i| i.value())
    }

    pub fn begin_edit(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.begin_edit(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounded(min: f64, max: f64, wrap: bool) -> DragParams {
        DragParams {
            min,
            max,
            step: 0.25,
            wrap,
            bounded: true,
            snap_override: 0.0,
        }
    }

    fn unbounded(step: f64) -> DragParams {
        DragParams {
            min: 0.0,
            max: 0.0,
            step,
            wrap: false,
            bounded: false,
            snap_override: 0.0,
        }
    }

    #[test]
    fn a_bounded_field_sweeps_its_range_across_its_width() {
        let p = bounded(0.0, 24.0, false);
        let a = DragAnchor { x: 0.0, value: 12.0 };
        // Half the width to the right = half the range up.
        let (v, _) = drag_map(&p, a, 100.0, 200.0, false, false);
        assert!((v - 24.0).abs() < 1e-9, "{v}");
        let (v, _) = drag_map(&p, a, 50.0, 200.0, false, false);
        assert!((v - 18.0).abs() < 1e-9, "{v}");
        // Shift is twenty times finer.
        let (v, _) = drag_map(&p, a, 50.0, 200.0, true, false);
        assert!((v - 12.3).abs() < 1e-9, "{v}");
    }

    #[test]
    fn an_unbounded_field_moves_by_pixels_times_step() {
        let p = unbounded(1.0);
        let a = DragAnchor { x: 0.0, value: 2000.0 };
        // One step per pixel; a hundred pixels is a hundred steps.
        let (v, _) = drag_map(&p, a, 100.0, 400.0, false, false);
        assert!((v - 2100.0).abs() < 1e-9, "{v}");
        // Shift is ten times finer.
        let (v, _) = drag_map(&p, a, 100.0, 400.0, true, false);
        assert!((v - 2010.0).abs() < 1e-9, "{v}");
        // The width of the row is irrelevant to an unbounded mapping.
        let (w, _) = drag_map(&p, a, 100.0, 40.0, false, false);
        assert!((w - v - 90.0).abs() < 1e-9);
    }

    #[test]
    fn the_scene_hour_field_sweeps_hours_per_pull_not_minutes() {
        // The reported defect: the Scene-tab Hour field (min 0, max 24,
        // step 0.06, no fill) crawled at step×0.1 = 0.006/px — an ordinary
        // 100 px pull moved the sun by 36 minutes, and the picture "did not
        // update" during the drag even though every step repainted. The
        // drag has to move the value at dial speed: hours per pull.
        let p = DragParams {
            min: 0.0,
            max: 24.0,
            step: 0.06,
            wrap: false,
            bounded: false,
            snap_override: 1.0,
        };
        let a = DragAnchor { x: 0.0, value: 14.0 };
        let (v, _) = drag_map(&p, a, 100.0, 305.0, false, false);
        assert!(
            (14.0 - v).abs() >= 4.0,
            "a 100px pull must sweep hours, got {v}"
        );
        // Shift keeps the old fine rate for precision work.
        let (v, _) = drag_map(&p, a, 100.0, 305.0, true, false);
        assert!((v - 14.6).abs() < 1e-9, "{v}");
    }

    #[test]
    fn clamping_shifts_the_anchor_so_reversal_moves_immediately() {
        let p = bounded(0.0, 1.0, false);
        let a = DragAnchor { x: 0.0, value: 0.5 };
        // Overshoot far past the maximum...
        let (v, a2) = drag_map(&p, a, 300.0, 100.0, false, false);
        assert!((v - 1.0).abs() < 1e-9);
        assert_eq!(a2.x, 300.0);
        assert_eq!(a2.value, 1.0);
        // ...and the very next pixel back already leaves the limit.
        let (v, _) = drag_map(&p, a2, 299.0, 100.0, false, false);
        assert!(v < 1.0, "{v}");
        assert!((v - 0.99).abs() < 1e-9, "{v}");
    }

    #[test]
    fn cyclic_fields_wrap_at_their_ends() {
        // The hour comes round at 24.
        let p = bounded(0.0, 24.0, true);
        let a = DragAnchor { x: 0.0, value: 23.0 };
        let (v, _) = drag_map(&p, a, 100.0, 1200.0, false, false);
        assert!((v - 1.0).abs() < 1e-9, "{v}");
        let a = DragAnchor { x: 0.0, value: 1.0 };
        let (v, _) = drag_map(&p, a, -100.0, 1200.0, false, false);
        assert!((v - 23.0).abs() < 1e-9, "{v}");
        // A month field wraps at 12 the same way.
        let p = bounded(1.0, 13.0, true);
        let a = DragAnchor { x: 0.0, value: 12.0 };
        let (v, _) = drag_map(&p, a, 200.0, 1200.0, false, false);
        assert!((v - 2.0).abs() < 1e-9, "{v}");
    }

    #[test]
    fn ctrl_snaps_to_a_range_sized_rung_and_rounds_to_nearest() {
        // A wide range snaps by ten — and −15 rounds to −20, not to −10.
        let p = DragParams {
            min: -180.0,
            max: 180.0,
            step: 1.0,
            wrap: false,
            bounded: true,
            snap_override: 0.0,
        };
        let a = DragAnchor { x: 0.0, value: 0.0 };
        let width = 360.0; // one pixel per unit for easy arithmetic
        let (v, _) = drag_map(&p, a, -15.0, width, false, true);
        assert!((v + 20.0).abs() < 1e-9, "{v}");
        // Ctrl+Shift takes the finer rung.
        let (v, _) = drag_map(&p, a, -15.0, width, true, true);
        // Shift also scales the rate (×0.05): −15 px = −0.75 units,
        // snapped by 1 → −1.
        assert!((v + 1.0).abs() < 1e-9, "{v}");
        // The exact ends stay reachable under snap.
        let (v, _) = drag_map(&p, a, 100000.0, width, false, true);
        assert!((v - 180.0).abs() < 1e-9, "{v}");
    }

    #[test]
    fn a_modifier_change_reanchors_instead_of_jumping() {
        let p = unbounded(1.0);
        // Drag 100 px: value 2100.
        let a = DragAnchor { x: 0.0, value: 2000.0 };
        let (v, _) = drag_map(&p, a, 100.0, 400.0, false, false);
        assert!((v - 2100.0).abs() < 1e-9);
        // Shift goes down: re-anchor at the pointer, value unchanged...
        let a2 = reanchor(v, 100.0);
        let (v2, _) = drag_map(&p, a2, 100.0, 400.0, true, false);
        assert!((v2 - v).abs() < 1e-9, "no jump on modifier change");
        // ...and further motion is ten times finer.
        let (v3, _) = drag_map(&p, a2, 110.0, 400.0, true, false);
        assert!((v3 - v - 1.0).abs() < 1e-9, "{v3}");
    }

    #[test]
    fn the_zones_split_arrows_from_the_drag_surface() {
        // 20-px-high row: each arrow zone is 14 px.
        assert_eq!(field_zone(5.0, 200.0, 20.0), FieldZone::Decrement);
        assert_eq!(field_zone(100.0, 200.0, 20.0), FieldZone::Middle);
        assert_eq!(field_zone(195.0, 200.0, 20.0), FieldZone::Increment);
        // A narrow row caps the zones at a third of the width.
        assert_eq!(field_zone(11.0, 30.0, 20.0), FieldZone::Middle);
        assert_eq!(field_zone(2.0, 30.0, 20.0), FieldZone::Decrement);
    }

    #[test]
    fn the_click_threshold_is_three_horizontal_pixels() {
        assert!(DRAG_THRESHOLD == 3.0);
    }
}
