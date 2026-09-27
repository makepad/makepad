//! CurveEditor: a curve of anchors over a unit box, drawn and edited.
//!
//! The port of the material bench's curve editor (its revolve profile, tooth,
//! wing and groove editors are one editor with different labels). A curve is
//! a list of anchors `[x, y, kind]`, x and y in 0..=1, ordered by x, joined
//! by cubic Beziers whose tangents are always derived from the neighbours
//! (the bench's `fixTangents`). The kind says how:
//!
//! * 0 point: no tangents, straight lines to both neighbours (a diamond).
//! * 1 smooth: mirrored tangents, flat at a local extremum (a circle).
//! * 2 corner: each side clamped on its own (a square).
//! * 3 horizontal: smooth, held level (a wide rounded rect).
//!
//! [`curve_polyline`] and [`curve_resample`] are the bench's `polyline` and
//! `resample`, step for step and in the same order of operations as the knob
//! bake's port of them, so the same anchors give the same numbers: what the
//! editor draws is what a bake reads.
//!
//! Input: a press takes the nearest anchor within 0.05 (in curve units), or
//! on empty canvas inserts a smooth anchor there, between its x-neighbours,
//! and selects it (where they are 0.02 apart or less there is no room, and
//! it inserts nothing); Shift or Ctrl (Cmd) toggles an anchor in the
//! selection. A drag moves the selection: the end anchors keep x 0 and 1, y
//! stays in 0..=1, no moved anchor passes one that stays, and each keeps
//! 0.01 inside its x-neighbours, or halfway between them where they are too
//! close for that. A single inner anchor dragged out of the box is drawn
//! dashed in the error colour and goes on release. The toolbar's kind row
//! lights the kind every selected anchor shares and sets the selection's
//! kind (Horizontal on several also levels them at the height of the last
//! one picked, lit or not); Delete takes its inner anchors. A press locks
//! the pointer to the canvas until release. Everything the canvas draws is
//! clipped to it: the end anchors' tangents reach past the box.
//!
//! The tangent handles are drawn but not dragged, this round: an anchor is
//! `[x, y, kind]` and carries no tangents of its own, so a handle drag would
//! have nowhere to keep what it set. The bench's dragged handles (its extra
//! per-anchor fields) wait for a later one. Until then a press on a drawn
//! handle's end (within 0.045) selects that handle's anchor and does
//! nothing else: no drag, and never an insert. It is picked as the bench
//! picks: a selected anchor's own handles first, where the press is nearer
//! them than the anchor, then the anchor, then any handle.
//!
//! [`CurveEditorAction::Changed`] goes out on every edit (an insert, each
//! drag step that moves something, a kind, a delete) and
//! [`CurveEditorAction::Committed`] when a gesture ends: the release after an
//! edit, a toolbar press that changed something.
use crate::{
    animator::Animate,
    family_api::measure,
    button::ButtonWidgetExt,
    makepad_derive_widget::*,
    makepad_draw::*,
    radio_button::RadioButtonWidgetExt,
    view::View,
    widget::*,
};
use std::fmt::Write;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawCurveCanvas::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    set_type_default() do #(DrawCurveFill::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    set_type_default() do #(DrawCurveSegment::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn(){
            let p = self.pos * self.rect_size
            let pa = p - self.seg_a
            let ba = self.seg_b - self.seg_a
            let len = max(length(ba), 0.0001)
            let h = clamp(dot(pa, ba) / (len * len), 0.0, 1.0)
            let d = length(pa - ba * h)
            let half = self.thickness * 0.5
            let mut aa = 1.0 - smoothstep(half - 0.6, half + 0.6, d)
            // Dashed from seg_a: `dash` on, `gap` off.
            if self.dash > 0.0 {
                let period = self.dash + max(self.gap, 0.0)
                let along = fract(h * len / period) * period
                aa = aa * (1.0 - step(self.dash, along))
            }
            // A faded stroke is mixed into the ground rather than drawn
            // translucent: the capsules overlap at every joint, and a
            // translucent run would show a bead at each one.
            let ink = mix(self.ground, self.color, self.fade)
            return vec4(ink.rgb * ink.a * aa, ink.a * aa)
        }
    }

    set_type_default() do #(DrawCurveKnob::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.CurveEditorBase = #(CurveEditor::register_widget(vm))

    /** A curve of anchors over a unit box, the canvas on top and a toolbar
     * under it: drag, insert and delete anchors, and set each one's kind. */
    mod.widgets.CurveEditor = set_type_default() do mod.widgets.CurveEditorBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_1
        /** the curve until a host sets another: [x, y, kind] rows, x and y 0..1, kind 0 point, 1 smooth, 2 corner, 3 horizontal */
        anchors: [[0, 1, 1], [1, 0, 1]]
        /** small caption at the canvas's bottom-left, "" for none */
        left_label: ""
        /** small caption at the canvas's bottom-right, "" for none */
        right_label: ""
        /** a dashed reference line at this height, -1 for none -1..1 step 0.01 */
        guide: -1.0
        /** the reference line's caption */
        guide_label: ""
        /** draw the curve followed by its mirror, faint, across a strip at the top: the whole tooth a half-tooth curve makes */
        mirror: false

        draw_canvas +: {
            color: theme.color_inset
            border_color: uniform(theme.color_bevel)
            grid_color: uniform(theme.color_outline_variant)
            radius: uniform(theme.corner_radius)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let sdf = Sdf2d.viewport(p)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, 1.0)
                let mut c = sdf.result
                // Five lines each way over the unit box, as the bench draws
                // them: a quarter apart, each only as long as the box.
                let l = self.unit.x
                let t = self.unit.y
                let w = max(self.unit.z, 1.0)
                let h = max(self.unit.w, 1.0)
                let inx = step(l - 0.5, p.x) * step(p.x, l + w + 0.5)
                let iny = step(t - 0.5, p.y) * step(p.y, t + h + 0.5)
                let gx = abs(fract((p.x - l) / w * 4.0 + 0.5) - 0.5) * w * 0.25
                let gy = abs(fract((p.y - t) / h * 4.0 + 0.5) - 0.5) * h * 0.25
                let grid = max(1.0 - clamp(gx, 0.0, 1.0), 1.0 - clamp(gy, 0.0, 1.0)) * inx * iny
                c = mix(c, vec4(self.grid_color.rgb, 1.0), grid * self.grid_color.a)
                return c
            }
        }
        draw_fill +: {
            color: theme.color_primary
            opacity: uniform(0.13)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let span = max(self.fill_b.x - self.fill_a.x, 0.0001)
                let u = clamp((p.x - self.fill_a.x) / span, 0.0, 1.0)
                let top = mix(self.fill_a.y, self.fill_b.y, u)
                let cov = clamp(p.y - top + 0.5, 0.0, 1.0)
                let a = self.color.a * self.opacity * cov
                return vec4(self.color.rgb * a, a)
            }
        }
        draw_curve +: {
            color: theme.color_primary
            thickness: 2.0
        }
        draw_guide +: {
            color: theme.color_primary
            thickness: 1.0
            dash: 4.0
            gap: 3.0
        }
        draw_mirror +: {
            color: theme.color_primary
            thickness: 1.5
            fade: 0.4
        }
        draw_arm +: {
            color: theme.color_text_meta
            thickness: 1.0
        }
        draw_knob +: {
            ink_color: uniform(theme.color_primary)
            face_color: uniform(theme.color_inset)
            arm_color: uniform(theme.color_text_meta)
            doomed_color: uniform(theme.color_error)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let sdf = Sdf2d.viewport(p)
                // A tangent handle's end: a small dot, square on a corner.
                if self.handle > 0.5 {
                    if abs(self.kind - 2.0) < 0.5 {
                        sdf.rect(c.x - 3.0, c.y - 3.0, 6.0, 6.0)
                    } else {
                        sdf.circle(c.x, c.y, 3.0)
                    }
                    sdf.fill(self.arm_color)
                    return sdf.result
                }
                // The selection ring, twice as heavy on the primary.
                if self.ring > 0.5 {
                    sdf.circle(c.x, c.y, 10.0)
                    sdf.stroke(self.ink_color, 0.5 * self.ring)
                }
                // The kind's glyph, a size up while it is doomed.
                let big = self.doomed
                if self.kind < 0.5 {
                    let r = mix(5.5, 7.0, big) * 0.70710678
                    sdf.rotate(PI * 0.25, c.x, c.y)
                    sdf.rect(c.x - r, c.y - r, 2.0 * r, 2.0 * r)
                } else {
                    if self.kind < 1.5 {
                        sdf.circle(c.x, c.y, mix(5.0, 7.0, big))
                    } else {
                        if self.kind < 2.5 {
                            let r = mix(4.5, 6.0, big)
                            sdf.rect(c.x - r, c.y - r, 2.0 * r, 2.0 * r)
                        } else {
                            let r = mix(3.5, 4.5, big)
                            sdf.box(c.x - 2.0 * r, c.y - r, 4.0 * r, 2.0 * r, r * 0.35)
                        }
                    }
                }
                sdf.fill_keep(self.face_color)
                // Dashed round the glyph while a release would delete it.
                let a = atan2(p.y - c.y, p.x - c.x)
                let dash = mix(1.0, step(0.5, fract(a * 1.1141)), big)
                let ink = mix(self.ink_color, self.doomed_color, big)
                sdf.stroke(vec4(ink.rgb, ink.a * dash), 1.0)
                return sdf.result
            }
        }
        draw_text +: {
            color: theme.color_text_meta
            text_style: theme.font_label_s
        }

        canvas := View{
            width: Fill
            height: 220
        }
        // The kinds are a radio row: the editor lights the rung the whole
        // selection shares, or none. Each rung speaks on every press, lit
        // or not (`independent`), so Horizontal pressed again still levels.
        tools := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: theme.space_1
            align: Align{y: 0.5}
            smooth := RadioButtonTab{text: "Smooth" independent: true}
            corner := RadioButtonTab{text: "Corner" independent: true}
            horizontal := RadioButtonTab{text: "Horizontal" independent: true}
            point := RadioButtonTab{text: "Point" independent: true}
            Filler{}
            delete := Button{text: "Delete"}
        }
    }
}

/// One anchor: `[x, y, kind]`, x and y in 0..=1, kind 0 point (straight to
/// its neighbours), 1 smooth, 2 corner, 3 horizontal.
pub type CurveAnchor = [f64; 3];

const POINT: f64 = 0.0;
const SMOOTH: f64 = 1.0;
const CORNER: f64 = 2.0;
const HORIZONTAL: f64 = 3.0;

/// The curve a new editor starts with: straight down across the box.
const DEFAULT_LINE: [CurveAnchor; 2] = [[0.0, 1.0, SMOOTH], [1.0, 0.0, SMOOTH]];

/// Samples of each cubic segment in the polyline (the bench's: at 24 a
/// smooth roof showed its creases under zoom).
const SEGMENT_STEPS: usize = 96;

/// How near a press must be to take an anchor, in curve units.
const PICK: f64 = 0.05;
/// How near a press must be to a tangent handle's end, in curve units.
const PICK_HANDLE: f64 = 0.045;
/// How far inside its x-neighbours a moved anchor stays.
const GAP: f64 = 0.01;
/// How far past the box a lone inner anchor goes before a release deletes
/// it, in x and in y.
const OUT_X: f64 = 0.04;
const OUT_Y: f64 = 0.06;

/// The canvas padding round the unit box, in pixels (the bench's `PAD`);
/// the captions sit in the bottom one.
const PAD: f64 = 16.0;
/// Polyline samples closer than this on screen are drawn as one step.
const STEP_PX: f64 = 2.0;
/// Half the side of an anchor's quad: the ring's radius, its stroke and
/// room for the edge to fade.
const KNOB_HALF: f64 = 14.0;
/// Half the side of a tangent handle's dot quad.
const DOT_HALF: f64 = 5.0;

/// The toolbar's kind rungs, in the row's order, and the kind each sets.
const KIND_RUNGS: [(LiveId, f64); 4] = [
    (live_id!(smooth), SMOOTH),
    (live_id!(corner), CORNER),
    (live_id!(horizontal), HORIZONTAL),
    (live_id!(point), POINT),
];

/// The canvas: its ground, its border and the unit grid. `unit` is the unit
/// box in the quad's local pixels: left, top, width, height.
#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawCurveCanvas {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    unit: Vec4f,
}

/// One column of the area under the curve: the quad spans the step's x and
/// reaches down to the box's floor, `fill_a`-`fill_b` is the curve across
/// its top, in the quad's local pixels.
#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawCurveFill {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    fill_a: Vec2f,
    #[live]
    fill_b: Vec2f,
    #[live]
    color: Vec4f,
}

/// One anti-aliased stroke segment (a capsule distance), optionally dashed,
/// and optionally faded into `ground` by `fade` (1 is the full colour).
#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawCurveSegment {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    seg_a: Vec2f,
    #[live]
    seg_b: Vec2f,
    #[live]
    color: Vec4f,
    #[live]
    ground: Vec4f,
    #[live(1.0)]
    fade: f32,
    #[live(1.0)]
    thickness: f32,
    #[live]
    dash: f32,
    #[live]
    gap: f32,
}

/// An anchor's glyph and selection ring, or (`handle` 1) a tangent handle's
/// dot, centred on the quad.
#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawCurveKnob {
    #[deref]
    draw_super: DrawQuad,
    /// The anchor's kind, 0..=3.
    #[live]
    kind: f32,
    #[live]
    handle: f32,
    /// 0 unselected, 1 selected, 2 the primary.
    #[live]
    ring: f32,
    /// 1 while a release would delete the anchor.
    #[live]
    doomed: f32,
}

/// What a [`CurveEditor`] reports, always the whole curve.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum CurveEditorAction {
    /// The curve moved: an insert, a drag step, a kind, a delete.
    Changed(Vec<CurveAnchor>),
    /// A gesture ended on an edit: the release after an insert, a drag or a
    /// drag-out delete, or a toolbar press that changed something. Every
    /// Committed follows a Changed of the same curve.
    Committed(Vec<CurveAnchor>),
    #[default]
    None,
}

// ---------------------------------------------------------------------------
// The curve: the bench's fixTangents, polyline and resample
// ---------------------------------------------------------------------------

/// An anchor with the tangents `fixTangents` gives it, as offsets from it.
#[derive(Clone, Copy, Debug, Default)]
struct Pt {
    x: f64,
    y: f64,
    ty: u8,
    ix: f64,
    iy: f64,
    ox: f64,
    oy: f64,
}

fn smooth_type(p: &Pt) -> bool {
    p.ty == 1 || p.ty == 3
}

/// The bench's `fixTangents` for anchors that carry no tangents: automatic
/// tangents, flat at a local extremum (Fritsch-Carlson), a third of the way
/// to each neighbour at most, and the pair rule so a segment's handles never
/// cross in x.
fn fix_tangents(p: &mut [Pt]) {
    let n = p.len();
    for i in 0..n {
        if p[i].ty == 0 {
            p[i].ix = 0.0;
            p[i].iy = 0.0;
            p[i].ox = 0.0;
            p[i].oy = 0.0;
            continue;
        }
        let a2 = p[i.saturating_sub(1)];
        let b2 = p[(i + 1).min(n - 1)];
        let mut ty = (b2.y - a2.y) / 6.0;
        if i > 0 && i < n - 1 && (p[i].y - a2.y) * (b2.y - p[i].y) <= 0.0 {
            ty = 0.0;
        }
        if p[i].ty == 3 {
            ty = 0.0;
        }
        p[i].ox = (b2.x - a2.x) / 6.0;
        p[i].oy = ty;
        p[i].ix = -p[i].ox;
        p[i].iy = -ty;
        let span = 1.0 / 3.0;
        let back = if i > 0 { (p[i].x - p[i - 1].x) * span } else { 1.0 };
        let fwd = if i < n - 1 { (p[i + 1].x - p[i].x) * span } else { 1.0 };
        if smooth_type(&p[i]) {
            if p[i].ty == 3 {
                p[i].oy = 0.0;
            }
            if p[i].ox < 0.0 {
                p[i].ox = 0.0;
                p[i].oy = 0.0;
            }
            let lim = back.min(fwd);
            if p[i].ox > lim {
                let s2 = lim / p[i].ox;
                p[i].ox *= s2;
                p[i].oy *= s2;
            }
            p[i].ix = -p[i].ox;
            p[i].iy = -p[i].oy;
        } else {
            p[i].ox = p[i].ox.min(fwd).max(0.0);
            p[i].ix = p[i].ix.max(-back).min(0.0);
        }
    }
    for i in 0..n.saturating_sub(1) {
        let l = p[i + 1].x - p[i].x;
        let c1 = p[i].ox;
        let c2 = -p[i + 1].ix;
        if c1 + c2 > l && c1 + c2 > 0.0 {
            let f = l / (c1 + c2);
            p[i].ox *= f;
            p[i].oy *= f;
            if smooth_type(&p[i]) {
                p[i].ix = -p[i].ox;
                p[i].iy = -p[i].oy;
            }
            p[i + 1].ix *= f;
            p[i + 1].iy *= f;
            if smooth_type(&p[i + 1]) {
                p[i + 1].ox = -p[i + 1].ix;
                p[i + 1].oy = -p[i + 1].iy;
            }
        }
    }
}

/// The anchors with their derived tangents.
fn fixed_tangents(anchors: &[CurveAnchor]) -> Vec<Pt> {
    let mut p: Vec<Pt> = anchors
        .iter()
        .map(|a| Pt { x: a[0], y: a[1], ty: a[2] as u8, ..Pt::default() })
        .collect();
    fix_tangents(&mut p);
    p
}

/// The polyline through anchors whose tangents are fixed.
fn polyline_of(p: &[Pt]) -> Vec<[f64; 2]> {
    let mut out = Vec::with_capacity(p.len() * (SEGMENT_STEPS + 1));
    for i in 0..p.len().saturating_sub(1) {
        let (p0, p1) = (p[i], p[i + 1]);
        let (c1x, c1y) = (p0.x + p0.ox, p0.y + p0.oy);
        let (c2x, c2y) = (p1.x + p1.ix, p1.y + p1.iy);
        for k in 0..=SEGMENT_STEPS {
            let t = k as f64 / SEGMENT_STEPS as f64;
            let u = 1.0 - t;
            out.push([
                u * u * u * p0.x + 3.0 * u * u * t * c1x + 3.0 * u * t * t * c2x + t * t * t * p1.x,
                u * u * u * p0.y + 3.0 * u * u * t * c1y + 3.0 * u * t * t * c2y + t * t * t * p1.y,
            ]);
        }
    }
    out
}

/// The curve as the bench's polyline: tangents by `fixTangents`, then each
/// segment's cubic Bezier sampled 97 times (96 steps, both ends included,
/// so a segment's last point repeats the next one's first). Fewer than two
/// anchors make no segment and an empty polyline.
pub fn curve_polyline(anchors: &[CurveAnchor]) -> Vec<[f64; 2]> {
    polyline_of(&fixed_tangents(anchors))
}

/// The curve's y at `n` evenly spaced x taps over 0..=1 (the bench's
/// `resample`), read off the polyline by linear interpolation; a tap left
/// of the first anchor takes its y, right of the last the last one's. The
/// polyline's x never runs backwards (the pair rule sees to that), so each
/// tap's search resumes at the segment the last one found, which is the
/// segment the bench's search from the start finds. One tap is at x 0; one
/// anchor is a level line at its height; none reads 0.
pub fn curve_resample(anchors: &[CurveAnchor], n: usize) -> Vec<f64> {
    let pts = curve_polyline(anchors);
    let Some(last) = pts.last().copied() else {
        return vec![anchors.first().map_or(0.0, |a| a[1]); n];
    };
    let mut k0 = 0;
    (0..n)
        .map(|i| {
            let r = if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 };
            let mut y = last[1];
            if r <= pts[0][0] {
                y = pts[0][1];
            } else {
                for k in k0..pts.len() - 1 {
                    if r >= pts[k][0] && r <= pts[k + 1][0] {
                        let t = (r - pts[k][0]) / (pts[k + 1][0] - pts[k][0]).max(1e-6);
                        y = pts[k][1] + t * (pts[k + 1][1] - pts[k][1]);
                        k0 = k;
                        break;
                    }
                }
            }
            y
        })
        .collect()
}

/// `anchors` made fit to edit: finite, x and y in 0..=1, kinds whole and in
/// 0..=3, ordered by x (a stable sort). Fewer than two is the default line.
fn tidy_anchors(anchors: &[CurveAnchor]) -> Vec<CurveAnchor> {
    let mut out: Vec<CurveAnchor> = anchors
        .iter()
        .filter(|a| a.iter().all(|v| v.is_finite()))
        .map(|a| [a[0].clamp(0.0, 1.0), a[1].clamp(0.0, 1.0), a[2].round().clamp(POINT, HORIZONTAL)])
        .collect();
    out.sort_by(|a, b| a[0].total_cmp(&b[0]));
    if out.len() < 2 {
        return DEFAULT_LINE.to_vec();
    }
    out
}

/// The `anchors` array an apply brings, looked up the way the derive looks
/// up every field (an eval's own keys only, anything else through the
/// prototypes); `None` when it brings none.
fn dsl_anchors(vm: &mut ScriptVm, apply: &Apply, value: ScriptValue) -> Option<Vec<ScriptValue>> {
    let heap = &mut vm.bx.heap;
    let arr = heap.value_for_apply(value, id!(anchors).into(), apply)?.as_array()?;
    Some((0..heap.array_len(arr)).map(|i| heap.array_index_unchecked(arr, i)).collect())
}

/// The DSL's `[x, y, kind]` arrays as anchors. An entry that is not an array
/// of two or three numbers is skipped; one without a kind is smooth.
fn read_anchors(vm: &ScriptVm, values: &[ScriptValue]) -> Vec<CurveAnchor> {
    let heap = &vm.bx.heap;
    values
        .iter()
        .filter_map(|v| {
            let arr = v.as_array()?;
            let len = heap.array_len(arr);
            if !(2..=3).contains(&len) {
                return None;
            }
            let at = |k: usize| heap.array_index_unchecked(arr, k).as_number();
            Some([at(0)?, at(1)?, if len == 3 { at(2)? } else { SMOOTH }])
        })
        .collect()
}

fn kind_name(kind: f64) -> &'static str {
    match kind as u8 {
        0 => "point",
        1 => "smooth",
        2 => "corner",
        _ => "horizontal",
    }
}

// ---------------------------------------------------------------------------
// The editing: the bench's makeEditor handlers, without the drawing
// ---------------------------------------------------------------------------

/// A drag under way: the anchor pressed, where the press was and where every
/// anchor stood then (a drag moves by the pointer's travel from the press).
#[derive(Clone, Debug)]
struct Drag {
    i: usize,
    start: [f64; 2],
    orig: Vec<[f64; 2]>,
    /// The gesture changed the curve (its press inserted, or it moved).
    edited: bool,
}

/// The curve, the selection and the gesture under way.
#[derive(Clone, Debug)]
struct CurveModel {
    anchors: Vec<CurveAnchor>,
    /// The selected anchors in the order they were picked; the last is the
    /// primary, the one Horizontal levels the others to.
    sels: Vec<usize>,
    drag: Option<Drag>,
    /// The dragged anchor is out of the box: a release deletes it.
    doomed: bool,
}

impl Default for CurveModel {
    fn default() -> Self {
        Self {
            anchors: DEFAULT_LINE.to_vec(),
            sels: Vec::new(),
            drag: None,
            doomed: false,
        }
    }
}

/// What a press lands on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pick {
    Anchor(usize),
    /// The end of one of this anchor's drawn tangent handles.
    Handle(usize),
}

/// `x` held `GAP` inside `lo..hi`, or halfway between them when they are
/// too close for that: never past either, which the bench's `keep` was when
/// its neighbours stood closer than `GAP`.
fn hold(x: f64, lo: f64, hi: f64) -> f64 {
    if hi - lo >= 2.0 * GAP {
        x.min(hi - GAP).max(lo + GAP)
    } else {
        (lo + hi) * 0.5
    }
}

/// The bench's `keep`: anchor `k` held inside its x-neighbours. The ends
/// have one neighbour each and are not moved.
fn keep(p: &mut [CurveAnchor], k: usize) {
    if k > 0 && k + 1 < p.len() {
        p[k][0] = hold(p[k][0], p[k - 1][0], p[k + 1][0]);
    }
}

impl CurveModel {
    fn set_anchors(&mut self, anchors: &[CurveAnchor]) {
        self.anchors.clear();
        self.anchors.extend_from_slice(anchors);
        self.sels.clear();
        self.drag = None;
        self.doomed = false;
    }

    fn is_sel(&self, i: usize) -> bool {
        self.sels.contains(&i)
    }

    fn primary(&self) -> Option<usize> {
        self.sels.last().copied()
    }

    /// Neither end: only these can be deleted.
    fn is_inner(&self, i: usize) -> bool {
        i > 0 && i + 1 < self.anchors.len()
    }

    fn can_delete(&self) -> bool {
        self.sels.iter().any(|&k| self.is_inner(k))
    }

    /// The kind every selected anchor has, the rung the toolbar lights;
    /// `None` when they differ or nothing is selected.
    fn shared_kind(&self) -> Option<f64> {
        let kind = self.anchors[*self.sels.first()?][2];
        self.sels.iter().all(|&k| self.anchors[k][2] == kind).then_some(kind)
    }

    /// The anchor a release would delete.
    fn doomed_anchor(&self) -> Option<usize> {
        self.drag.as_ref().filter(|_| self.doomed).map(|d| d.i)
    }

    /// The drawn tangent handle end nearest `n` within `PICK_HANDLE`, only
    /// the selected anchors' when `only_selected`: (its anchor, how near).
    /// Point anchors draw no handles and have none to take.
    fn nearest_handle(&self, n: [f64; 2], only_selected: bool) -> Option<(usize, f64)> {
        let mut best = None;
        let mut bd = PICK_HANDLE;
        for (i, t) in fixed_tangents(&self.anchors).iter().enumerate() {
            if t.ty == POINT as u8 || (only_selected && !self.is_sel(i)) {
                continue;
            }
            for (dx, dy) in [(t.ix, t.iy), (t.ox, t.oy)] {
                let d = (t.x + dx - n[0]).hypot(t.y + dy - n[1]);
                if d < bd {
                    bd = d;
                    best = Some((i, d));
                }
            }
        }
        best
    }

    /// What a press at `n` takes, the bench's `pick`: the anchor nearest
    /// within `PICK`, in curve units (the box is not square on screen, so
    /// neither is the reach; the bench's too), unless a selected anchor's
    /// own handle is nearer still; a handle of any anchor only when no
    /// anchor is in reach.
    fn pick(&self, n: [f64; 2]) -> Option<Pick> {
        let mut best = None;
        let mut bd = PICK;
        for (i, a) in self.anchors.iter().enumerate() {
            let d = (a[0] - n[0]).hypot(a[1] - n[1]);
            if d < bd {
                bd = d;
                best = Some(i);
            }
        }
        if let Some((i, d)) = self.nearest_handle(n, true) {
            if best.is_none() || d < bd {
                return Some(Pick::Handle(i));
            }
        }
        best.map(Pick::Anchor)
            .or_else(|| self.nearest_handle(n, false).map(|(i, _)| Pick::Handle(i)))
    }

    /// A press at `n`, the bench's `pointerdown`. True when it inserted an
    /// anchor.
    fn press(&mut self, n: [f64; 2], multi: bool) -> bool {
        self.doomed = false;
        self.drag = None;
        let mut hit = match self.pick(n) {
            // A handle selects its anchor, as the bench's does before its
            // drag; with no handle drag yet, that is all it does.
            Some(Pick::Handle(i)) => {
                if !self.is_sel(i) {
                    self.sels.clear();
                    self.sels.push(i);
                }
                return false;
            }
            Some(Pick::Anchor(i)) => Some(i),
            None => None,
        };
        let mut inserted = false;
        if hit.is_none() && !multi {
            // Empty canvas: a smooth anchor where the press is, between its
            // x-neighbours, never before the first or after the last, and
            // only where there is room for one `GAP` inside both. The
            // selection goes either way. The bench takes the press as it
            // is; here y is held in the box and x inside the neighbours, as
            // the first drag step would put them.
            let idx = self.anchors.iter().take_while(|a| a[0] < n[0]).count();
            self.sels.clear();
            if idx > 0 && idx < self.anchors.len() {
                let (lo, hi) = (self.anchors[idx - 1][0], self.anchors[idx][0]);
                if hi - lo > 2.0 * GAP {
                    let x = hold(n[0], lo, hi);
                    self.anchors.insert(idx, [x, n[1].clamp(0.0, 1.0), SMOOTH]);
                    hit = Some(idx);
                    inserted = true;
                }
            }
        }
        let Some(i) = hit else {
            return false;
        };
        // A modified press toggles the anchor in the selection; a plain one
        // makes it the selection, or the primary when it was already in it.
        if multi {
            match self.sels.iter().position(|&k| k == i) {
                Some(at) => {
                    self.sels.remove(at);
                }
                None => self.sels.push(i),
            }
        } else if !self.is_sel(i) {
            self.sels.clear();
            self.sels.push(i);
        } else {
            self.sels.retain(|&k| k != i);
            self.sels.push(i);
        }
        if self.is_sel(i) {
            self.drag = Some(Drag {
                i,
                start: n,
                orig: self.anchors.iter().map(|a| [a[0], a[1]]).collect(),
                edited: inserted,
            });
        }
        inserted
    }

    /// The pointer of a drag at `n`, the bench's `pointermove`. True when an
    /// anchor moved.
    fn drag_to(&mut self, n: [f64; 2]) -> bool {
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };
        let len = self.anchors.len();
        let i = drag.i;
        let (dx, dy) = (n[0] - drag.start[0], n[1] - drag.start[1]);
        // Anchors selected together move together, from the left.
        let group: Vec<usize> = if self.sels.len() > 1 && self.sels.contains(&i) {
            let mut g = self.sels.clone();
            g.sort_unstable();
            g
        } else {
            vec![i]
        };
        self.doomed = group.len() == 1
            && i > 0
            && i + 1 < len
            && (n[0] < -OUT_X || n[0] > 1.0 + OUT_X || n[1] < -OUT_Y || n[1] > 1.0 + OUT_Y);
        let before = self.anchors.clone();
        for &k in &group {
            let o = drag.orig[k];
            self.anchors[k][0] = if k == 0 {
                0.0
            } else if k == len - 1 {
                1.0
            } else {
                o[0] + dx
            };
            self.anchors[k][1] = (o[1] + dy).clamp(0.0, 1.0);
        }
        // The height stays a function of x. No moved anchor passes one that
        // stays (the ends stay): each is held between the nearest such on
        // either side, which keeps the order however far the group goes.
        // Then each keeps inside its neighbours, a pass each way, so a
        // group pushed into a fixed anchor closes up from that end.
        let stays = |j: usize| j == 0 || j == len - 1 || !group.contains(&j);
        for &k in &group {
            if k == 0 || k == len - 1 {
                continue;
            }
            let lo = (0..k).rev().find(|&j| stays(j)).map_or(0.0, |j| self.anchors[j][0]);
            let hi = (k + 1..len).find(|&j| stays(j)).map_or(1.0, |j| self.anchors[j][0]);
            self.anchors[k][0] = hold(self.anchors[k][0], lo, hi);
        }
        for &k in &group {
            keep(&mut self.anchors, k);
        }
        for &k in group.iter().rev() {
            keep(&mut self.anchors, k);
        }
        let changed = self.anchors != before;
        drag.edited |= changed;
        changed
    }

    /// The end of a gesture, the bench's `pointerup`: (whether it deleted
    /// the doomed anchor, whether the gesture edited the curve at all).
    fn release(&mut self) -> (bool, bool) {
        let doomed = std::mem::take(&mut self.doomed);
        let Some(drag) = self.drag.take() else {
            return (false, false);
        };
        if doomed && self.is_inner(drag.i) {
            self.anchors.remove(drag.i);
            self.sels.clear();
            return (true, true);
        }
        (false, drag.edited)
    }

    /// Sets the selected anchors' kind. Horizontal on several is an
    /// alignment as well: they all take the primary's height. True when
    /// anything changed.
    fn set_kind(&mut self, kind: f64) -> bool {
        let Some(primary) = self.primary() else {
            return false;
        };
        let level = self.anchors[primary][1];
        let align = kind == HORIZONTAL && self.sels.len() > 1;
        let mut changed = false;
        for &k in &self.sels {
            let a = &mut self.anchors[k];
            if align && a[1] != level {
                a[1] = level;
                changed = true;
            }
            if a[2] != kind {
                a[2] = kind;
                changed = true;
            }
        }
        changed
    }

    /// Deletes the selected inner anchors (the ends stay). True when any
    /// went.
    fn delete_selected(&mut self) -> bool {
        let mut gone: Vec<usize> = self.sels.iter().copied().filter(|&k| self.is_inner(k)).collect();
        if gone.is_empty() {
            return false;
        }
        gone.sort_unstable_by(|a, b| b.cmp(a));
        for k in gone {
            self.anchors.remove(k);
        }
        self.sels.clear();
        self.drag = None;
        self.doomed = false;
        true
    }
}

// ---------------------------------------------------------------------------
// The canvas geometry
// ---------------------------------------------------------------------------

/// The canvas as drawn, and the unit box `PAD` inside it, y up (the bench's
/// `pxOf` and `at`). Positions are absolute, in the space of `fe.abs`: the
/// drawn rect already carries any scroll.
#[derive(Clone, Copy, Debug)]
struct Frame {
    pos: DVec2,
    size: DVec2,
}

impl Frame {
    fn new(r: Rect) -> Self {
        Self { pos: r.pos, size: r.size }
    }

    fn unit(&self) -> DVec2 {
        dvec2((self.size.x - 2.0 * PAD).max(1.0), (self.size.y - 2.0 * PAD).max(1.0))
    }

    fn to_px(&self, x: f64, y: f64) -> DVec2 {
        let u = self.unit();
        dvec2(self.pos.x + PAD + x * u.x, self.pos.y + self.size.y - PAD - y * u.y)
    }

    fn to_unit(&self, p: DVec2) -> [f64; 2] {
        let u = self.unit();
        [
            (p.x - self.pos.x - PAD) / u.x,
            (self.pos.y + self.size.y - PAD - p.y) / u.y,
        ]
    }
}

fn to_f32(p: DVec2) -> Vec2f {
    vec2(p.x as f32, p.y as f32)
}

fn square(c: DVec2, half: f64) -> Rect {
    Rect {
        pos: c - dvec2(half, half),
        size: dvec2(2.0 * half, 2.0 * half),
    }
}

/// Consecutive pairs of `pts` at least `STEP_PX` apart (the last point
/// always closes a pair), so a stroke or a fill takes a quad per couple of
/// pixels rather than one per sample.
struct Steps<I: Iterator<Item = DVec2>> {
    pts: std::iter::Peekable<I>,
    from: Option<DVec2>,
}

fn steps<I: Iterator<Item = DVec2>>(pts: I) -> Steps<I> {
    Steps { pts: pts.peekable(), from: None }
}

impl<I: Iterator<Item = DVec2>> Iterator for Steps<I> {
    type Item = (DVec2, DVec2);

    fn next(&mut self) -> Option<(DVec2, DVec2)> {
        let a = match self.from {
            Some(a) => a,
            None => {
                let a = self.pts.next()?;
                self.from = Some(a);
                a
            }
        };
        while let Some(p) = self.pts.next() {
            if self.pts.peek().is_some() && a.distance(&p) < STEP_PX {
                continue;
            }
            self.from = Some(p);
            return Some((a, p));
        }
        None
    }
}

/// One stroke segment from `a` to `b`, on a quad just big enough for it.
fn stroke(draw: &mut DrawCurveSegment, cx: &mut Cx2d, a: DVec2, b: DVec2) {
    let m = draw.thickness as f64 * 0.5 + 2.0;
    let lo = dvec2(a.x.min(b.x) - m, a.y.min(b.y) - m);
    let hi = dvec2(a.x.max(b.x) + m, a.y.max(b.y) + m);
    draw.seg_a = to_f32(a - lo);
    draw.seg_b = to_f32(b - lo);
    draw.draw_abs(cx, Rect { pos: lo, size: hi - lo });
}

// ---------------------------------------------------------------------------
// CurveEditor
// ---------------------------------------------------------------------------

/// A curve of anchors over a unit box: the canvas (the `canvas` child's
/// place, drawn over by the editor) and a toolbar of the kind row and
/// Delete under it.
#[derive(Script, Widget)]
pub struct CurveEditor {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[live]
    draw_canvas: DrawCurveCanvas,
    #[live]
    draw_fill: DrawCurveFill,
    #[live]
    draw_curve: DrawCurveSegment,
    #[live]
    draw_guide: DrawCurveSegment,
    #[live]
    draw_mirror: DrawCurveSegment,
    #[live]
    draw_arm: DrawCurveSegment,
    #[live]
    draw_knob: DrawCurveKnob,
    #[live]
    draw_text: DrawText,

    /// The DSL's `anchors`, declared here for the DSL and the inspector;
    /// the curve itself is read in `on_after_apply` and lives in the model
    /// ([`CurveEditor::anchors`]).
    #[live]
    anchors: Vec<ScriptValue>,
    #[live]
    left_label: String,
    #[live]
    right_label: String,
    #[live(-1.0)]
    guide: f64,
    #[live]
    guide_label: String,
    #[live]
    mirror: bool,

    /// The DSL curve last adopted: an apply that brings the same one again
    /// (a restyle) leaves an edited curve alone.
    #[rust]
    applied: Option<Vec<CurveAnchor>>,
    #[rust]
    model: CurveModel,
    /// The anchors with their tangents, and the polyline through them, as
    /// of the last draw; `dirty` says the curve has changed since.
    #[rust]
    fixed: Vec<Pt>,
    #[rust]
    poly: Vec<[f64; 2]>,
    #[rust(true)]
    dirty: bool,
    /// The toolbar as last set: (kind row enabled, Delete enabled, the kind
    /// whose rung is lit). `None` after an apply, which may have rebuilt the
    /// children, so the next draw sets them again.
    #[rust]
    tools: Option<(bool, bool, Option<f64>)>,
}

impl ScriptHook for CurveEditor {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        // Read off the apply itself rather than the field: there the arrays
        // are this apply's own, and an apply that does not bring `anchors`
        // (a control's chunk writing `mirror`) finds none and leaves the
        // curve alone. The field keeps whatever the derive last put in it.
        if let Some(values) = dsl_anchors(vm, apply, value) {
            let dsl = tidy_anchors(&read_anchors(vm, &values));
            if self.applied.as_ref() != Some(&dsl) {
                self.model.set_anchors(&dsl);
                self.applied = Some(dsl);
                self.dirty = true;
            }
        }
        // Any apply may have rebuilt or restyled the toolbar's children,
        // which then stand enabled and unlit whatever the selection says.
        self.tools = None;
        self.view.redraw(vm.cx_mut());
    }
}

impl CurveEditor {
    /// The curve.
    pub fn anchors(&self) -> &[CurveAnchor] {
        &self.model.anchors
    }

    /// Replaces the curve, tidied (x and y held in 0..=1, kinds whole,
    /// ordered by x; fewer than two anchors is the default line), clears
    /// the selection and redraws. Emits nothing.
    pub fn set_anchors(&mut self, cx: &mut Cx, anchors: &[CurveAnchor]) {
        self.model.set_anchors(&tidy_anchors(anchors));
        self.dirty = true;
        self.sync_tools(cx);
        self.redraw_all(cx);
    }

    fn redraw_all(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
        self.draw_canvas.redraw(cx);
    }

    /// Where the pointer at `abs` falls, in curve units, on the canvas as
    /// last drawn.
    fn unit_at(&self, cx: &Cx, abs: DVec2) -> [f64; 2] {
        Frame::new(self.draw_canvas.area().rect(cx)).to_unit(abs)
    }

    fn press(&mut self, cx: &mut Cx, n: [f64; 2], multi: bool) {
        let changed = self.model.press(n, multi);
        self.after_edit(cx, changed, false);
    }

    fn drag_to(&mut self, cx: &mut Cx, n: [f64; 2]) {
        let changed = self.model.drag_to(n);
        self.after_edit(cx, changed, false);
    }

    fn release(&mut self, cx: &mut Cx) {
        let (changed, commit) = self.model.release();
        self.after_edit(cx, changed, commit);
    }

    fn set_kind(&mut self, cx: &mut Cx, kind: f64) {
        let changed = self.model.set_kind(kind);
        self.after_edit(cx, changed, changed);
    }

    fn delete_selected(&mut self, cx: &mut Cx) {
        let changed = self.model.delete_selected();
        self.after_edit(cx, changed, changed);
    }

    /// Reports an edit and redraws (a press that only selects redraws too).
    fn after_edit(&mut self, cx: &mut Cx, changed: bool, commit: bool) {
        let uid = self.widget_uid();
        if changed {
            self.dirty = true;
            cx.widget_action(uid, CurveEditorAction::Changed(self.model.anchors.clone()));
        }
        if commit {
            cx.widget_action(uid, CurveEditorAction::Committed(self.model.anchors.clone()));
        }
        self.sync_tools(cx);
        self.redraw_all(cx);
    }

    /// The kind row needs a selection and lights the kind it all shares,
    /// Delete needs an inner anchor in it. A disabled control is set by
    /// both switches where it has two: `enabled` refuses the press, the
    /// disabled track draws the face dimmed.
    fn sync_tools(&mut self, cx: &mut Cx) {
        let state = (
            !self.model.sels.is_empty(),
            self.model.can_delete(),
            self.model.shared_kind(),
        );
        if self.tools == Some(state) {
            return;
        }
        self.tools = Some(state);
        for (id, kind) in KIND_RUNGS {
            let rung = self.view.radio_button(cx, &[id]);
            rung.set_active(cx, state.2 == Some(kind), Animate::No);
            rung.set_disabled(cx, !state.0);
        }
        let delete = self.view.button(cx, ids!(delete));
        delete.set_enabled(cx, state.1);
        delete.set_disabled(cx, !state.1);
    }

    /// The canvas over `rect`: ground and grid, the area under the curve,
    /// the guide, the curve, the mirrored tooth, the tangent handles, the
    /// anchors and the captions, in that order (each kind of quad drawn in
    /// one run, so each is one draw call).
    fn draw_curve_canvas(&mut self, cx: &mut Cx2d, rect: Rect) {
        if self.dirty {
            self.dirty = false;
            self.fixed = fixed_tangents(&self.model.anchors);
            self.poly = polyline_of(&self.fixed);
        }
        let f = Frame::new(rect);
        let unit = f.unit();
        // All of it inside the canvas, as the bench's canvas element keeps
        // it: the end anchors' tangents reach a sixth of their span past
        // the box, and would otherwise spill over what is round the editor.
        cx.push_clip_rect(rect);
        self.draw_canvas.unit = vec4(PAD as f32, PAD as f32, unit.x as f32, unit.y as f32);
        self.draw_canvas.draw_abs(cx, rect);

        // The area under the curve, a column a step: the columns meet
        // without overlapping, so the translucent fill stays even.
        let floor = f.to_px(0.0, 0.0).y;
        for (a, b) in steps(self.poly.iter().map(|q| f.to_px(q[0], q[1]))) {
            if b.x - a.x < 1e-6 {
                continue;
            }
            let top = a.y.min(b.y) - 1.0;
            let lo = dvec2(a.x, top);
            self.draw_fill.fill_a = to_f32(a - lo);
            self.draw_fill.fill_b = to_f32(b - lo);
            self.draw_fill.draw_abs(
                cx,
                Rect {
                    pos: lo,
                    size: dvec2(b.x - a.x, (floor - top).max(0.0)),
                },
            );
        }

        let guide = (self.guide >= 0.0).then(|| f.to_px(0.0, self.guide.min(1.0)).y);
        if let Some(y) = guide {
            let (a, b) = (dvec2(f.pos.x + PAD, y), dvec2(f.pos.x + f.size.x - PAD, y));
            stroke(&mut self.draw_guide, cx, a, b);
        }

        for (a, b) in steps(self.poly.iter().map(|q| f.to_px(q[0], q[1]))) {
            stroke(&mut self.draw_curve, cx, a, b);
        }

        // A whole tooth across the top strip: the curve, then its mirror,
        // which is what goes round the knob. Whether a crest meets its
        // neighbour in a point or a flat only shows in the pair.
        if self.mirror {
            self.draw_mirror.ground = self.draw_canvas.color;
            let strip = unit.y * 0.22;
            let half = unit.x * 0.5;
            let (x0, y0) = (f.pos.x + PAD, f.pos.y + PAD + strip);
            let there = self.poly.iter().map(|q| dvec2(x0 + q[0] * half, y0 - q[1] * strip));
            let back = self
                .poly
                .iter()
                .rev()
                .map(|q| dvec2(x0 + half + (1.0 - q[0]) * half, y0 - q[1] * strip));
            for (a, b) in steps(there.chain(back)) {
                stroke(&mut self.draw_mirror, cx, a, b);
            }
        }

        // The derived tangents of every anchor that has some.
        for t in &self.fixed {
            if t.ty == POINT as u8 {
                continue;
            }
            let at = f.to_px(t.x, t.y);
            for (dx, dy) in [(t.ix, t.iy), (t.ox, t.oy)] {
                stroke(&mut self.draw_arm, cx, at, f.to_px(t.x + dx, t.y + dy));
            }
        }
        self.draw_knob.handle = 1.0;
        self.draw_knob.ring = 0.0;
        self.draw_knob.doomed = 0.0;
        for t in &self.fixed {
            if t.ty == POINT as u8 {
                continue;
            }
            self.draw_knob.kind = t.ty as f32;
            for (dx, dy) in [(t.ix, t.iy), (t.ox, t.oy)] {
                let h = f.to_px(t.x + dx, t.y + dy);
                self.draw_knob.draw_abs(cx, square(h, DOT_HALF));
            }
        }
        self.draw_knob.handle = 0.0;
        let primary = self.model.primary();
        let doomed = self.model.doomed_anchor();
        for (i, t) in self.fixed.iter().enumerate() {
            self.draw_knob.kind = t.ty as f32;
            self.draw_knob.ring = if primary == Some(i) {
                2.0
            } else if self.model.is_sel(i) {
                1.0
            } else {
                0.0
            };
            self.draw_knob.doomed = (doomed == Some(i)) as u8 as f32;
            self.draw_knob.draw_abs(cx, square(f.to_px(t.x, t.y), KNOB_HALF));
        }

        // The captions, small, in the bottom padding and over the guide.
        let line = self.draw_text.text_style.font_size as f64 * 1.4;
        let y = f.pos.y + f.size.y - 3.0 - line;
        if !self.left_label.is_empty() {
            self.draw_text
                .draw_abs(cx, dvec2(f.pos.x + PAD, y), &self.left_label);
        }
        if !self.right_label.is_empty() {
            let w = measure(&self.draw_text, cx, &self.right_label);
            self.draw_text
                .draw_abs(cx, dvec2(f.pos.x + f.size.x - PAD - w, y), &self.right_label);
        }
        if let Some(gy) = guide.filter(|_| !self.guide_label.is_empty()) {
            self.draw_text
                .draw_abs(cx, dvec2(f.pos.x + PAD + 4.0, gy - 3.0 - line), &self.guide_label);
        }
        cx.pop_clip_rect();
    }
}

impl Widget for CurveEditor {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.tools.is_none() {
            self.sync_tools(cx);
        }
        let step = self.view.draw_walk(cx, scope, walk);
        // The canvas goes over the place the `canvas` child took in the
        // layout, once the view has laid it out.
        if step.is_done() {
            let rect = self.view.widget(cx, ids!(canvas)).area().rect(cx);
            self.draw_curve_canvas(cx, rect);
        }
        step
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if !actions.is_empty() {
            for (id, kind) in KIND_RUNGS {
                if self.view.radio_button(cx, &[id]).clicked(&actions) {
                    // The rung lit itself, or put itself out when it was
                    // lit: the row goes back to what the selection says
                    // whether or not the kind changed anything.
                    self.tools = None;
                    self.set_kind(cx, kind);
                }
            }
            if self.view.button(cx, ids!(delete)).clicked(&actions) {
                self.delete_selected(cx);
            }
        }
        // `hits` takes the pointer on the press: until the release the drag
        // is the canvas's, wherever the pointer goes.
        match event.hits(cx, self.draw_canvas.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                if self.model.drag.is_none() {
                    let n = self.unit_at(cx, fe.abs);
                    cx.set_cursor(if matches!(self.model.pick(n), Some(Pick::Anchor(_))) {
                        MouseCursor::Grab
                    } else {
                        MouseCursor::Default
                    });
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                let n = self.unit_at(cx, fe.abs);
                let m = fe.modifiers;
                self.press(cx, n, m.shift || m.control || m.logo);
                if self.model.drag.is_some() {
                    cx.set_cursor(MouseCursor::Grabbing);
                }
            }
            Hit::FingerMove(fe) => {
                if self.model.drag.is_some() {
                    cx.set_cursor(MouseCursor::Grabbing);
                    let n = self.unit_at(cx, fe.abs);
                    self.drag_to(cx, n);
                }
            }
            Hit::FingerUp(_) => {
                if self.model.drag.is_some() {
                    self.release(cx);
                }
            }
            _ => (),
        }
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        let mut s = String::new();
        for (i, a) in self.model.anchors.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            let _ = write!(s, "[{:.3}, {:.3}, {}]", a[0], a[1], kind_name(a[2]));
        }
        if !self.model.sels.is_empty() {
            let _ = write!(s, " selected {:?}", self.model.sels);
        }
        Some(s)
    }
}

impl CurveEditorRef {
    /// Replaces the curve and redraws; clears the selection. Emits nothing.
    pub fn set_anchors(&self, cx: &mut Cx, anchors: &[CurveAnchor]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_anchors(cx, anchors);
        }
    }

    /// The curve.
    pub fn anchors(&self) -> Vec<CurveAnchor> {
        self.borrow()
            .map(|inner| inner.anchors().to_vec())
            .unwrap_or_default()
    }

    /// The curve while it moves: a drag step, an insert, a kind, a delete
    /// (the last of them in this batch).
    pub fn changed(&self, actions: &Actions) -> Option<Vec<CurveAnchor>> {
        actions
            .filter_widget_actions_cast::<CurveEditorAction>(self.widget_uid())
            .filter_map(|a| match a {
                CurveEditorAction::Changed(v) => Some(v),
                _ => None,
            })
            .last()
    }

    /// The curve a gesture settled on.
    pub fn committed(&self, actions: &Actions) -> Option<Vec<CurveAnchor>> {
        actions
            .filter_widget_actions_cast::<CurveEditorAction>(self.widget_uid())
            .filter_map(|a| match a {
                CurveEditorAction::Committed(v) => Some(v),
                _ => None,
            })
            .last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use crate::makepad_script::traits::ScriptApply;
    use std::cell::Cell;

    fn test_cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }

    fn near(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-12 && (a[1] - b[1]).abs() < 1e-12
    }

    // -- The curve ------------------------------------------------------------

    /// Two points: no tangents, so both control points sit on their anchors
    /// and the cubic runs along the chord as 3t^2 - 2t^3, sampled 97 times.
    #[test]
    fn a_point_to_point_segment_is_the_cubic_with_its_handles_on_the_anchors() {
        let poly = curve_polyline(&[[0.0, 0.0, POINT], [1.0, 1.0, POINT]]);
        assert_eq!(poly.len(), SEGMENT_STEPS + 1);
        assert!(near(poly[0], [0.0, 0.0]));
        // t = 1/4: 3/16 - 2/64.
        assert!(near(poly[24], [0.15625, 0.15625]), "{:?}", poly[24]);
        assert!(near(poly[48], [0.5, 0.5]), "{:?}", poly[48]);
        assert!(near(poly[96], [1.0, 1.0]));
    }

    /// A smooth peak, worked by hand through `fixTangents`: the peak's
    /// tangent goes flat (its neighbours are both lower) and reaches a sixth
    /// either way, the third of the way to each neighbour it is allowed; the
    /// ends take a sixth of the neighbour difference, (1/12, +-1/6). At t
    /// 1/2 the Bernstein weights are 1/8, 3/8, 3/8, 1/8.
    #[test]
    fn a_smooth_peak_goes_flat_and_keeps_its_handles_to_a_third() {
        let poly = curve_polyline(&[[0.0, 0.0, SMOOTH], [0.5, 1.0, SMOOTH], [1.0, 0.0, SMOOTH]]);
        assert_eq!(poly.len(), 2 * (SEGMENT_STEPS + 1));
        // (0,0) (1/12,1/6) (1/3,1) (1/2,1): x = 3/96 + 1/8 + 1/16, y = 1/16 + 3/8 + 1/8.
        assert!(near(poly[48], [0.21875, 0.5625]), "{:?}", poly[48]);
        assert!(near(poly[96], [0.5, 1.0]));
        assert!(near(poly[97], [0.5, 1.0]), "the next segment starts on the peak again");
        // (1/2,1) (2/3,1) (11/12,1/6) (1,0), the mirror image.
        assert!(near(poly[97 + 48], [0.78125, 0.5625]), "{:?}", poly[97 + 48]);
        // Flat at the peak: the samples either side stand level with it
        // to first order, which a tangent carrying the neighbours' slope
        // would not.
        assert!(poly[95][1] < 1.0 && 1.0 - poly[95][1] < 1e-3, "{:?}", poly[95]);
    }

    /// A horizontal anchor is a smooth one held level whatever its
    /// neighbours do, and a point anchor has no tangents at all.
    #[test]
    fn horizontal_holds_level_and_point_bends_nothing() {
        let rising = [[0.0, 0.0, SMOOTH], [0.5, 0.5, HORIZONTAL], [1.0, 1.0, SMOOTH]];
        let p = fixed_tangents(&rising);
        assert_eq!(p[1].oy, 0.0);
        assert_eq!(p[1].iy, 0.0);
        assert!(p[1].ox > 0.0 && p[1].ix == -p[1].ox);
        let straight = fixed_tangents(&[[0.0, 0.0, POINT], [0.5, 0.5, POINT], [1.0, 1.0, POINT]]);
        assert!(straight.iter().all(|t| t.ix == 0.0 && t.iy == 0.0 && t.ox == 0.0 && t.oy == 0.0));
    }

    /// Resampling a straight line reads the line back, whichever way it is
    /// drawn: the default smooth pair, whose handles lie on the chord, and a
    /// point pair.
    #[test]
    fn resampling_a_straight_line_reads_the_line() {
        let down = curve_resample(&DEFAULT_LINE, 11);
        assert_eq!(down.len(), 11);
        for (i, y) in down.iter().enumerate() {
            let want = 1.0 - i as f64 / 10.0;
            assert!((y - want).abs() < 1e-9, "tap {i}: {y} for {want}");
        }
        let up = curve_resample(&[[0.0, 0.0, POINT], [1.0, 1.0, POINT]], 256);
        for (i, y) in up.iter().enumerate() {
            let want = i as f64 / 255.0;
            assert!((y - want).abs() < 1e-9, "tap {i}: {y} for {want}");
        }
    }

    /// The edges of the resample: taps outside the anchors' span take the
    /// nearest end, one anchor is a level line and none reads zero.
    #[test]
    fn resampling_holds_the_ends_and_survives_a_short_curve() {
        let middle = curve_resample(&[[0.25, 0.2, POINT], [0.75, 0.8, POINT]], 5);
        assert_eq!(middle[0], 0.2);
        assert_eq!(middle[4], 0.8);
        assert!((middle[2] - 0.5).abs() < 1e-9);
        assert_eq!(curve_resample(&[[0.3, 0.7, SMOOTH]], 3), vec![0.7; 3]);
        assert_eq!(curve_resample(&[], 2), vec![0.0; 2]);
        assert_eq!(curve_resample(&DEFAULT_LINE, 1), vec![1.0]);
        assert!(curve_polyline(&[[0.3, 0.7, SMOOTH]]).is_empty());
    }

    /// Whatever a host hands in comes out editable.
    #[test]
    fn a_curve_is_tidied_on_the_way_in() {
        let tidy = tidy_anchors(&[[1.2, 0.5, 1.4], [0.5, -1.0, 7.0], [0.0, 0.3, 2.0], [f64::NAN, 0.0, 1.0]]);
        assert_eq!(tidy, vec![[0.0, 0.3, CORNER], [0.5, 0.0, HORIZONTAL], [1.0, 0.5, SMOOTH]]);
        assert_eq!(tidy_anchors(&[[0.5, 0.5, SMOOTH]]), DEFAULT_LINE.to_vec());
    }

    // -- The editing, through the widget ------------------------------------

    /// An editor from the library's DSL on the pooled context, holding
    /// `anchors`, with nothing reported yet.
    fn editor(cx: &mut Cx, anchors: &[CurveAnchor]) -> CurveEditor {
        let mut e = cx.with_vm(CurveEditor::script_new_with_default);
        e.set_anchors(cx, anchors);
        cx.new_actions.clear();
        e
    }

    /// What the editor reported since the last call.
    fn reported(cx: &mut Cx, e: &CurveEditor) -> Vec<CurveEditorAction> {
        let actions = std::mem::take(&mut cx.new_actions);
        actions
            .filter_widget_actions_cast::<CurveEditorAction>(e.widget_uid())
            .collect()
    }

    fn changed(acts: &[CurveEditorAction]) -> usize {
        acts.iter().filter(|a| matches!(a, CurveEditorAction::Changed(_))).count()
    }

    fn committed(acts: &[CurveEditorAction]) -> Option<&Vec<CurveAnchor>> {
        acts.iter().find_map(|a| match a {
            CurveEditorAction::Committed(v) => Some(v),
            _ => None,
        })
    }

    const FOUR: [CurveAnchor; 4] = [
        [0.0, 0.0, SMOOTH],
        [0.3, 0.5, SMOOTH],
        [0.6, 0.5, SMOOTH],
        [1.0, 1.0, SMOOTH],
    ];

    /// No anchor stands left of the one before it.
    fn ordered(anchors: &[CurveAnchor]) -> bool {
        anchors.windows(2).all(|w| w[0][0] <= w[1][0])
    }

    /// The toolbar as its children show it: (the kind whose rung is lit,
    /// the kind row live, Delete live). At most one rung is lit, the row is
    /// live or dead as one, and Delete's two switches agree: pressable and
    /// drawn at full strength, or neither.
    fn toolbar(cx: &Cx, e: &CurveEditor) -> (Option<f64>, bool, bool) {
        let mut lit = None;
        let mut live = Vec::new();
        for (id, kind) in KIND_RUNGS {
            let rung = e.view.radio_button(cx, &[id]);
            assert!(rung.borrow().is_some(), "no rung {id:?}");
            if rung.active(cx) {
                assert_eq!(lit, None, "two rungs lit");
                lit = Some(kind);
            }
            live.push(!rung.disabled(cx));
        }
        assert!(live.iter().all(|&l| l == live[0]), "a row split live and dead: {live:?}");
        let delete = e.view.button(cx, ids!(delete));
        let enabled = delete.borrow().is_some_and(|b| b.enabled());
        assert_eq!(enabled, !delete.disabled(cx), "Delete's switches disagree");
        (lit, live[0], enabled)
    }

    /// The DSL's default is the bench's two-anchor line, and the toolbar
    /// starts with nothing to act on: the kind row dead and unlit, Delete
    /// dead, and all of it drawn dimmed.
    #[test]
    fn a_new_editor_holds_the_default_line_and_an_idle_toolbar() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let mut e = cx.with_vm(CurveEditor::script_new_with_default);
            assert_eq!(e.anchors(), &DEFAULT_LINE[..]);
            assert!(!e.mirror);
            assert_eq!(e.guide, -1.0);
            e.sync_tools(&mut cx);
            assert_eq!(e.tools, Some((false, false, None)));
            assert_eq!(toolbar(&cx, &e), (None, false, false));
        });
    }

    /// The toolbar is set again after an apply: a restyle or a rebuild
    /// stands the children live and unlit, whatever the selection says,
    /// and the next draw puts them back.
    #[test]
    fn an_apply_sets_the_toolbar_again() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let (widget, value) = cx.with_vm(|vm| {
                let value = crate::script_eval!(vm, {
                    use mod.widgets.*
                    CurveEditor{anchors: [[0, 0, 1], [0.5, 0.5, 2], [1, 1, 1]]}
                });
                (WidgetRef::script_from_value(vm, value), value)
            });
            let editor = widget.as_curve_editor();
            {
                let mut e = editor.borrow_mut().expect("a CurveEditor");
                e.sync_tools(&mut cx);
                assert_eq!(toolbar(&cx, &e), (None, false, false));
                // What a rebuild leaves: every control standing live.
                for id in [live_id!(smooth), live_id!(corner), live_id!(horizontal), live_id!(point), live_id!(delete)] {
                    e.view.widget(&cx, &[id]).set_disabled(&mut cx, false);
                }
                e.view.button(&cx, ids!(delete)).set_enabled(&mut cx, true);
                e.view.radio_button(&cx, ids!(corner)).set_active(&mut cx, true, Animate::No);
                assert_eq!(toolbar(&cx, &e), (Some(CORNER), true, true));
            }
            cx.with_vm(|vm| {
                let mut target = widget.clone();
                target.script_apply(vm, &Apply::Eval, &mut Scope::empty(), value)
            });
            let mut e = editor.borrow_mut().unwrap();
            assert_eq!(e.tools, None, "the apply asks for the toolbar again");
            e.sync_tools(&mut cx);
            assert_eq!(toolbar(&cx, &e), (None, false, false));
        });
    }

    /// A chunk applied over a widget's own source, the way the storybook's
    /// controls and the tweaker write one property.
    fn apply_chunk(cx: &mut Cx, widget: &WidgetRef, chunk: &str) {
        let code = format!("use mod.prelude.widgets.*\n__script_source__{chunk};");
        let errors = cx.with_vm(|vm| {
            vm.bx.captured_errors = Some(Vec::new());
            let script_mod = crate::makepad_script::ScriptMod {
                cargo_manifest_path: String::new(),
                module_path: "curve_editor_test".to_string(),
                file: "test://chunk".to_string(),
                line: 2 + chunk.len(),
                column: 1,
                code,
                values: Vec::new(),
            };
            let mut target = widget.clone();
            target.script_apply_eval(vm, script_mod);
            vm.take_errors()
        });
        assert!(errors.is_empty(), "{chunk}: {errors:?}");
    }

    /// The anchors a DSL writes are read (a missing kind is smooth), and a
    /// restyle that brings the same curve again leaves an edit alone; a
    /// chunk that writes another property touches nothing else, and one
    /// that writes a new curve takes it.
    #[test]
    fn the_dsl_curve_is_read_and_a_restyle_keeps_the_edit() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let (widget, value) = cx.with_vm(|vm| {
                let value = crate::script_eval!(vm, {
                    use mod.widgets.*
                    CurveEditor{
                        anchors: [[0, 1, 2], [0.4, 0.72], [1, 0, 2]]
                        left_label: "CREST"
                        mirror: true
                        guide: 0.5
                    }
                });
                (WidgetRef::script_from_value(vm, value), value)
            });
            let editor = widget.as_curve_editor();
            assert_eq!(editor.anchors(), vec![[0.0, 1.0, CORNER], [0.4, 0.72, SMOOTH], [1.0, 0.0, CORNER]]);
            {
                let e = editor.borrow().expect("a CurveEditor");
                assert!(e.mirror);
                assert_eq!(e.guide, 0.5);
                assert_eq!(e.left_label, "CREST");
            }

            editor.set_anchors(&mut cx, &FOUR);
            cx.with_vm(|vm| {
                let mut target = widget.clone();
                target.script_apply(vm, &Apply::Eval, &mut Scope::empty(), value)
            });
            assert_eq!(editor.anchors(), FOUR.to_vec(), "the same DSL again is a restyle, not a reset");

            apply_chunk(&mut cx, &widget, "{mirror: false}");
            assert!(!editor.borrow().unwrap().mirror);
            assert_eq!(editor.borrow().unwrap().tools, None, "a chunk sets the toolbar again too");
            assert_eq!(editor.anchors(), FOUR.to_vec(), "a chunk without anchors leaves the curve");

            apply_chunk(&mut cx, &widget, "{anchors: [[0, 0, 0], [1, 0.5, 0]]}");
            assert_eq!(editor.anchors(), vec![[0.0, 0.0, POINT], [1.0, 0.5, POINT]]);
        });
    }

    /// A press on empty canvas inserts a smooth anchor between its
    /// x-neighbours and selects it; the release commits the insert. Outside
    /// the first or last anchor a press inserts nothing and drops the
    /// selection.
    #[test]
    fn a_press_on_empty_canvas_inserts_a_smooth_anchor() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let mut e = editor(&mut cx, &DEFAULT_LINE);
            e.press(&mut cx, [0.5, 0.2], false);
            assert_eq!(e.anchors(), &[DEFAULT_LINE[0], [0.5, 0.2, SMOOTH], DEFAULT_LINE[1]]);
            assert_eq!(e.model.sels, vec![1]);
            assert_eq!(e.tools, Some((true, true, Some(SMOOTH))), "a selected inner anchor can be deleted");
            assert_eq!(toolbar(&cx, &e), (Some(SMOOTH), true, true));
            let acts = reported(&mut cx, &e);
            assert_eq!(changed(&acts), 1);
            assert!(committed(&acts).is_none(), "the gesture is still under way");
            e.release(&mut cx);
            let acts = reported(&mut cx, &e);
            assert_eq!(committed(&acts).map(Vec::len), Some(3));

            // A press on an anchor only selects it: nothing to report.
            e.press(&mut cx, [0.51, 0.21], false);
            e.release(&mut cx);
            assert!(reported(&mut cx, &e).is_empty());

            // Left of the first anchor: nothing to insert between.
            e.press(&mut cx, [-0.06, 0.5], false);
            assert_eq!(e.anchors().len(), 3);
            assert!(e.model.sels.is_empty());
            assert_eq!(e.tools, Some((false, false, None)));
            assert_eq!(toolbar(&cx, &e), (None, false, false));
            e.release(&mut cx);
            assert!(reported(&mut cx, &e).is_empty());
        });
    }

    /// A drag moves the selected anchor by the pointer's travel, keeps it
    /// 0.01 inside its x-neighbours and y in the box; the ends keep x 0 and
    /// 1 whatever the pointer does.
    #[test]
    fn a_drag_moves_an_anchor_and_keeps_x_inside_its_neighbours() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let mut e = editor(&mut cx, &FOUR);
            e.press(&mut cx, [0.31, 0.5], false);
            assert_eq!(e.model.sels, vec![1]);
            e.drag_to(&mut cx, [0.41, 0.6]);
            assert!(near([e.anchors()[1][0], e.anchors()[1][1]], [0.4, 0.6]));
            // Past the next anchor and above the box, but within the margin
            // a drag-out needs: held, not doomed.
            e.drag_to(&mut cx, [0.91, 1.05]);
            assert!(near([e.anchors()[1][0], e.anchors()[1][1]], [0.59, 1.0]), "{:?}", e.anchors()[1]);
            assert_eq!(e.model.doomed_anchor(), None);
            let acts = reported(&mut cx, &e);
            assert_eq!(changed(&acts), 2);
            e.release(&mut cx);
            let acts = reported(&mut cx, &e);
            let settled = committed(&acts).expect("the drag commits on release");
            assert_eq!(settled.len(), 4);
            assert!(near([settled[1][0], settled[1][1]], [0.59, 1.0]), "{:?}", settled[1]);

            e.press(&mut cx, [0.0, 0.0], false);
            e.drag_to(&mut cx, [0.2, 0.3]);
            assert!(near([e.anchors()[0][0], e.anchors()[0][1]], [0.0, 0.3]), "{:?}", e.anchors()[0]);
            e.release(&mut cx);
        });
    }

    /// Shift toggles anchors into the selection and they drag together;
    /// Horizontal on several levels them at the primary's height.
    #[test]
    fn a_selection_drags_together_and_horizontal_levels_it() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let mut e = editor(&mut cx, &FOUR);
            e.press(&mut cx, [0.6, 0.5], false);
            e.release(&mut cx);
            e.press(&mut cx, [0.3, 0.5], true);
            assert_eq!(e.model.sels, vec![2, 1], "the last picked is the primary");
            e.drag_to(&mut cx, [0.35, 0.3]);
            assert!(near([e.anchors()[1][0], e.anchors()[1][1]], [0.35, 0.3]));
            assert!(near([e.anchors()[2][0], e.anchors()[2][1]], [0.65, 0.3]));
            e.release(&mut cx);
            reported(&mut cx, &e);

            e.model.anchors[2][1] = 0.8;
            e.set_kind(&mut cx, HORIZONTAL);
            assert_eq!(e.anchors()[1][2], HORIZONTAL);
            assert_eq!(e.anchors()[2][2], HORIZONTAL);
            assert_eq!(e.anchors()[2][1], 0.3, "levelled at the primary's height");
            assert_eq!(toolbar(&cx, &e), (Some(HORIZONTAL), true, true));
            let acts = reported(&mut cx, &e);
            assert_eq!((changed(&acts), committed(&acts).is_some()), (1, true));

            // Toggled out again, one anchor takes a kind on its own.
            e.press(&mut cx, [0.35, 0.3], true);
            assert_eq!(e.model.sels, vec![2]);
            e.set_kind(&mut cx, POINT);
            assert_eq!(e.anchors()[2][2], POINT);
            assert_eq!(e.anchors()[1][2], HORIZONTAL);
            assert_eq!(toolbar(&cx, &e), (Some(POINT), true, true));
            // The same kind again changes nothing and says nothing.
            reported(&mut cx, &e);
            e.set_kind(&mut cx, POINT);
            assert!(reported(&mut cx, &e).is_empty());

            // Two kinds in the selection: no rung is theirs.
            e.press(&mut cx, [0.35, 0.3], true);
            assert_eq!(e.model.sels, vec![2, 1]);
            assert_eq!(toolbar(&cx, &e), (None, true, true));
        });
    }

    /// Delete takes the selected inner anchors and never an end; a lone
    /// inner anchor dragged out of the box is doomed while it is out and
    /// goes on release.
    #[test]
    fn delete_and_a_drag_out_remove_inner_anchors_only() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let mut e = editor(&mut cx, &FOUR);
            e.press(&mut cx, [0.0, 0.0], false);
            e.release(&mut cx);
            assert_eq!(e.tools, Some((true, false, Some(SMOOTH))), "an end alone cannot be deleted");
            assert_eq!(toolbar(&cx, &e), (Some(SMOOTH), true, false));
            e.press(&mut cx, [0.3, 0.5], true);
            e.release(&mut cx);
            e.delete_selected(&mut cx);
            assert_eq!(e.anchors(), &[FOUR[0], FOUR[2], FOUR[3]]);
            assert!(e.model.sels.is_empty());
            assert!(committed(&reported(&mut cx, &e)).is_some());

            e.press(&mut cx, [0.6, 0.5], false);
            e.drag_to(&mut cx, [0.6, -0.1]);
            assert_eq!(e.model.doomed_anchor(), Some(1));
            e.drag_to(&mut cx, [0.6, 0.0]);
            assert_eq!(e.model.doomed_anchor(), None, "back in the box, it is safe again");
            e.drag_to(&mut cx, [1.2, 0.5]);
            assert_eq!(e.model.doomed_anchor(), Some(1));
            e.release(&mut cx);
            assert_eq!(e.anchors(), &[FOUR[0], FOUR[3]]);
            let acts = reported(&mut cx, &e);
            assert_eq!(committed(&acts).map(Vec::len), Some(2));

            // An end dragged out stays.
            e.press(&mut cx, [1.0, 1.0], false);
            e.drag_to(&mut cx, [1.3, 1.3]);
            assert_eq!(e.model.doomed_anchor(), None);
            e.release(&mut cx);
            assert_eq!(e.anchors().len(), 2);
        });
    }

    /// `set_anchors` replaces the curve, tidied, and drops the selection.
    #[test]
    fn set_anchors_replaces_the_curve_and_clears_the_selection() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let mut e = editor(&mut cx, &FOUR);
            e.press(&mut cx, [0.3, 0.5], false);
            e.release(&mut cx);
            reported(&mut cx, &e);
            e.set_anchors(&mut cx, &[[1.0, 0.0, CORNER], [0.0, 1.0, CORNER]]);
            assert_eq!(e.anchors(), &[[0.0, 1.0, CORNER], [1.0, 0.0, CORNER]]);
            assert!(e.model.sels.is_empty());
            assert_eq!(e.tools, Some((false, false, None)));
            assert!(reported(&mut cx, &e).is_empty(), "a host's own curve is not an edit");
        });
    }

    /// Anchors crowded closer than the gap stay in order: one dragged
    /// between neighbours with no room for it sits halfway between them
    /// wherever the pointer goes, and a press in a gap with no room inserts
    /// nothing. A group dragged past an anchor that stays closes up against
    /// it rather than passing it.
    #[test]
    fn crowded_anchors_stay_in_order() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let crowded = [
                [0.0, 0.0, SMOOTH],
                [0.5, 0.2, SMOOTH],
                [0.503, 0.8, SMOOTH],
                [0.506, 0.2, SMOOTH],
                [1.0, 1.0, SMOOTH],
            ];
            let mut e = editor(&mut cx, &crowded);
            e.press(&mut cx, [0.503, 0.8], false);
            assert_eq!(e.model.sels, vec![2]);
            for n in [[0.9, 0.5], [0.1, 0.5], [0.5, 0.7], [1.3, -0.2], [-0.4, 1.3], [0.52, 0.6]] {
                e.drag_to(&mut cx, n);
                assert!(ordered(e.anchors()), "dragged to {n:?}: {:?}", e.anchors());
                assert!((e.anchors()[2][0] - 0.503).abs() < 1e-12, "{:?}", e.anchors()[2]);
            }
            e.release(&mut cx);
            assert_eq!(e.anchors().len(), 5);
            // The tangents see no span running backwards.
            let t = fixed_tangents(e.anchors());
            assert!(t.iter().all(|p| p.ox >= 0.0 && p.ix <= 0.0), "{t:?}");
            reported(&mut cx, &e);

            // Between 0.5 and 0.503 there is no room for another.
            let before = e.anchors().to_vec();
            assert!(!e.model.press([0.5015, 0.5], false));
            assert_eq!(e.anchors(), &before[..]);
            assert!(e.model.sels.is_empty() && e.model.drag.is_none());

            let mut e = editor(
                &mut cx,
                &[[0.0, 0.0, SMOOTH], [0.2, 0.5, SMOOTH], [0.3, 0.5, SMOOTH], [0.5, 0.5, SMOOTH], [1.0, 1.0, SMOOTH]],
            );
            e.press(&mut cx, [0.2, 0.5], false);
            e.release(&mut cx);
            e.press(&mut cx, [0.3, 0.5], true);
            assert_eq!(e.model.sels, vec![1, 2]);
            e.drag_to(&mut cx, [0.7, 0.5]);
            assert!(ordered(e.anchors()), "{:?}", e.anchors());
            assert!((e.anchors()[1][0] - 0.48).abs() < 1e-9, "{:?}", e.anchors());
            assert!((e.anchors()[2][0] - 0.49).abs() < 1e-9, "{:?}", e.anchors());
            assert_eq!(e.anchors()[3][0], 0.5, "the anchor that stays is where it was");
            e.release(&mut cx);
        });
    }

    /// A press on a drawn tangent handle selects that handle's anchor and
    /// does nothing else until handles drag: no insert, no drag, nothing
    /// reported. A selected anchor's own handle wins over the anchor where
    /// the press is nearer it, and an unselected anchor's does not.
    #[test]
    fn a_press_on_a_handle_selects_its_anchor_and_inserts_nothing() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let mut e = editor(&mut cx, &DEFAULT_LINE);
            // The out-handle of anchor 0: a sixth of the way along the line.
            let h = [1.0 / 6.0, 5.0 / 6.0];
            let t = fixed_tangents(&DEFAULT_LINE);
            assert!(near([t[0].x + t[0].ox, t[0].y + t[0].oy], h), "{:?}", t[0]);
            e.press(&mut cx, h, false);
            assert_eq!(e.anchors(), &DEFAULT_LINE[..]);
            assert_eq!(e.model.sels, vec![0]);
            assert!(e.model.drag.is_none());
            e.drag_to(&mut cx, [0.5, 0.5]);
            e.release(&mut cx);
            assert_eq!(e.anchors(), &DEFAULT_LINE[..]);
            assert!(reported(&mut cx, &e).is_empty());
            // Pressed again, it keeps the selection it found.
            e.press(&mut cx, h, true);
            assert_eq!(e.model.sels, vec![0]);
            // Just out of the handle's reach is empty canvas again.
            e.press(&mut cx, [h[0] + 0.05, h[1]], false);
            assert_eq!(e.anchors().len(), 3, "an insert");
            e.release(&mut cx);

            // Anchor 0 in reach (0.042 away) and its out-handle, at
            // (1/30, 29/30), nearer still.
            let mut e = editor(&mut cx, &[[0.0, 1.0, SMOOTH], [0.2, 0.8, SMOOTH], [1.0, 0.0, SMOOTH]]);
            let at = [0.03, 0.97];
            e.press(&mut cx, at, false);
            assert!(e.model.drag.is_some(), "an unselected anchor's handle does not beat the anchor");
            e.release(&mut cx);
            e.press(&mut cx, at, false);
            assert_eq!(e.model.sels, vec![0]);
            assert!(e.model.drag.is_none(), "the selected anchor's own handle does");
            assert_eq!(e.anchors().len(), 3);
        });
    }

    // -- The canvas, drawn and pressed ----------------------------------------

    const PANE: DVec2 = dvec2(600.0, 400.0);
    const WINDOW: WindowId = WindowId(1, 1);

    fn draw(cx: &mut Cx, root: &WidgetRef, pass: &DrawPass, draw_list: &mut DrawList2d) {
        pass.set_size(cx, PANE);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(PANE, Layout::flow_down());
        root.draw_all(&mut cx2d, &mut Scope::empty());
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(pass);
    }

    fn mouse_down(abs: DVec2) -> Event {
        mouse_down_with(abs, KeyModifiers::default())
    }

    fn mouse_down_with(abs: DVec2, modifiers: KeyModifiers) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers,
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn mouse_move(abs: DVec2) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: DVec2::default(),
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.1,
        })
    }

    fn mouse_up(abs: DVec2) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WINDOW,
            modifiers: KeyModifiers::default(),
            time: 0.2,
        })
    }

    /// The whole road: the editor drawn in a view, the canvas where the
    /// `canvas` child was laid out, and real pointer events. A press on the
    /// empty canvas inserts and takes the pointer; the drag follows it out
    /// of the canvas altogether, and the release there deletes the anchor
    /// the press made.
    #[test]
    fn a_press_takes_the_pointer_and_a_drag_out_of_the_canvas_deletes() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let root = cx.with_vm(|vm| {
                let value = crate::script_eval!(vm, {
                    use mod.prelude.widgets.*
                    use mod.widgets.*
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        editor := CurveEditor{
                            width: 400
                            anchors: [[0, 0, 1], [1, 1, 1]]
                            left_label: "ROOT"
                            right_label: "TIP"
                            guide: 0.5
                            guide_label: "CAP TOP"
                            mirror: true
                        }
                    }
                });
                WidgetRef::script_from_value(vm, value)
            });
            let pass = DrawPass::new(&mut cx);
            let mut draw_list = DrawList2d::new(&mut cx);
            draw(&mut cx, &root, &pass, &mut draw_list);

            let editor = root.curve_editor(&cx, ids!(editor));
            let (canvas, frame) = {
                let inner = editor.borrow().expect("a CurveEditor");
                let r = inner.draw_canvas.area().rect(&cx);
                (inner.draw_canvas.area(), Frame::new(r))
            };
            assert_eq!(frame.size, dvec2(400.0, 220.0), "the canvas is the `canvas` child's place");
            assert!(!editor.borrow().unwrap().poly.is_empty(), "the curve was drawn");

            cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
            let at = frame.to_px(0.5, 0.2);
            let actions = cx.capture_actions(|cx| root.handle_event(cx, &mouse_down(at), &mut Scope::empty()));
            let inserted = editor.changed(&actions).expect("the press inserted");
            assert_eq!(inserted.len(), 3);
            assert!((inserted[1][0] - 0.5).abs() < 1e-9 && (inserted[1][1] - 0.2).abs() < 1e-9, "{:?}", inserted[1]);
            assert!(cx.fingers.is_area_captured(canvas), "the canvas holds the pointer");

            let past = frame.pos + dvec2(frame.size.x * 0.5, frame.size.y + 80.0);
            let actions = cx.capture_actions(|cx| root.handle_event(cx, &mouse_move(past), &mut Scope::empty()));
            assert!(editor.changed(&actions).is_some(), "the drag followed the pointer out");
            assert_eq!(editor.borrow().unwrap().model.doomed_anchor(), Some(1));
            draw(&mut cx, &root, &pass, &mut draw_list);

            let actions = cx.capture_actions(|cx| root.handle_event(cx, &mouse_up(past), &mut Scope::empty()));
            cx.fingers.first_mouse_button = None;
            assert_eq!(editor.committed(&actions), Some(vec![[0.0, 0.0, SMOOTH], [1.0, 1.0, SMOOTH]]));
            assert_eq!(editor.anchors().len(), 2);
            draw(&mut cx, &root, &pass, &mut draw_list);
        });
    }

    /// Everything the canvas draws stays on it. The default line's last
    /// tangent arm runs a sixth of the span past the box's corner, well
    /// out of the canvas, with room round the editor for it to spill into:
    /// it is cut at the canvas's edge.
    #[test]
    fn the_canvas_clips_what_it_draws() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let root = cx.with_vm(|vm| {
                let value = crate::script_eval!(vm, {
                    use mod.prelude.widgets.*
                    use mod.widgets.*
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        padding: 60
                        editor := CurveEditor{width: 400}
                    }
                });
                WidgetRef::script_from_value(vm, value)
            });
            let pass = DrawPass::new(&mut cx);
            let mut draw_list = DrawList2d::new(&mut cx);
            draw(&mut cx, &root, &pass, &mut draw_list);

            let editor = root.curve_editor(&cx, ids!(editor));
            let e = editor.borrow().expect("a CurveEditor");
            let canvas = e.draw_canvas.area().rect(&cx);
            assert_eq!(canvas.pos, dvec2(60.0, 60.0));
            let end = |r: Rect| r.pos + r.size;
            let arm = e.draw_arm.area();
            let (whole, shown) = (arm.rect(&cx), arm.clipped_rect(&cx));
            assert!(end(whole).x > end(canvas).x + 30.0 && end(whole).y > end(canvas).y + 10.0, "{whole:?} past {canvas:?}");
            assert!(shown.size.x > 0.0 && shown.size.y > 0.0, "its part on the canvas is drawn: {shown:?}");
            assert!(shown.pos.x >= canvas.pos.x && shown.pos.y >= canvas.pos.y, "{shown:?} in {canvas:?}");
            assert!(end(shown).x <= end(canvas).x + 1e-3 && end(shown).y <= end(canvas).y + 1e-3, "{shown:?} in {canvas:?}");
        });
    }

    /// A press and a release at `abs` through the root, `modifiers` held
    /// on the press: everything reported. The capture the press took is let
    /// go by hand, as the platform does after a release, or it would take
    /// every later press.
    fn click(cx: &mut Cx, root: &WidgetRef, abs: DVec2, modifiers: KeyModifiers) -> ActionsBuf {
        cx.fingers.first_mouse_button = Some((MouseButton::PRIMARY, WINDOW));
        let down = mouse_down_with(abs, modifiers);
        let mut actions = cx.capture_actions(|cx| root.handle_event(cx, &down, &mut Scope::empty()));
        actions.extend(cx.capture_actions(|cx| root.handle_event(cx, &mouse_up(abs), &mut Scope::empty())));
        if let Event::MouseDown(e) = &down {
            down.unhandle(cx, &e.handled.get());
        }
        cx.fingers.first_mouse_button = None;
        actions
    }

    /// The kind row, pressed for real: dead with nothing selected, it lights
    /// the kind every selected anchor shares and none when they differ; a
    /// rung sets the selection's kind, and Horizontal pressed again while
    /// it is lit still levels the selection.
    #[test]
    fn the_kind_row_lights_the_shared_kind_and_a_rung_sets_it() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            let root = cx.with_vm(|vm| {
                let value = crate::script_eval!(vm, {
                    use mod.prelude.widgets.*
                    use mod.widgets.*
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        editor := CurveEditor{
                            width: 500
                            anchors: [[0, 0, 1], [0.5, 0.5, 2], [1, 1, 1]]
                        }
                    }
                });
                WidgetRef::script_from_value(vm, value)
            });
            let pass = DrawPass::new(&mut cx);
            let mut draw_list = DrawList2d::new(&mut cx);
            draw(&mut cx, &root, &pass, &mut draw_list);

            let editor = root.curve_editor(&cx, ids!(editor));
            let row = |cx: &Cx| toolbar(cx, &editor.borrow().unwrap());
            let frame = Frame::new(editor.borrow().unwrap().draw_canvas.area().rect(&cx));
            let rung = |cx: &Cx, id: &[LiveId]| {
                let r = editor.borrow().unwrap().view.radio_button(cx, id).area().rect(cx);
                assert!(r.size.x > 0.0 && r.size.y > 0.0, "rung {id:?} drawn");
                r.pos + r.size * 0.5
            };
            let shift = KeyModifiers { shift: true, ..Default::default() };
            assert_eq!(row(&cx), (None, false, false), "nothing selected: the row is dead");
            let horizontal = rung(&cx, ids!(horizontal));
            let actions = click(&mut cx, &root, horizontal, KeyModifiers::default());
            assert_eq!(editor.changed(&actions), None, "a dead rung sets nothing");

            click(&mut cx, &root, frame.to_px(0.5, 0.5), KeyModifiers::default());
            assert_eq!(row(&cx), (Some(CORNER), true, true));
            draw(&mut cx, &root, &pass, &mut draw_list);

            let actions = click(&mut cx, &root, horizontal, KeyModifiers::default());
            assert_eq!(editor.anchors()[1], [0.5, 0.5, HORIZONTAL]);
            assert_eq!(editor.committed(&actions).map(|a| a[1][2]), Some(HORIZONTAL));
            assert_eq!(row(&cx), (Some(HORIZONTAL), true, true));
            draw(&mut cx, &root, &pass, &mut draw_list);

            // A smooth end joins the horizontal anchor: two kinds, no rung.
            click(&mut cx, &root, frame.to_px(0.0, 0.0), shift);
            assert_eq!(editor.borrow().unwrap().model.sels, vec![1, 0]);
            assert_eq!(row(&cx), (None, true, true));
            draw(&mut cx, &root, &pass, &mut draw_list);

            // Horizontal on both levels them at the primary's height.
            let actions = click(&mut cx, &root, horizontal, KeyModifiers::default());
            assert!(editor.committed(&actions).is_some());
            assert_eq!(editor.anchors()[..2], [[0.0, 0.0, HORIZONTAL], [0.5, 0.0, HORIZONTAL]]);
            assert_eq!(row(&cx), (Some(HORIZONTAL), true, true));
            draw(&mut cx, &root, &pass, &mut draw_list);

            // Lit, and pressed again with the pair apart: it levels them.
            editor.borrow_mut().unwrap().model.anchors[1][1] = 0.4;
            let actions = click(&mut cx, &root, horizontal, KeyModifiers::default());
            assert_eq!(editor.committed(&actions).map(|a| a[1][1]), Some(0.0));
            assert_eq!(row(&cx), (Some(HORIZONTAL), true, true), "the rung stays lit");
            // And again with nothing to level: nothing to report, still lit.
            let actions = click(&mut cx, &root, horizontal, KeyModifiers::default());
            assert_eq!(editor.changed(&actions), None);
            assert_eq!(row(&cx), (Some(HORIZONTAL), true, true));

            let corner = rung(&cx, ids!(corner));
            click(&mut cx, &root, corner, KeyModifiers::default());
            assert_eq!(editor.anchors()[..2], [[0.0, 0.0, CORNER], [0.5, 0.0, CORNER]]);
            assert_eq!(row(&cx), (Some(CORNER), true, true));
            draw(&mut cx, &root, &pass, &mut draw_list);
        });
    }

    /// Every shader the editor draws with compiles: a shader error would
    /// otherwise only show as a blank canvas at run time.
    #[test]
    fn the_editor_shaders_compile() {
        crate::on_test_cx(|| {
            let mut cx = test_cx();
            cx.with_vm(|vm| {
                // Each with a name only its own pixel function reads, so a
                // shader that fell back to a default does not pass.
                for (name, own, value) in [
                    ("draw_canvas", "grid_color", crate::script_eval!(vm, {
                        mod.shader.test_compile_draw_source(mod.widgets.CurveEditor.draw_canvas, "glsl", false)
                    })),
                    ("draw_fill", "opacity", crate::script_eval!(vm, {
                        mod.shader.test_compile_draw_source(mod.widgets.CurveEditor.draw_fill, "glsl", false)
                    })),
                    ("draw_mirror", "fade", crate::script_eval!(vm, {
                        mod.shader.test_compile_draw_source(mod.widgets.CurveEditor.draw_mirror, "glsl", false)
                    })),
                    ("draw_guide", "dash", crate::script_eval!(vm, {
                        mod.shader.test_compile_draw_source(mod.widgets.CurveEditor.draw_guide, "glsl", false)
                    })),
                    ("draw_knob", "doomed_color", crate::script_eval!(vm, {
                        mod.shader.test_compile_draw_source(mod.widgets.CurveEditor.draw_knob, "glsl", false)
                    })),
                ] {
                    let text = vm
                        .bx
                        .heap
                        .string_with(value, |_heap, text| text.to_string())
                        .expect("the compiler answers with source");
                    assert!(!text.starts_with("ERRORS:"), "{name} did not compile: {text}");
                    assert!(text.contains(own), "{name} is not the editor's own shader");
                }
            });
        });
    }
}
