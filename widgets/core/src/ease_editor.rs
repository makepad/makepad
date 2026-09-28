//! EaseCurve and EaseEditor: a cubic-bezier easing curve, shown and edited.
//!
//! * [`EaseCurve`] draws one ease on a unit grid: the curve as a polyline of
//!   [`EASE_CURVE_SEGMENTS`] segments whose x steps follow the cubic's own
//!   parameter (`X(u)` at uniform `u`, so a near-vertical stretch gets as many
//!   segments as a flat one) and whose y comes from the RUNTIME solver (the
//!   parity [`Easing::Bezier`] that `Ease::Bezier` runs through in the
//!   Animator and in tweens), so what is drawn is what plays. Design section
//!   11 asks for the parametric `(X(u), Y(u))`; the y deviates on purpose, by
//!   at most about 0.011 (ease_in_out_back). A stretch past the widget is
//!   clipped, not flattened onto the edge. A preview dot runs along the
//!   curve (1 s, hold 0.4 s, loop) on a [`TweenClock`] chain that patches one
//!   instance per frame (no redraw, no layout, no allocation) and stops as soon
//!   as the curve is no longer drawn. With `editable: true` the two control
//!   points P1 and P2 become draggable handles.
//! * [`EaseEditor`] is the curve plus a preset picker over
//!   [`CSS_PRESETS`](crate::tween::CSS_PRESETS), four numeric fields
//!   (x1, y1, x2, y2) and a `cubic-bezier(..)` readout. It reports
//!   [`EaseEditorAction::Changed`] while a value moves and
//!   [`EaseEditorAction::Committed`] when a gesture ends, both carrying
//!   `Ease::Bezier { cp0: x1, cp1: y1, cp2: x2, cp3: y2 }`.
//!
//! x is clamped to 0..=1 (CSS requires it, and it keeps the curve a function
//! of time); y may leave the unit square (-1..=2) for back and overshoot
//! curves. A handle dragged past the value range (`y_lo..=y_hi`) stops on it,
//! and a handle whose value lies outside it is drawn on it; the fields reach
//! the whole range.
//!
//! Input: drag a handle (the cursor says Grab over one); a double tap resets to
//! the last picked preset (a press that follows a drag is a new grab, never
//! the second tap); with the canvas focused the arrow keys nudge the
//! hovered (or last used) handle by 0.01, 0.1 with Shift.
use crate::event::TouchState;
use crate::{
    animator::Ease,
    badge::measure,
    drop_down::DropDownWidgetRefExt,
    label::LabelWidgetRefExt,
    makepad_derive_widget::*,
    makepad_draw::*,
    tween::{tween_ticker_ref, Easing, TweenClock, CSS_PRESETS},
    value_input::{ValueInputAction, ValueInputWidgetRefExt},
    view::View,
    widget::*,
};
use std::fmt::Write;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawEaseCurve::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    set_type_default() do #(DrawEaseSegment::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn(){
            let p = self.pos * self.rect_size
            let pa = p - self.seg_a
            let ba = self.seg_b - self.seg_a
            let h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0)
            let d = length(pa - ba * h)
            let half = self.thickness * 0.5
            let aa = 1.0 - smoothstep(half - 0.6, half + 0.6, d)
            return vec4(self.color.rgb * self.color.a * aa, self.color.a * aa)
        }
    }

    mod.widgets.EaseCurveBase = #(EaseCurve::register_widget(vm))

    /** An easing curve on a unit grid, with an animated preview dot. */
    mod.widgets.EaseCurve = set_type_default() do mod.widgets.EaseCurveBase{
        width: Fill
        height: 240
        /** the ease shown until a host sets another */
        ease: Ease.Bezier{cp0: 0.25 cp1: 0.1 cp2: 0.25 cp3: 1.0}
        /** drag the two control points */
        editable: false
        /** run the preview dot along the curve */
        preview: true
        /** preview travel time in seconds 0.1..5 step 0.1 */
        preview_secs: 1.0
        /** pause at the end before the dot loops, seconds 0..3 step 0.1 */
        preview_hold: 0.4
        /** room around the value range in pixels 0..40 step 1 */
        pad: 8.0
        /** handle radius in pixels 2..16 step 0.5 */
        handle_radius: 6.0
        /** how far from a handle a press still takes it, pixels 4..24 step 1 */
        grab_radius: 8.0
        /** lowest value shown -1..0 step 0.05 */
        y_lo: -0.6
        /** highest value shown 1..2 step 0.05 */
        y_hi: 1.6
        /** curve stroke width in pixels 0.5..6 step 0.25 */
        line_width: 2.0

        draw_bg +: {
            color: uniform(theme.color_inset)
            border_color: uniform(theme.color_bevel)
            grid_color: uniform(theme.color_outline_variant)
            square_color: uniform(theme.color_outline)
            arm_color: uniform(theme.color_on_surface_variant)
            radius: uniform(theme.corner_radius)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let sdf = Sdf2d.viewport(p)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, 1.0)
                let mut c = sdf.result
                let s = max(self.sq.z, 1.0)
                let ux = (p.x - self.sq.x) / s
                let uy = (p.y - self.sq.y) / s
                let tol = 0.75 / s
                let inside = step(-tol, ux) * step(ux, 1.0 + tol) * step(-tol, uy) * step(uy, 1.0 + tol)
                let gx = abs(fract(ux * 4.0 + 0.5) - 0.5) * s * 0.25
                let gy = abs(fract(uy * 4.0 + 0.5) - 0.5) * s * 0.25
                let grid = (1.0 - clamp(min(gx, gy), 0.0, 1.0)) * inside
                c = mix(c, vec4(self.grid_color.rgb, 1.0), grid * self.grid_color.a)
                let ex = abs(fract(ux + 0.5) - 0.5) * s
                let ey = abs(fract(uy + 0.5) - 0.5) * s
                let edge = (1.0 - clamp(min(ex, ey), 0.0, 1.0)) * inside
                c = mix(c, vec4(self.square_color.rgb, 1.0), edge * self.square_color.a)
                let pa = p - self.p0
                let ba = self.p1 - self.p0
                let ha = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0)
                let da = length(pa - ba * ha)
                let pb = p - self.p3
                let bb = self.p2 - self.p3
                let hb = clamp(dot(pb, bb) / max(dot(bb, bb), 0.0001), 0.0, 1.0)
                let db = length(pb - bb * hb)
                let arm = (1.0 - clamp(min(da, db) - 0.25, 0.0, 1.0)) * self.handles_on
                c = mix(c, vec4(self.arm_color.rgb, 1.0), arm * self.arm_color.a)
                return c
            }
        }
        draw_curve +: {
            color: theme.color_primary
        }
        draw_knobs +: {
            handle_color: uniform(theme.color_handle)
            handle_color_hover: uniform(theme.color_handle_hover)
            ring_color: uniform(theme.color_primary)
            dot_color: uniform(theme.color_primary)
            rail_color: uniform(theme.color_on_surface_variant)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let sdf = Sdf2d.viewport(p)
                sdf.rect(self.rail_x - 0.5, self.sq.y, 1.0, self.sq.z)
                sdf.fill(self.rail_color * (0.5 * self.dot_on))
                sdf.circle(self.rail_x, self.dot.y, 3.0)
                sdf.fill(self.dot_color * self.dot_on)
                sdf.circle(self.dot.x, self.dot.y, 4.0)
                sdf.fill(self.dot_color * self.dot_on)
                let r = self.handle_r
                sdf.circle(self.p1.x, self.p1.y, r + self.hot1 * 1.5)
                sdf.fill_keep(self.handle_color.mix(self.handle_color_hover, self.hot1) * self.handles_on)
                sdf.stroke(self.ring_color * self.handles_on, 1.5)
                sdf.circle(self.p2.x, self.p2.y, r + self.hot2 * 1.5)
                sdf.fill_keep(self.handle_color.mix(self.handle_color_hover, self.hot2) * self.handles_on)
                sdf.stroke(self.ring_color * self.handles_on, 1.5)
                return sdf.result
            }
        }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p}
        }
    }

    mod.widgets.EaseEditorBase = #(EaseEditor::register_widget(vm))

    /** A cubic-bezier ease editor: presets, a draggable curve, four numbers
     * and the CSS readout. */
    mod.widgets.EaseEditor = set_type_default() do mod.widgets.EaseEditorBase{
        width: 300
        height: Fit
        flow: Down
        spacing: theme.space_2
        /** the ease shown until a host sets another */
        ease: Ease.Bezier{cp0: 0.25 cp1: 0.1 cp2: 0.25 cp3: 1.0}

        preset := DropDown{
            width: Fill
        }
        curve := mod.widgets.EaseCurve{
            width: Fill
            height: 300
            editable: true
        }
        fields := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_1
            x1 := ValueInput{width: Fill min: 0.0 max: 1.0 step: 0.01 precision: 3.0}
            y1 := ValueInput{width: Fill min: -1.0 max: 2.0 step: 0.01 precision: 3.0}
            x2 := ValueInput{width: Fill min: 0.0 max: 1.0 step: 0.01 precision: 3.0}
            y2 := ValueInput{width: Fill min: -1.0 max: 2.0 step: 0.01 precision: 3.0}
        }
        readout := Label{
            width: Fill
            text: "cubic-bezier(0.25, 0.1, 0.25, 1)"
            draw_text +: {
                color: theme.color_text
                text_style: theme.font_code
            }
        }
    }
}

/// Segments of the curve polyline (the issue 786 editor draws 64).
pub const EASE_CURVE_SEGMENTS: usize = 64;

/// The drop-down row shown when the points match no preset.
const CUSTOM_ROW: usize = CSS_PRESETS.len();

/// The x range of a control point (CSS: a function of time).
const X_RANGE: (f64, f64) = (0.0, 1.0);
/// The y range of a control point (the back presets reach -0.55 and 1.55).
const Y_RANGE: (f64, f64) = (-1.0, 2.0);

/// The grid, the unit square and the two control arms (`draw_bg`), or the
/// handles and the preview dot (`draw_knobs`): one shader struct, two pixel
/// functions. Every position is in the quad's local pixels, y down.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawEaseCurve {
    #[deref]
    draw_super: DrawQuad,
    /// The unit square: left, top (unit y = 1), side, unused.
    #[live]
    pub sq: Vec4f,
    #[live]
    pub p0: Vec2f,
    #[live]
    pub p1: Vec2f,
    #[live]
    pub p2: Vec2f,
    #[live]
    pub p3: Vec2f,
    #[live]
    pub hot1: f32,
    #[live]
    pub hot2: f32,
    #[live]
    pub handles_on: f32,
    #[live]
    pub handle_r: f32,
    #[live]
    pub dot: Vec2f,
    #[live]
    pub dot_on: f32,
    #[live]
    pub rail_x: f32,
}

/// One anti-aliased segment of the curve polyline (a capsule distance).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawEaseSegment {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub seg_a: Vec2f,
    #[live]
    pub seg_b: Vec2f,
    #[live]
    pub color: Vec4f,
    #[live(2.0)]
    pub thickness: f32,
}

/// One of the two control points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EaseHandle {
    /// P1 = (x1, y1), joined to (0, 0).
    #[default]
    P1,
    /// P2 = (x2, y2), joined to (1, 1).
    P2,
}

/// What an [`EaseCurve`] with `editable: true` reports, as `[x1, y1, x2, y2]`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum EaseCurveAction {
    /// A handle moved (every drag step, every key nudge).
    Changed([f64; 4]),
    /// A gesture ended: finger up after a drag, a key nudge, a double-tap reset.
    Committed([f64; 4]),
    #[default]
    None,
}

/// What an [`EaseEditor`] reports. The value is always `Ease::Bezier`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum EaseEditorAction {
    /// The value is moving: a drag step, a field scrub step.
    Changed(Ease),
    /// The value settled: finger up, a field commit, a preset pick, a key
    /// nudge, a double-tap reset. Every Committed follows a Changed of the
    /// same value (in the same or an earlier action batch).
    Committed(Ease),
    #[default]
    None,
}

/// Unit space to canvas pixels and back: a square unit cell sized so the
/// whole `y_lo..=y_hi` range fits, centred. The hit test and the shaders use
/// the same numbers, so what is drawn is what is grabbable.
#[derive(Clone, Copy, Debug)]
struct Canvas {
    origin: DVec2,
    side: f64,
    /// The value range shown, never inverted (`y_hi - y_lo >= 1`).
    y_lo: f64,
    y_hi: f64,
    size: DVec2,
}

impl Canvas {
    fn new(size: DVec2, pad: f64, y_lo: f64, y_hi: f64) -> Self {
        let span = (y_hi - y_lo).max(1.0);
        let side = (size.x - 2.0 * pad)
            .min((size.y - 2.0 * pad) / span)
            .max(1.0);
        let origin = dvec2((size.x - side) * 0.5, (size.y - side * span) * 0.5);
        Self {
            origin,
            side,
            y_lo: y_hi - span,
            y_hi,
            size,
        }
    }

    fn to_px(&self, x: f64, y: f64) -> DVec2 {
        dvec2(
            self.origin.x + x * self.side,
            self.origin.y + (self.y_hi - y) * self.side,
        )
    }

    fn to_unit(&self, p: DVec2) -> (f64, f64) {
        (
            (p.x - self.origin.x) / self.side,
            self.y_hi - (p.y - self.origin.y) / self.side,
        )
    }

    /// `p` pulled inside the widget (the preview dot past the value range
    /// stays at the edge rather than drawing outside it).
    fn inside(&self, p: DVec2) -> DVec2 {
        dvec2(
            p.x.clamp(0.0, self.size.x.max(0.0)),
            p.y.clamp(0.0, self.size.y.max(0.0)),
        )
    }

    /// The y range a drag may reach: the value range shown, within -1..=2.
    fn drag_y(&self) -> (f64, f64) {
        (self.y_lo.max(Y_RANGE.0), self.y_hi.min(Y_RANGE.1))
    }

    /// Where handle `h` is drawn: at its value, or on the edge of the value
    /// range when its y lies outside it (a field can type y2 = 2 while
    /// `y_hi` is 1.6), so a handle is always whole and grabbable.
    fn handle_px(&self, pts: &[f64; 4], h: EaseHandle) -> DVec2 {
        let (x, y) = match h {
            EaseHandle::P1 => (pts[0], pts[1]),
            EaseHandle::P2 => (pts[2], pts[3]),
        };
        self.inside(self.to_px(x, y.clamp(self.y_lo, self.y_hi)))
    }

    /// The part of the segment `a`-`b` inside the widget, inset by `inset`
    /// (Liang-Barsky), or `None` when none of it is.
    fn clip(&self, a: DVec2, b: DVec2, inset: f64) -> Option<(DVec2, DVec2)> {
        let d = b - a;
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for (p, q) in [
            (-d.x, a.x - inset),
            (d.x, self.size.x - inset - a.x),
            (-d.y, a.y - inset),
            (d.y, self.size.y - inset - a.y),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    t0 = t0.max(r);
                } else {
                    t1 = t1.min(r);
                }
            }
        }
        if t0 > t1 {
            None
        } else {
            Some((a + d * t0, a + d * t1))
        }
    }

    /// The handle under `p` within `radius`, the nearer one when both are.
    fn pick(&self, p: DVec2, pts: &[f64; 4], radius: f64) -> Option<EaseHandle> {
        let d1 = p.distance(&self.handle_px(pts, EaseHandle::P1));
        let d2 = p.distance(&self.handle_px(pts, EaseHandle::P2));
        if d1.min(d2) > radius {
            None
        } else if d2 < d1 {
            Some(EaseHandle::P2)
        } else {
            Some(EaseHandle::P1)
        }
    }
}

fn to_f32(p: DVec2) -> Vec2f {
    vec2(p.x as f32, p.y as f32)
}

/// `[x1, y1, x2, y2]` with x in 0..=1 and y in -1..=2 (NaN becomes 0).
pub fn clamp_ease_points(p: [f64; 4]) -> [f64; 4] {
    let c = |v: f64, (lo, hi): (f64, f64)| if v.is_nan() { 0.0 } else { v.clamp(lo, hi) };
    [
        c(p[0], X_RANGE),
        c(p[1], Y_RANGE),
        c(p[2], X_RANGE),
        c(p[3], Y_RANGE),
    ]
}

/// The runtime (parity) easing of four control points: `Ease::Bezier`'s solver.
fn bezier_easing(p: [f64; 4]) -> Easing {
    Easing::Bezier {
        x1: p[0],
        y1: p[1],
        x2: p[2],
        y2: p[3],
    }
}

/// `Ease::Bezier` of four control points.
fn bezier_ease(p: [f64; 4]) -> Ease {
    Ease::Bezier {
        cp0: p[0],
        cp1: p[1],
        cp2: p[2],
        cp3: p[3],
    }
}

/// The [`CSS_PRESETS`] row whose points are `p`, if any.
fn preset_index(p: [f64; 4]) -> Option<usize> {
    CSS_PRESETS
        .iter()
        .position(|(_, q)| q.iter().zip(p.iter()).all(|(a, b)| (a - b).abs() < 1e-9))
}

/// Appends `v` rounded to 3 decimals without trailing zeros (`0.25`, `1`,
/// `-0.55`), the way CSS values are usually written.
fn push_num(out: &mut String, v: f64) {
    let v = (v * 1000.0).round() / 1000.0 + 0.0;
    let start = out.len();
    let _ = write!(out, "{:.3}", v);
    while out.len() > start && out.ends_with('0') {
        out.pop();
    }
    if out.ends_with('.') {
        out.pop();
    }
}

/// Writes `cubic-bezier(x1, y1, x2, y2)` into `out` (cleared first; the
/// buffer's capacity is reused).
pub fn write_cubic_bezier(out: &mut String, p: [f64; 4]) {
    out.clear();
    out.push_str("cubic-bezier(");
    for (i, v) in p.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        push_num(out, *v);
    }
    out.push(')');
}

/// Whether `event` is a pointer move (a ValueInput scrub reports on these).
fn pointer_moved(event: &Event) -> bool {
    match event {
        Event::MouseMove(_) => true,
        Event::TouchUpdate(te) => te.touches.iter().any(|t| t.state == TouchState::Move),
        _ => false,
    }
}

/// Whether `event` lifts a pointer (the end of a ValueInput scrub).
fn pointer_released(event: &Event) -> bool {
    match event {
        Event::MouseUp(_) => true,
        Event::TouchUpdate(te) => te.touches.iter().any(|t| t.state == TouchState::Stop),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// EaseCurve
// ---------------------------------------------------------------------------

/// An easing curve on a unit grid. Read-only by default; `editable: true`
/// adds the two draggable control points.
#[derive(Script, Widget)]
pub struct EaseCurve {
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
    draw_bg: DrawEaseCurve,
    #[live]
    draw_curve: DrawEaseSegment,
    #[live]
    draw_knobs: DrawEaseCurve,
    #[live]
    draw_text: DrawText,

    /// The ease the DSL sets. Adopted when it changes; the live value is
    /// [`EaseCurve::easing`] / [`EaseCurve::points`].
    #[live(Ease::Bezier{cp0: 0.25, cp1: 0.1, cp2: 0.25, cp3: 1.0})]
    ease: Ease,
    /// Drag the two control points.
    #[live]
    pub editable: bool,
    /// Run the preview dot along the curve.
    #[live(true)]
    pub preview: bool,
    #[live(1.0)]
    pub preview_secs: f64,
    #[live(0.4)]
    pub preview_hold: f64,
    #[live(8.0)]
    pub pad: f64,
    #[live(6.0)]
    pub handle_radius: f64,
    #[live(8.0)]
    pub grab_radius: f64,
    #[live(-0.6)]
    pub y_lo: f64,
    #[live(1.6)]
    pub y_hi: f64,
    #[live(2.0)]
    pub line_width: f64,

    /// The DSL `ease` last adopted.
    #[rust]
    applied: Option<Ease>,
    /// The control points (the last ones, while a sampled ease is shown).
    #[rust([0.0, 0.0, 1.0, 1.0])]
    pts: [f64; 4],
    /// Where a double tap puts the points back.
    #[rust]
    home: [f64; 4],
    /// What the curve and the dot evaluate: `Easing::Bezier` of `pts`, or the
    /// sampled ease.
    #[rust]
    easing: Easing,
    /// The curve is a non-bezier ease sampled from `Easing::map`: no handles.
    #[rust]
    sampled: bool,
    /// The dragged handle and the press offset from its centre.
    #[rust]
    grab: Option<(EaseHandle, DVec2)>,
    #[rust]
    moved: bool,
    /// The last press ended a drag: the next press is a new grab even when
    /// the platform counts it as the second tap.
    #[rust]
    last_drag: bool,
    #[rust]
    hot: Option<EaseHandle>,
    /// The handle the arrow keys move when none is hovered.
    #[rust]
    active: EaseHandle,
    #[rust]
    clock: TweenClock,
    /// Seconds into the preview loop.
    #[rust]
    phase: f64,
    /// The handle tip ("x1 0.25  y1 0.10"), rebuilt only when it changes.
    #[rust]
    tip: String,
}

impl ScriptHook for EaseCurve {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if self.applied != Some(self.ease) {
            self.applied = Some(self.ease);
            self.adopt(Easing::from(&self.ease));
            self.home = self.pts;
            self.draw_bg.redraw(vm.cx_mut());
        }
    }
}

impl EaseCurve {
    /// The canvas of a widget of `size`. `y_lo` / `y_hi` are plain DSL
    /// numbers: they are taken into -1..=2 here (NaN as the default), and
    /// `Canvas` keeps the range at least 1 high, so it is never inverted.
    fn canvas(&self, size: DVec2) -> Canvas {
        let y = |v: f64, d: f64| {
            if v.is_nan() {
                d
            } else {
                v.clamp(Y_RANGE.0, Y_RANGE.1)
            }
        };
        Canvas::new(size, self.pad, y(self.y_lo, -0.6), y(self.y_hi, 1.6))
    }

    /// The canvas as drawn and its top-left in event space, from the
    /// widget's full drawn rect. A finger event's `rect` is the CLIPPED one
    /// (`Area::clipped_rect`), which shrinks and moves once the curve is
    /// partly scrolled out of view; the drawn rect already carries the
    /// scroll (the turtle applies it at draw time), so it is in the same
    /// space as `fe.abs`.
    fn hit_canvas(&self, cx: &Cx) -> (Canvas, DVec2) {
        let r = self.draw_bg.area().rect(cx);
        (self.canvas(r.size), r.pos)
    }

    /// Takes `e` as the shown ease: its control points when it has some
    /// (a Penner arm takes its CSS approximation), else sampled. A precise
    /// CSS bezier ([`Easing::CubicBezier`]) keeps its precise solver; every
    /// other ease with points runs the parity one.
    fn adopt(&mut self, e: Easing) {
        if let Some(p) = e.css_points() {
            self.pts = clamp_ease_points(p);
            self.easing = match e {
                Easing::CubicBezier { .. } => {
                    Easing::css(self.pts[0], self.pts[1], self.pts[2], self.pts[3])
                }
                _ => bezier_easing(self.pts),
            };
            self.sampled = false;
        } else {
            self.easing = e;
            self.sampled = true;
            self.grab = None;
            self.hot = None;
        }
    }

    /// The control points `[x1, y1, x2, y2]` (the last ones while a sampled
    /// ease is shown).
    pub fn points(&self) -> [f64; 4] {
        self.pts
    }

    /// What the curve evaluates: the parity `Easing::Bezier` of the points,
    /// or the sampled ease.
    pub fn easing(&self) -> Easing {
        self.easing
    }

    /// Whether the curve shows a non-bezier ease (handles hidden).
    pub fn is_sampled(&self) -> bool {
        self.sampled
    }

    /// Shows the cubic bezier `[x1, y1, x2, y2]` (x clamped to 0..=1, y to
    /// -1..=2). Emits nothing.
    pub fn set_points(&mut self, cx: &mut Cx, p: [f64; 4]) {
        let p = clamp_ease_points(p);
        // Unchanged only when the parity solver is already the one drawn: a
        // precise curve from `set_easing(Easing::css(..))` switches over.
        if !self.sampled && p == self.pts && matches!(self.easing, Easing::Bezier { .. }) {
            return;
        }
        self.pts = p;
        self.easing = bezier_easing(p);
        self.sampled = false;
        self.refresh_tip();
        self.draw_bg.redraw(cx);
    }

    /// Shows `e`: an ease with control points (a bezier, linear, or a Penner
    /// arm through its CSS approximation) as its points, drawn with the
    /// parity solver except for [`Easing::CubicBezier`], which stays
    /// precise (`set_easing(cx, Easing::css(..))` shows the CSS curve exactly), anything else
    /// (elastic, bounce, steps, ...) sampled from `Easing::map` with the
    /// handles hidden. Emits nothing.
    pub fn set_easing(&mut self, cx: &mut Cx, e: Easing) {
        self.adopt(e);
        self.refresh_tip();
        self.draw_bg.redraw(cx);
    }

    /// Where a double tap puts the points back.
    pub fn set_home(&mut self, p: [f64; 4]) {
        self.home = clamp_ease_points(p);
    }

    fn set_hot(&mut self, cx: &mut Cx, hot: Option<EaseHandle>) {
        if self.hot != hot {
            self.hot = hot;
            self.refresh_tip();
            self.draw_bg.redraw(cx);
        }
    }

    /// The handle the tip describes: the dragged one, else the hovered one.
    fn tip_handle(&self) -> Option<EaseHandle> {
        self.grab.map(|(h, _)| h).or(self.hot)
    }

    fn refresh_tip(&mut self) {
        self.tip.clear();
        let (n, x, y) = match self.tip_handle() {
            Some(EaseHandle::P1) => (1, self.pts[0], self.pts[1]),
            Some(EaseHandle::P2) => (2, self.pts[2], self.pts[3]),
            None => return,
        };
        let _ = write!(self.tip, "x{n} {:.2}  y{n} {:.2}", x + 0.0, y + 0.0);
    }

    /// Moves handle `h` to `(x, y)` (clamped); true when the points changed.
    fn put(&mut self, h: EaseHandle, x: f64, y: f64) -> bool {
        let mut p = self.pts;
        match h {
            EaseHandle::P1 => {
                p[0] = x;
                p[1] = y;
            }
            EaseHandle::P2 => {
                p[2] = x;
                p[3] = y;
            }
        }
        let p = clamp_ease_points(p);
        if p == self.pts {
            return false;
        }
        self.pts = p;
        self.easing = bezier_easing(p);
        self.refresh_tip();
        true
    }

    fn preview_on(&self, cx: &Cx) -> bool {
        self.preview && self.preview_secs > 0.0 && !tween_ticker_ref(cx).reduced_motion
    }

    /// The preview dot at the current phase, in local pixels.
    fn dot_px(&self, c: &Canvas) -> DVec2 {
        let t = (self.phase / self.preview_secs.max(1e-3)).min(1.0);
        c.inside(c.to_px(t, self.easing.map(t)))
    }

    fn step_preview(&mut self, cx: &mut Cx, dt: f64) {
        let cycle = self.preview_secs.max(1e-3) + self.preview_hold.max(0.0);
        self.phase = (self.phase + dt) % cycle;
        let rect = self.draw_knobs.area().rect(cx);
        let dot = to_f32(self.dot_px(&self.canvas(rect.size)));
        self.draw_knobs.dot = dot;
        self.draw_knobs
            .set_instance_on_area(cx, id!(dot), &[dot.x, dot.y]);
    }
}

impl Widget for EaseCurve {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(dt) = self.clock.tick(cx, event) {
            // The chain runs only while the dot is on screen: a curve that
            // is no longer drawn stops here, and draw_walk restarts it.
            if self.preview_on(cx) && self.draw_knobs.area().is_valid(cx) {
                self.step_preview(cx, dt);
                self.clock.finish_frame(cx, true);
            } else {
                self.clock.stop();
            }
            return;
        }
        if !self.editable || self.sampled {
            return;
        }
        let uid = self.uid;
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                if self.grab.is_none() {
                    let (c, at) = self.hit_canvas(cx);
                    let hot = c.pick(fe.abs - at, &self.pts, self.grab_radius);
                    self.set_hot(cx, hot);
                }
                cx.set_cursor(if self.hot.is_some() {
                    MouseCursor::Grab
                } else {
                    MouseCursor::Default
                });
            }
            Hit::FingerHoverOut(_) => {
                if self.grab.is_none() {
                    self.set_hot(cx, None);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                // A double tap resets, but only a real one: the platform
                // counts any press within 0.5 s and 5 px of the last, so a
                // quick re-grab after a short drag also arrives as tap 2.
                // A reset that changes nothing emits nothing and the press
                // grabs as usual.
                if fe.tap_count == 2 && !self.last_drag && self.home != self.pts {
                    self.grab = None;
                    self.pts = self.home;
                    self.easing = bezier_easing(self.home);
                    self.refresh_tip();
                    self.draw_bg.redraw(cx);
                    cx.widget_action(uid, EaseCurveAction::Changed(self.pts));
                    cx.widget_action(uid, EaseCurveAction::Committed(self.pts));
                    return;
                }
                let (c, at) = self.hit_canvas(cx);
                let local = fe.abs - at;
                if let Some(h) = c.pick(local, &self.pts, self.grab_radius) {
                    self.grab = Some((h, local - c.handle_px(&self.pts, h)));
                    self.active = h;
                    self.moved = false;
                    self.hot = Some(h);
                    self.refresh_tip();
                    cx.set_cursor(MouseCursor::Grabbing);
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerMove(fe) => {
                let Some((h, offset)) = self.grab else {
                    return;
                };
                cx.set_cursor(MouseCursor::Grabbing);
                let (c, at) = self.hit_canvas(cx);
                let (x, y) = c.to_unit(fe.abs - at - offset);
                // Drags land on 0.01 (about a pixel at the default size); the
                // value range stops a handle so it is never dragged out of
                // reach (`drag_y` is never inverted).
                let q = |v: f64| (v * 100.0).round() / 100.0;
                let (lo, hi) = c.drag_y();
                let y = y.clamp(lo, hi);
                if self.put(h, q(x), q(y)) {
                    self.moved = true;
                    cx.widget_action(uid, EaseCurveAction::Changed(self.pts));
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerUp(fe) => {
                if self.grab.take().is_none() {
                    self.last_drag = false;
                    return;
                }
                self.last_drag = self.moved;
                if self.moved {
                    cx.widget_action(uid, EaseCurveAction::Committed(self.pts));
                }
                let hot = if fe.is_over && fe.device.has_hovers() {
                    let (c, at) = self.hit_canvas(cx);
                    c.pick(fe.abs - at, &self.pts, self.grab_radius)
                } else {
                    None
                };
                self.hot = None;
                self.set_hot(cx, hot);
                self.refresh_tip();
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(ke) => {
                let d = if ke.modifiers.shift { 0.1 } else { 0.01 };
                let (dx, dy) = match ke.key_code {
                    KeyCode::ArrowLeft => (-d, 0.0),
                    KeyCode::ArrowRight => (d, 0.0),
                    KeyCode::ArrowUp => (0.0, d),
                    KeyCode::ArrowDown => (0.0, -d),
                    _ => return,
                };
                let h = self.hot.unwrap_or(self.active);
                self.active = h;
                let (x, y) = match h {
                    EaseHandle::P1 => (self.pts[0], self.pts[1]),
                    EaseHandle::P2 => (self.pts[2], self.pts[3]),
                };
                let r = |v: f64| (v * 1000.0).round() / 1000.0;
                if self.put(h, r(x + dx), r(y + dy)) {
                    cx.widget_action(uid, EaseCurveAction::Changed(self.pts));
                    cx.widget_action(uid, EaseCurveAction::Committed(self.pts));
                    self.draw_bg.redraw(cx);
                }
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let c = self.canvas(rect.size);
        let handles = if self.editable && !self.sampled {
            1.0
        } else {
            0.0
        };
        let p1 = c.handle_px(&self.pts, EaseHandle::P1);
        let p2 = c.handle_px(&self.pts, EaseHandle::P2);
        let top = c.to_px(0.0, 1.0);
        let sq = vec4(top.x as f32, top.y as f32, c.side as f32, 0.0);
        let hot1 = (self.hot == Some(EaseHandle::P1)) as u8 as f32;
        let hot2 = (self.hot == Some(EaseHandle::P2)) as u8 as f32;
        for d in [&mut self.draw_bg, &mut self.draw_knobs] {
            d.sq = sq;
            d.p0 = to_f32(c.to_px(0.0, 0.0));
            d.p1 = to_f32(p1);
            d.p2 = to_f32(p2);
            d.p3 = to_f32(c.to_px(1.0, 1.0));
            d.hot1 = hot1;
            d.hot2 = hot2;
            d.handles_on = handles;
            d.handle_r = self.handle_radius as f32;
            d.rail_x = (top.x + c.side) as f32;
        }
        self.draw_bg.draw_abs(cx, rect);

        // The curve: EASE_CURVE_SEGMENTS capsules, each on a quad just big
        // enough for it. x steps along the cubic's parameter (X(u) at
        // uniform u) for a bezier, uniformly for a sampled ease; y is the
        // runtime solver's. A segment past the widget is clipped to it (the
        // stroke stays inside), and one wholly outside is skipped.
        self.draw_curve.thickness = self.line_width as f32;
        let m = self.line_width + 2.0;
        let inset = self.line_width * 0.5;
        let [x1, _, x2, _] = self.pts;
        let bezier = !self.sampled;
        let mut prev = c.to_px(0.0, self.easing.map(0.0));
        for i in 1..=EASE_CURVE_SEGMENTS {
            let u = i as f64 / EASE_CURVE_SEGMENTS as f64;
            let x = if bezier {
                let v = 1.0 - u;
                (3.0 * v * v * u * x1 + 3.0 * v * u * u * x2 + u * u * u).clamp(0.0, 1.0)
            } else {
                u
            };
            let next = c.to_px(x, self.easing.map(x));
            if let Some((a, b)) = c.clip(prev, next, inset) {
                let lo = dvec2(a.x.min(b.x) - m, a.y.min(b.y) - m);
                let hi = dvec2(a.x.max(b.x) + m, a.y.max(b.y) + m);
                self.draw_curve.seg_a = to_f32(a - lo);
                self.draw_curve.seg_b = to_f32(b - lo);
                self.draw_curve.draw_abs(
                    cx,
                    Rect {
                        pos: rect.pos + lo,
                        size: hi - lo,
                    },
                );
            }
            prev = next;
        }

        let preview = self.preview_on(cx);
        self.draw_knobs.dot_on = preview as u8 as f32;
        self.draw_knobs.dot = to_f32(self.dot_px(&c));
        self.draw_knobs.draw_abs(cx, rect);

        if handles > 0.5 {
            if let Some(h) = self.tip_handle() {
                let at = c.handle_px(&self.pts, h);
                let size = self.draw_text.text_style.font_size as f64;
                let w = measure(&self.draw_text, cx, &self.tip);
                let x = (at.x + self.handle_radius + 4.0).min(rect.size.x - w - 4.0);
                let y = if at.y - size * 2.0 > 0.0 {
                    at.y - size * 2.0
                } else {
                    at.y + self.handle_radius + 4.0
                };
                self.draw_text
                    .draw_abs(cx, rect.pos + dvec2(x.max(4.0), y), &self.tip);
            }
            cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        }

        if preview {
            self.clock.draw_check(cx, true);
        }
        DrawStep::done()
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        if self.sampled {
            return Some(format!("{:?}", self.easing));
        }
        let mut s = String::new();
        write_cubic_bezier(&mut s, self.pts);
        if matches!(self.easing, Easing::CubicBezier { .. }) {
            s.push_str(" precise");
        }
        Some(s)
    }
}

impl EaseCurveRef {
    /// The control points `[x1, y1, x2, y2]`.
    pub fn points(&self) -> [f64; 4] {
        self.borrow()
            .map(|inner| inner.points())
            .unwrap_or([0.0, 0.0, 1.0, 1.0])
    }

    /// Shows the cubic bezier `[x1, y1, x2, y2]`. Emits nothing.
    pub fn set_points(&self, cx: &mut Cx, p: [f64; 4]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_points(cx, p);
        }
    }

    /// What the curve evaluates (the parity solver for a bezier).
    pub fn easing(&self) -> Easing {
        self.borrow()
            .map(|inner| inner.easing())
            .unwrap_or(Easing::Linear)
    }

    /// Shows any easing: its control points when it has some, else sampled.
    pub fn set_easing(&self, cx: &mut Cx, e: Easing) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_easing(cx, e);
        }
    }

    /// The points while a handle moves.
    pub fn changed(&self, actions: &Actions) -> Option<[f64; 4]> {
        actions
            .filter_widget_actions_cast::<EaseCurveAction>(self.widget_uid())
            .find_map(|a| match a {
                EaseCurveAction::Changed(p) => Some(p),
                _ => None,
            })
    }

    /// The points a gesture settled on.
    pub fn committed(&self, actions: &Actions) -> Option<[f64; 4]> {
        actions
            .filter_widget_actions_cast::<EaseCurveAction>(self.widget_uid())
            .find_map(|a| match a {
                EaseCurveAction::Committed(p) => Some(p),
                _ => None,
            })
    }
}

// ---------------------------------------------------------------------------
// EaseEditor
// ---------------------------------------------------------------------------

/// A cubic-bezier ease editor: a preset drop-down over `CSS_PRESETS`, an
/// editable [`EaseCurve`], four fields (x1, y1, x2, y2) and a
/// `cubic-bezier(..)` readout.
#[derive(Script, Widget)]
pub struct EaseEditor {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    /// The ease the DSL sets. Adopted when it changes; the live value is
    /// [`EaseEditor::ease`].
    #[live(Ease::Bezier{cp0: 0.25, cp1: 0.1, cp2: 0.25, cp3: 1.0})]
    ease: Ease,

    #[rust]
    applied: Option<Ease>,
    /// The control points `[x1, y1, x2, y2]`.
    #[rust([0.0, 0.0, 1.0, 1.0])]
    pts: [f64; 4],
    /// Where a double tap on the curve goes back to: the last preset picked
    /// (or the last value set from code).
    #[rust]
    home: [f64; 4],
    /// The `CSS_PRESETS` row the points match.
    #[rust]
    preset: Option<usize>,
    /// A non-bezier ease on show (sampled curve, handles hidden) until a
    /// preset is picked or a number typed: the easing drawn, and the `Ease`
    /// it was set as (`None` when it was set as an [`Easing`] only).
    #[rust]
    custom: Option<(Easing, Option<Ease>)>,
    /// A field scrub is under way: Committed goes out on the finger up.
    #[rust]
    scrubbing: bool,
    #[rust]
    labels_set: bool,
    /// The children show the current value.
    #[rust]
    dressed: bool,
    /// The readout text, reused.
    #[rust]
    readout: String,
}

impl ScriptHook for EaseEditor {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if self.applied != Some(self.ease) {
            self.applied = Some(self.ease);
            self.adopt(self.ease);
            self.home = self.pts;
            self.dressed = false;
            self.view.redraw(vm.cx_mut());
        }
    }
}

impl EaseEditor {
    fn adopt(&mut self, e: Ease) {
        self.adopt_easing(Easing::from(&e), Some(e));
    }

    fn adopt_easing(&mut self, easing: Easing, e: Option<Ease>) {
        match easing.css_points() {
            Some(p) => {
                self.pts = clamp_ease_points(p);
                self.preset = preset_index(self.pts);
                self.custom = None;
            }
            None => {
                self.preset = None;
                self.custom = Some((easing, e));
            }
        }
    }

    /// The value: `Ease::Bezier { cp0: x1, cp1: y1, cp2: x2, cp3: y2 }`, or
    /// the non-bezier ease set with [`EaseEditor::set_ease`] while it is
    /// shown. While an [`Easing`] with no `Ease` form is shown
    /// ([`EaseEditor::set_easing`]), it is the bezier of the last points.
    pub fn ease(&self) -> Ease {
        match self.custom {
            Some((_, Some(e))) => e,
            _ => bezier_ease(self.pts),
        }
    }

    /// What the curve shows: the parity solver of the points, or the
    /// non-bezier ease on show.
    pub fn easing(&self) -> Easing {
        match self.custom {
            Some((e, _)) => e,
            None => bezier_easing(self.pts),
        }
    }

    /// The control points `[x1, y1, x2, y2]`.
    pub fn points(&self) -> [f64; 4] {
        self.pts
    }

    /// The `CSS_PRESETS` name the points match, if any.
    pub fn preset(&self) -> Option<&'static str> {
        self.preset.map(|i| CSS_PRESETS[i].0)
    }

    /// Sets the points (x clamped to 0..=1, y to -1..=2). Emits nothing.
    pub fn set_points(&mut self, cx: &mut Cx, p: [f64; 4]) {
        self.pts = clamp_ease_points(p);
        self.home = self.pts;
        self.preset = preset_index(self.pts);
        self.custom = None;
        self.dress(cx);
    }

    /// Sets the value. A Penner arm (OutCubic, InOutBack, ...) shows as its
    /// CSS approximation; an ease without one (elastic, bounce, ...) is shown
    /// sampled, with the handles hidden, until a preset is picked. Emits
    /// nothing.
    pub fn set_ease(&mut self, cx: &mut Cx, e: Ease) {
        self.adopt(e);
        self.home = self.pts;
        self.dress(cx);
    }

    /// Shows any [`Easing`] (a GSAP ease a host parsed, a precise CSS
    /// curve): as its points when it has some (drawn and edited with the
    /// parity solver from then on), else sampled with the handles hidden,
    /// the readout naming it, until a preset is picked or a number typed.
    /// Emits nothing.
    pub fn set_easing(&mut self, cx: &mut Cx, e: Easing) {
        self.adopt_easing(e, None);
        self.home = self.pts;
        self.dress(cx);
    }

    /// Selects a `CSS_PRESETS` row by name (`"ease_out_back"`). False when
    /// no row has that name. Emits nothing.
    pub fn set_preset(&mut self, cx: &mut Cx, name: &str) -> bool {
        match CSS_PRESETS.iter().position(|(n, _)| *n == name) {
            Some(i) => {
                self.set_points(cx, CSS_PRESETS[i].1);
                true
            }
            None => false,
        }
    }

    /// Pushes the value into the children (their setters emit nothing).
    fn dress(&mut self, cx: &mut Cx) {
        self.dressed = true;
        let preset = self.view.widget(cx, ids!(preset)).as_drop_down();
        if !self.labels_set {
            self.labels_set = true;
            let mut labels: Vec<String> = CSS_PRESETS.iter().map(|(n, _)| n.to_string()).collect();
            labels.push("custom".to_string());
            preset.set_labels(cx, labels);
        }
        preset.set_selected_item(cx, self.preset.unwrap_or(CUSTOM_ROW));
        if let Some(mut curve) = self.view.widget(cx, ids!(curve)).borrow_mut::<EaseCurve>() {
            match self.custom {
                Some((e, _)) => curve.set_easing(cx, e),
                None => curve.set_points(cx, self.pts),
            }
            curve.set_home(self.home);
        }
        for (i, id) in [live_id!(x1), live_id!(y1), live_id!(x2), live_id!(y2)]
            .into_iter()
            .enumerate()
        {
            self.view
                .widget(cx, &[id])
                .as_value_input()
                .set_value(cx, self.pts[i]);
        }
        self.readout.clear();
        let _ = match self.custom {
            Some((_, Some(e))) => write!(self.readout, "{:?} (no cubic-bezier)", e),
            Some((e, None)) => write!(self.readout, "{:?} (no cubic-bezier)", e),
            None => {
                write_cubic_bezier(&mut self.readout, self.pts);
                Ok(())
            }
        };
        self.view
            .widget(cx, ids!(readout))
            .as_label()
            .set_text(cx, &self.readout);
    }

    fn emit(&self, cx: &mut Cx, changed: bool, committed: bool) {
        let uid = self.widget_uid();
        let e = self.ease();
        if changed {
            cx.widget_action(uid, EaseEditorAction::Changed(e));
        }
        if committed {
            cx.widget_action(uid, EaseEditorAction::Committed(e));
        }
    }

    /// Takes new points from a child control.
    fn take(&mut self, cx: &mut Cx, p: [f64; 4]) {
        self.pts = clamp_ease_points(p);
        self.preset = preset_index(self.pts);
        self.custom = None;
        self.dress(cx);
    }
}

impl Widget for EaseEditor {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.dressed {
            self.dress(cx);
        }
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if self.scrubbing && pointer_released(event) {
            self.scrubbing = false;
            self.emit(cx, false, true);
        }
        if actions.is_empty() {
            return;
        }
        let preset = self.view.widget(cx, ids!(preset)).as_drop_down();
        if let Some(i) = preset.selected(&actions) {
            if i < CUSTOM_ROW {
                self.pts = CSS_PRESETS[i].1;
                self.home = self.pts;
                self.preset = Some(i);
                self.custom = None;
                self.dress(cx);
                self.emit(cx, true, true);
            } else {
                // "custom" names the current points; it has none of its own.
                preset.set_selected_item(cx, self.preset.unwrap_or(CUSTOM_ROW));
            }
        }
        let curve_uid = self.view.widget(cx, ids!(curve)).widget_uid();
        let fields = [
            self.view.widget(cx, ids!(x1)).widget_uid(),
            self.view.widget(cx, ids!(y1)).widget_uid(),
            self.view.widget(cx, ids!(x2)).widget_uid(),
            self.view.widget(cx, ids!(y2)).widget_uid(),
        ];
        for action in actions.iter() {
            let Some(wa) = action.as_widget_action() else {
                continue;
            };
            if wa.widget_uid == curve_uid {
                match wa.cast::<EaseCurveAction>() {
                    EaseCurveAction::Changed(p) => {
                        self.take(cx, p);
                        self.emit(cx, true, false);
                    }
                    EaseCurveAction::Committed(p) => {
                        self.take(cx, p);
                        self.emit(cx, false, true);
                    }
                    EaseCurveAction::None => {}
                }
                continue;
            }
            let Some(k) = fields.iter().position(|uid| *uid == wa.widget_uid) else {
                continue;
            };
            if let ValueInputAction::Changed(v) = wa.cast::<ValueInputAction>() {
                let mut p = self.pts;
                p[k] = v;
                // A field commits whatever it holds, also on a plain Enter
                // with nothing typed: the same number is no edit (the
                // preset, and a host's timeline, stay as they are).
                if self.custom.is_none() && clamp_ease_points(p) == self.pts {
                    continue;
                }
                self.take(cx, p);
                // A scrub reports every pixel; it commits on the finger up.
                let scrub = pointer_moved(event);
                self.scrubbing |= scrub;
                self.emit(cx, true, !scrub);
            }
        }
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.readout.clone())
    }
}

impl EaseEditorRef {
    /// The value (`Ease::Bezier`, or the non-bezier ease on show).
    pub fn ease(&self) -> Ease {
        self.borrow()
            .map(|inner| inner.ease())
            .unwrap_or(bezier_ease([0.0, 0.0, 1.0, 1.0]))
    }

    /// What the curve shows (the parity solver of the points, or the
    /// non-bezier ease on show).
    pub fn easing(&self) -> Easing {
        self.borrow()
            .map(|inner| inner.easing())
            .unwrap_or(Easing::Linear)
    }

    /// The control points `[x1, y1, x2, y2]`.
    pub fn points(&self) -> [f64; 4] {
        self.borrow()
            .map(|inner| inner.points())
            .unwrap_or([0.0, 0.0, 1.0, 1.0])
    }

    /// The `CSS_PRESETS` name the points match, if any.
    pub fn preset(&self) -> Option<&'static str> {
        self.borrow().and_then(|inner| inner.preset())
    }

    /// Sets the points. Emits nothing.
    pub fn set_points(&self, cx: &mut Cx, p: [f64; 4]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_points(cx, p);
        }
    }

    /// Sets the value. Emits nothing.
    pub fn set_ease(&self, cx: &mut Cx, e: Ease) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_ease(cx, e);
        }
    }

    /// Shows any easing (sampled when it has no control points). Emits
    /// nothing.
    pub fn set_easing(&self, cx: &mut Cx, e: Easing) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_easing(cx, e);
        }
    }

    /// Selects a `CSS_PRESETS` row by name. False when there is none.
    /// Emits nothing.
    pub fn set_preset(&self, cx: &mut Cx, name: &str) -> bool {
        self.borrow_mut()
            .map(|mut inner| inner.set_preset(cx, name))
            .unwrap_or(false)
    }

    /// The value while it moves (drag step, field scrub, and the first half
    /// of every commit).
    pub fn changed(&self, actions: &Actions) -> Option<Ease> {
        actions
            .filter_widget_actions_cast::<EaseEditorAction>(self.widget_uid())
            .find_map(|a| match a {
                EaseEditorAction::Changed(e) => Some(e),
                _ => None,
            })
    }

    /// The value a gesture settled on.
    pub fn committed(&self, actions: &Actions) -> Option<Ease> {
        actions
            .filter_widget_actions_cast::<EaseEditorAction>(self.widget_uid())
            .find_map(|a| match a {
                EaseEditorAction::Committed(e) => Some(e),
                _ => None,
            })
    }
}
