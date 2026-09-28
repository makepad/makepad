//! Sequencer: a timeline editor for any timeline data (the ImSequencer /
//! GSDevTools idea, rebuilt for Makepad).
//!
//! The widget shows a [`SequencerModel`] (plain data: tracks of items on a
//! time axis, plus labels) as rows of bars under a time ruler, with a name
//! column on the left:
//!
//! * a bar per item: its delay drawn hatched before it, one segment per
//!   iteration (`repeat`, `-1` forever up to the visible edge), a yoyo pass
//!   with its direction ramp mirrored, a hatched connector across each
//!   `repeat_delay` gap; a zero-duration item is a diamond marker;
//! * a ruler with ticks that stay at least 64 px apart at any zoom, the
//!   model's labels as named flags, and a draggable playhead;
//! * foldable rows: a track with children folds them away.
//!
//! Input (the widget emits [`SequencerAction`]s and edits ITS copy of the
//! model; the host applies the edits to whatever the model came from):
//!
//! * press or drag on the ruler to scrub ([`SequencerAction::ScrubStart`],
//!   [`SequencerAction::Scrub`], [`SequencerAction::ScrubEnd`]); Home / End
//!   scrub to either end;
//! * drag a bar to move it, an edge to resize it: snapped (within 6 px) to
//!   0, the end, the playhead, the labels and the start and end of every
//!   item outside the dragged one's subtree, all taken when the drag starts
//!   (so a parent or last bar never snaps to itself), Alt held to place
//!   freely, Escape to put it back; the bar itself rides the pointer with a
//!   time tag, and a guide line shows the snap;
//! * drag a label flag to move the label;
//! * Left / Right nudge the selected item by one minor tick (Shift: one
//!   major tick);
//! * primary modifier (Ctrl, Cmd on macOS) + wheel zooms about the pointer,
//!   Shift + wheel, a horizontal wheel or a drag on empty space pans; the
//!   "-", "+" and "Fit" buttons in the corner zoom too. A plain vertical
//!   wheel is left to the parent (the widget has no vertical scroll of its
//!   own: its height fits its visible rows; put a big model in a ScrollView).
//!
//! The host recipe for a `makepad_tween` timeline (see the storybook's
//! Foundations / Motion / Sequencer page):
//!
//! ```text
//! // after building: seq.set_model(cx, SequencerModel::from_engine(&host.engine, tl, name_of));
//! for a in seq.actions(actions) {
//!     match a {
//!         SequencerAction::Scrub(t) => { host.control(cx, tl).seek(Seek::Time(t), Emit::Suppress); }
//!         SequencerAction::ItemMoved { .. } | SequencerAction::ItemResized { .. }
//!         | SequencerAction::LabelMoved { .. } => {
//!             if apply_sequencer_edit(&mut host.engine, tl, a) {
//!                 seq.set_model(cx, SequencerModel::from_engine(&host.engine, tl, name_of));
//!             }
//!         }
//!         _ => {}
//!     }
//! }
//! // every changed frame: seq.set_playhead(cx, host.engine.anim_ref(tl).total_time());
//! ```
//!
//! Cost: rows are laid out only when the model changes (a new generation)
//! or a fold toggles; drawing allocates nothing once its buffers have grown;
//! [`SequencerRef::set_playhead`] moves the playhead quad in place
//! (`update_abs`: no redraw, no layout) unless it pans the view.
//!
//! The model ([`SequencerModel`] and its parts), the engine mapping
//! ([`SequencerModel::from_engine`], [`apply_sequencer_edit`]) and the drag
//! maths ([`SequencerModel::item_snap_targets`], [`SequencerModel::drag_move`],
//! [`snap_time`], ...) are plain data and pure functions in
//! `makepad_tween::sequencer_model`, re-exported here.
//!
//! Limits: model time is the root's local time (the first iteration of
//! every nested animation is placed; later iterations are drawn on the
//! parent's bar; a child of a reversed timeline is placed where it plays,
//! mirrored, and its bar's ramp runs backwards); nested timelines' own
//! labels are not shown; the children of stagger and keyframes groups are
//! read-only rows (moving one would desync the group's cached content
//! duration), and keyframes steps under a non-linear group ease are placed
//! linearly.
use crate::{badge::measure, makepad_derive_widget::*, makepad_draw::*, widget::*};
use std::fmt::Write;

/// The model, its engine mapping and the drag maths live in the tween
/// engine (`makepad_tween::sequencer_model`, plain data and pure functions);
/// they are re-exported here for hosts of the widget.
pub use crate::tween::{
    apply_sequencer_edit, snap_nearest, snap_time, SequencerAction, SequencerDrag, SequencerItem,
    SequencerKind, SequencerLabel, SequencerModel, SequencerTrack,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawSeqGrid::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    set_type_default() do #(DrawSeqBar::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.SequencerBase = #(Sequencer::register_widget(vm))

    /** A timeline editor: tracks of bars under a time ruler, labels, a draggable playhead, zoom and pan. */
    mod.widgets.Sequencer = set_type_default() do mod.widgets.SequencerBase{
        width: Fill
        height: Fit
        /** width of the name column in pixels 60..400 step 1 */
        name_width: 160.0
        /** height of the time ruler in pixels 16..48 step 1 */
        ruler_height: 26.0
        /** height of one row in pixels 14..48 step 1 */
        row_height: 22.0
        /** indent per nesting level in pixels 0..40 step 1 */
        indent: 14.0
        /** how close an edge snaps, pixels 0..24 step 1 */
        snap_px: 6.0
        /** how far from a bar end a press resizes, pixels 2..16 step 1 */
        edge_px: 5.0
        /** lowest zoom in pixels per second 1..100 step 1 */
        min_zoom: 5.0
        /** highest zoom in pixels per second 100..20000 step 100 */
        max_zoom: 5000.0
        /** pan to keep the playhead in view while it moves */
        follow: true
        color_tween: theme.color_primary
        color_timeline: theme.color_secondary
        color_group: theme.color_tertiary
        color_marker: theme.color_warning
        color_caption: theme.color_on_primary
        color_dim: theme.color_on_surface_variant

        draw_bg +: {
            color: uniform(theme.color_inset)
            name_color: uniform(theme.color_surface_container_low)
            ruler_color: uniform(theme.color_surface_container)
            stripe_color: uniform(theme.color_outline_variant)
            select_color: uniform(theme.color_selection_focus)
            hover_color: uniform(theme.color_highlight)
            tick_color: uniform(theme.color_on_surface_variant)
            grid_color: uniform(theme.color_outline_variant)
            line_color: uniform(theme.color_outline)
            pixel: fn() {
                let p = self.pos * self.rect_size
                var c = self.color
                let ty = p.y - self.ruler_h
                let row = floor(ty / max(self.row_h, 1.0))
                let in_rows = step(0.0, ty) * step(row, self.rows - 0.5)
                if in_rows > 0.5 && modf(row, 2.0) > 0.5 {
                    c = mix(c, vec4(self.stripe_color.rgb, 1.0), 0.16)
                }
                if p.x < self.name_w {
                    c = self.name_color
                }
                if in_rows > 0.5 && abs(row - self.hover_row) < 0.5 {
                    c = mix(c, vec4(self.hover_color.rgb, 1.0), self.hover_color.a)
                }
                if in_rows > 0.5 && abs(row - self.sel_row) < 0.5 {
                    c = mix(c, vec4(self.select_color.rgb, 1.0), self.select_color.a)
                }
                if ty < 0.0 {
                    c = self.ruler_color
                }
                if p.x >= self.name_w {
                    // Major ticks every tick_px from tick_x0, minors in between.
                    let tp = max(self.tick_px, 1.0)
                    let u = (p.x - self.name_w - self.tick_x0) / tp
                    let dm = abs(fract(u + 0.5) - 0.5) * tp
                    let major = 1.0 - clamp(dm - 0.25, 0.0, 1.0)
                    if ty < 0.0 {
                        let mt = tp / max(self.minor_div, 1.0)
                        let v = (p.x - self.name_w - self.tick_x0) / mt
                        let dn = abs(fract(v + 0.5) - 0.5) * mt
                        let minor = (1.0 - clamp(dn - 0.25, 0.0, 1.0)) * step(1.5, self.minor_div)
                        let tall = major * step(self.ruler_h * 0.4, p.y)
                        let short = minor * step(self.ruler_h * 0.7, p.y)
                        c = mix(c, vec4(self.tick_color.rgb, 1.0), max(tall, short) * self.tick_color.a)
                    } else {
                        c = mix(c, vec4(self.grid_color.rgb, 1.0), major * in_rows * 0.5 * self.grid_color.a)
                    }
                }
                let sx = 1.0 - clamp(abs(p.x - self.name_w + 0.5) - 0.25, 0.0, 1.0)
                let sy = 1.0 - clamp(abs(p.y - self.ruler_h + 0.5) - 0.25, 0.0, 1.0)
                c = mix(c, vec4(self.line_color.rgb, 1.0), max(sx, sy) * self.line_color.a)
                return vec4(c.rgb * c.a, c.a)
            }
        }
        draw_bar +: {
            outline_color: uniform(theme.color_text)
            grip_color: uniform(theme.color_on_primary)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let sdf = Sdf2d.viewport(p)
                let r = min(4.0, self.rect_size.y * 0.5)
                sdf.box(0.5, 0.5, max(self.rect_size.x - 1.0, 0.5), self.rect_size.y - 1.0, r)
                // Brighter towards the end of the pass: a yoyo pass mirrors it.
                let ramp = mix(self.pos.x, 1.0 - self.pos.x, self.mirror)
                let lift = 0.7 + 0.3 * ramp + 0.12 * step(0.5, self.state)
                sdf.fill_keep(vec4(self.color.rgb * lift, self.color.a))
                sdf.stroke(self.outline_color * step(1.5, self.state), 1.5)
                // Edge grips on an editable bar (open) that is hovered or held.
                let g = step(0.5, self.state) * step(0.5, self.open)
                let gy = step(self.rect_size.y * 0.3, p.y) * step(p.y, self.rect_size.y * 0.7)
                let gl = 1.0 - clamp(abs(p.x - 3.5) - 0.5, 0.0, 1.0)
                let gr = 1.0 - clamp(abs(p.x - self.rect_size.x + 3.5) - 0.5, 0.0, 1.0)
                let a = g * gy * max(gl, gr) * self.grip_color.a
                return mix(sdf.result, vec4(self.grip_color.rgb, 1.0) * sdf.result.a, a)
            }
        }
        draw_delay +: {
            pixel: fn() {
                let p = self.pos * self.rect_size
                let h = step(0.5, fract((p.x + p.y) / 6.0))
                let a = self.color.a * (0.12 + 0.23 * h)
                return vec4(self.color.rgb * a, a)
            }
        }
        draw_marker +: {
            outline_color: uniform(theme.color_text)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let r = min(c.x, c.y) - 0.5
                let d = abs(p.x - c.x) + abs(p.y - c.y) - r
                let fill = 1.0 - clamp(d + 0.5, 0.0, 1.0)
                let ring = step(1.5, self.state) * (1.0 - clamp(abs(d + 1.25) - 0.5, 0.0, 1.0))
                let lift = 1.0 + 0.15 * step(0.5, self.state)
                let col = mix(vec4(self.color.rgb * lift, self.color.a), self.outline_color, ring)
                let a = col.a * fill
                return vec4(col.rgb * a, a)
            }
        }
        draw_fold +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                sdf.rotate(self.open * 1.5707963, c.x, c.y)
                sdf.move_to(c.x - 2.5, c.y - 4.0)
                sdf.line_to(c.x + 3.5, c.y)
                sdf.line_to(c.x - 2.5, c.y + 4.0)
                sdf.close_path()
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_head +: {
            color: theme.color_error
            pixel: fn() {
                let p = self.pos * self.rect_size
                let mid = self.rect_size.x * 0.5
                let sdf = Sdf2d.viewport(p)
                sdf.move_to(0.5, 0.5)
                sdf.line_to(self.rect_size.x - 0.5, 0.5)
                sdf.line_to(mid, self.head)
                sdf.close_path()
                sdf.fill(self.color)
                let line = (1.0 - clamp(abs(p.x - mid) - 0.75, 0.0, 1.0)) * self.color.a
                let a = max(sdf.result.a, line)
                return vec4(self.color.rgb * a, a)
            }
        }
        draw_flag +: {
            snap_color: uniform(theme.color_warning)
            pixel: fn() {
                let p = self.pos * self.rect_size
                // The line runs down the left edge: dashed for a label, solid
                // for the snap guide (state 3); the flag is the top `head` px.
                let solid = step(2.5, self.state)
                let dash = mix(step(0.45, fract(p.y / 6.0)), 1.0, solid)
                let line = (1.0 - clamp(abs(p.x - 1.0) - 0.5, 0.0, 1.0)) * dash
                let flag = step(p.y, self.head) * step(0.5, self.head)
                let col = mix(self.color, self.snap_color, solid)
                let hot = 0.7 + 0.3 * step(0.5, self.state) * (1.0 - solid)
                let a = max(line * col.a * 0.8, flag * col.a * hot)
                return vec4(col.rgb * a, a)
            }
        }
        draw_button +: {
            color: theme.color_surface_container_high
            hover_color: uniform(theme.color_highlight)
            border_color: uniform(theme.color_outline_variant)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 3.0)
                sdf.fill_keep(mix(self.color, vec4(self.hover_color.rgb, 1.0), step(0.5, self.state) * 0.5))
                sdf.stroke(self.border_color, 1.0)
                return sdf.result
            }
        }
        draw_tip +: {
            color: theme.color_surface_container_high
            border_color: uniform(theme.color_outline)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 4.0)
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, 1.0)
                return sdf.result
            }
        }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_code{font_size: theme.font_size_p}
        }
    }
}

// ---------------------------------------------------------------------------
// Shader structs
// ---------------------------------------------------------------------------

/// The whole widget in one quad: the name column, the ruler with its ticks,
/// row stripes, the selected and hovered rows, the grid and the separators.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSeqGrid {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub name_w: f32,
    #[live]
    pub ruler_h: f32,
    #[live]
    pub row_h: f32,
    /// Visible rows.
    #[live]
    pub rows: f32,
    /// The first major tick's x from the track area's left edge.
    #[live]
    pub tick_x0: f32,
    /// Pixels between major ticks.
    #[live]
    pub tick_px: f32,
    /// Minor ticks per major (1: none).
    #[live]
    pub minor_div: f32,
    /// The selected row (-1: none).
    #[live]
    pub sel_row: f32,
    /// The hovered row (-1: none).
    #[live]
    pub hover_row: f32,
}

/// Bars, delays, markers, fold arrows, the playhead, label flags, buttons
/// and the tooltip box: one struct, each field with its own pixel function.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSeqBar {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    /// A yoyo pass: the direction ramp runs backwards.
    #[live]
    pub mirror: f32,
    /// 0 idle, 1 hover, 2 selected, 3 dragging (a flag: 3 = the snap guide).
    #[live]
    pub state: f32,
    /// A fold arrow: 1 open. A bar: 1 editable (edge grips shown).
    #[live]
    pub open: f32,
    /// The playhead's triangle height, a label flag's height.
    #[live]
    pub head: f32,
}

// ---------------------------------------------------------------------------
// Layout and view math
// ---------------------------------------------------------------------------

/// (major, minor, decimals): the first whose major step is at least
/// [`TICK_MIN_PX`] at the zoom wins, else the last.
const TICKS: [(f64, f64, usize); 14] = [
    (0.01, 0.002, 2),
    (0.02, 0.005, 2),
    (0.05, 0.01, 2),
    (0.1, 0.02, 1),
    (0.2, 0.05, 1),
    (0.25, 0.05, 2),
    (0.5, 0.1, 1),
    (1.0, 0.2, 0),
    (2.0, 0.5, 0),
    (5.0, 1.0, 0),
    (10.0, 2.0, 0),
    (15.0, 5.0, 0),
    (30.0, 5.0, 0),
    (60.0, 10.0, 0),
];
const TICK_MIN_PX: f64 = 64.0;
const MINOR_MIN_PX: f64 = 6.0;
/// Iteration segments drawn per item at most.
const MAX_SEGMENTS: i64 = 512;
/// The shortest duration a resize leaves.
const MIN_DURATION: f64 = 0.01;
/// A press that moves less than this is a tap.
const TAP_SLOP: f64 = 3.0;
/// Label flags: a press this close to the line takes the label.
const FLAG_GRAB: f64 = 5.0;
const FLAG_H: f64 = 12.0;
const HEAD_W: f64 = 11.0;
const HEAD_H: f64 = 8.0;
const BUTTON_H: f64 = 18.0;
const BUTTONS: [(&str, f64); 3] = [("-", 20.0), ("+", 20.0), ("Fit", 28.0)];
const ZOOM_STEP: f64 = 1.5;

fn ticks_for(zoom: f64) -> (f64, f64, usize) {
    TICKS
        .iter()
        .copied()
        .find(|t| t.0 * zoom >= TICK_MIN_PX)
        .unwrap_or(TICKS[TICKS.len() - 1])
}

/// Time to pixels and back, shared by drawing and hit testing: what is drawn
/// is what is grabbable. Pixels are local to the widget's rect.
#[derive(Clone, Copy, Debug, Default)]
struct SeqView {
    /// The track area's left edge (the name column's width).
    x0: f64,
    /// The track area's width.
    w: f64,
    /// Pixels per second.
    zoom: f64,
    /// The time at `x0`.
    offset: f64,
}

impl SeqView {
    fn x_at(&self, t: f64) -> f64 {
        self.x0 + (t - self.offset) * self.zoom
    }

    fn t_at(&self, x: f64) -> f64 {
        self.offset + (x - self.x0) / self.zoom
    }

    fn t_end(&self) -> f64 {
        self.offset + self.w / self.zoom
    }
}

/// A visible row: a track shown because every ancestor is open.
#[derive(Clone, Copy, Debug, Default)]
struct Row {
    track: u32,
    depth: u32,
    openable: bool,
    open: bool,
}

/// An item's first iteration as drawn (local pixels): the hit test.
#[derive(Clone, Copy, Debug, Default)]
struct BarHit {
    id: u64,
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
    marker: bool,
    locked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    Body,
    Left,
    Right,
}

/// What is under the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Hover {
    #[default]
    None,
    Item(u64, Zone),
    Label(u64),
    Ruler,
    Fold(u32),
    Name(u32),
    Button(u8),
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragKind {
    Move,
    ResizeLeft,
    ResizeRight,
    Label,
    Scrub,
    Pan,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    kind: DragKind,
    /// The item or label.
    id: u64,
    /// The pointer's time minus the grabbed edge's time at the press.
    grab_dt: f64,
    /// What Escape puts back.
    start0: f64,
    dur0: f64,
    time0: f64,
    offset0: f64,
    press: DVec2,
    moved: bool,
    edited: bool,
}

/// Lays out the visible rows of `tracks` (`open` overrides `expanded`).
fn lay_rows(rows: &mut Vec<Row>, tracks: &[SequencerTrack], open: &[(u64, bool)]) {
    rows.clear();
    walk_rows(tracks, open, |r| rows.push(r));
}

/// Calls `f` with every visible row of `tracks`, in order (`open`
/// overrides `expanded`). Allocation-free.
fn walk_rows(tracks: &[SequencerTrack], open: &[(u64, bool)], mut f: impl FnMut(Row)) {
    let mut hide: Option<u32> = None;
    for (i, t) in tracks.iter().enumerate() {
        if let Some(d) = hide {
            if t.depth > d {
                continue;
            }
            hide = None;
        }
        let openable = t.has_children || tracks.get(i + 1).is_some_and(|n| n.depth > t.depth);
        let is_open = !openable
            || open
                .iter()
                .find(|(id, _)| *id == t.id)
                .map_or(t.expanded, |(_, o)| *o);
        f(Row {
            track: i as u32,
            depth: t.depth,
            openable,
            open: is_open,
        });
        if !is_open {
            hide = Some(t.depth);
        }
    }
}

/// `t` for printing: no "-0.000".
fn clean(t: f64) -> f64 {
    if t.abs() < 1e-9 {
        0.0
    } else {
        t + 0.0
    }
}

// ---------------------------------------------------------------------------
// The widget
// ---------------------------------------------------------------------------

/// A timeline editor over a [`SequencerModel`] (see the module docs).
#[derive(Script, ScriptHook, Widget)]
pub struct Sequencer {
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
    draw_bg: DrawSeqGrid,
    #[live]
    draw_bar: DrawSeqBar,
    #[live]
    draw_delay: DrawSeqBar,
    #[live]
    draw_marker: DrawSeqBar,
    #[live]
    draw_fold: DrawSeqBar,
    #[live]
    draw_head: DrawSeqBar,
    #[live]
    draw_flag: DrawSeqBar,
    #[live]
    draw_button: DrawSeqBar,
    #[live]
    draw_tip: DrawSeqBar,
    #[live]
    draw_text: DrawText,

    #[live(160.0)]
    pub name_width: f64,
    #[live(26.0)]
    pub ruler_height: f64,
    #[live(22.0)]
    pub row_height: f64,
    #[live(14.0)]
    pub indent: f64,
    #[live(6.0)]
    pub snap_px: f64,
    #[live(5.0)]
    pub edge_px: f64,
    #[live(5.0)]
    pub min_zoom: f64,
    #[live(5000.0)]
    pub max_zoom: f64,
    #[live(true)]
    pub follow: bool,
    #[live]
    color_tween: Vec4f,
    #[live]
    color_timeline: Vec4f,
    #[live]
    color_group: Vec4f,
    #[live]
    color_marker: Vec4f,
    /// Captions on bars.
    #[live]
    color_caption: Vec4f,
    /// Tick labels and fold arrows.
    #[live]
    color_dim: Vec4f,

    #[rust]
    model: SequencerModel,
    /// Bumped by every `set_model`.
    #[rust]
    generation: u64,
    /// `rows` no longer match the model or the folds (set by `set_model`
    /// and every fold, cleared by the layout).
    #[rust]
    rows_dirty: bool,
    #[rust]
    rows: Vec<Row>,
    #[rust]
    hits: Vec<BarHit>,
    /// Label flags as drawn: (label index, x0, x1) in local pixels.
    #[rust]
    flags: Vec<(u32, f64, f64)>,
    /// Fold state per track id (overrides the model's `expanded`).
    #[rust]
    open: Vec<(u64, bool)>,
    #[rust]
    view: SeqView,
    /// The view was fitted to a model (the first one, or Fit).
    #[rust]
    fitted: bool,
    #[rust]
    playhead: f64,
    #[rust]
    selected: Option<u64>,
    #[rust]
    hover: Hover,
    #[rust]
    drag: Option<Drag>,
    /// The time a drag snapped to (drawn as a guide line).
    #[rust]
    snap_guide: Option<f64>,
    /// What the current drag snaps to, taken once at the press
    /// ([`SequencerModel::item_snap_targets`]).
    #[rust]
    snap_targets: Vec<f64>,
    /// The corner buttons, local pixels.
    #[rust]
    buttons: [Rect; 3],
    /// The playhead quad as drawn (absolute).
    #[rust]
    head_rect: Rect,
    /// The pointer, local pixels.
    #[rust]
    pointer: DVec2,
    /// The tooltip of the hovered item (rebuilt when the item changes).
    #[rust]
    tip: String,
    #[rust]
    tip_for: Option<u64>,
    /// Scratch text: tick labels, the drag tag.
    #[rust]
    num: String,
    /// The (generation, font size) the text widths below were measured at:
    /// measuring allocates, so it happens once per model, not per draw.
    #[rust]
    widths_for: Option<(u64, f32)>,
    /// Per track, the index of its first item in `cap_w`.
    #[rust]
    track_base: Vec<u32>,
    /// Every item's caption width (0 when it has none), in model order.
    #[rust]
    cap_w: Vec<f64>,
    /// Every label name's width.
    #[rust]
    label_w: Vec<f64>,
    #[rust]
    button_w: [f64; 3],
    /// The tooltip's width (measured once per tooltip text).
    #[rust]
    tip_w: Option<f64>,
}

impl Sequencer {
    // ---- model -------------------------------------------------------------

    /// Shows `m`. The view (zoom, offset) is kept (a first model is fitted),
    /// the folds too; the selection survives when its item still exists.
    pub fn set_model(&mut self, cx: &mut Cx, m: SequencerModel) {
        self.model = m;
        self.generation = self.generation.wrapping_add(1);
        self.rows_dirty = true;
        // Fold state of tracks that are gone would only pile up.
        let tracks = &self.model.tracks;
        self.open
            .retain(|(id, _)| tracks.iter().any(|t| t.id == *id));
        self.playhead = finite_or(self.model.playhead, 0.0);
        if let Some(id) = self.selected {
            if self.model.find(id).is_none() {
                self.selected = None;
            }
        }
        if let Some(d) = self.drag {
            let gone = match d.kind {
                DragKind::Move | DragKind::ResizeLeft | DragKind::ResizeRight => {
                    self.model.find(d.id).is_none()
                }
                DragKind::Label => !self.model.labels.iter().any(|l| l.id == d.id),
                DragKind::Scrub | DragKind::Pan => false,
            };
            if gone {
                self.drag = None;
                self.snap_guide = None;
            }
        }
        self.tip_for = None;
        self.refresh_tip();
        self.draw_bg.redraw(cx);
    }

    pub fn model(&self) -> &SequencerModel {
        &self.model
    }

    fn ensure_rows(&mut self) {
        if self.rows_dirty {
            self.rows_dirty = false;
            lay_rows(&mut self.rows, &self.model.tracks, &self.open);
        }
    }

    fn relayout(&mut self) {
        self.rows_dirty = true;
    }

    /// Folds or unfolds track `track` (its children rows). Emits nothing.
    pub fn set_expanded(&mut self, cx: &mut Cx, track: u64, open: bool) {
        match self.open.iter_mut().find(|(id, _)| *id == track) {
            Some(o) => o.1 = open,
            None => self.open.push((track, open)),
        }
        self.relayout();
        self.draw_bg.redraw(cx);
    }

    /// Rows shown (every ancestor open).
    pub fn visible_rows(&self) -> usize {
        let mut n = 0;
        walk_rows(&self.model.tracks, &self.open, |_| n += 1);
        n
    }

    fn is_open(&self, track: &SequencerTrack) -> bool {
        self.open
            .iter()
            .find(|(id, _)| *id == track.id)
            .map_or(track.expanded, |(_, o)| *o)
    }

    fn duration(&self) -> f64 {
        finite_or(self.model.duration, 0.0).max(0.0)
    }

    // ---- view --------------------------------------------------------------

    fn zoom_range(&self) -> (f64, f64) {
        let lo = finite_or(self.min_zoom, 5.0).max(1e-3);
        (lo, finite_or(self.max_zoom, 5000.0).max(lo))
    }

    fn clamp_zoom(&self, z: f64) -> f64 {
        let (lo, hi) = self.zoom_range();
        finite_or(z, lo).clamp(lo, hi)
    }

    fn set_width(&mut self, width: f64) {
        self.view.x0 = self.name_width.max(0.0);
        self.view.w = (width - self.view.x0).max(1.0);
    }

    fn fit_view(&mut self) {
        let z = self.clamp_zoom((self.view.w - 24.0) / self.duration().max(0.1));
        self.view.zoom = z;
        self.view.offset = -12.0 / z;
    }

    fn clamp_offset(&mut self) {
        if self.view.zoom <= 0.0 {
            return;
        }
        let half = 0.5 * self.view.w / self.view.zoom;
        let lo = -half;
        let hi = (self.duration() - half).max(lo);
        self.view.offset = finite_or(self.view.offset, lo).clamp(lo, hi);
    }

    /// Zooms by `f` keeping the time under local x `x` in place.
    fn zoom_about(&mut self, x: f64, f: f64) {
        let t = self.view.t_at(x);
        self.view.zoom = self.clamp_zoom(self.view.zoom * f);
        self.view.offset = t - (x - self.view.x0) / self.view.zoom;
        self.clamp_offset();
    }

    /// Makes sure the view has a zoom (a widget not drawn yet has none).
    fn ensure_view(&mut self) {
        if self.view.zoom <= 0.0 || !self.view.zoom.is_finite() {
            self.fit_view();
        }
    }

    /// Pixels per second and the time at the track area's left edge.
    pub fn zoom(&self) -> (f64, f64) {
        (self.view.zoom, self.view.offset)
    }

    /// Sets the zoom (pixels per second, clamped to min_zoom..max_zoom) and
    /// the time at the left edge. Emits nothing.
    pub fn set_zoom(&mut self, cx: &mut Cx, px_per_s: f64, offset: f64) {
        self.view.zoom = self.clamp_zoom(px_per_s);
        self.view.offset = finite_or(offset, 0.0);
        self.clamp_offset();
        self.fitted = true;
        self.draw_bg.redraw(cx);
    }

    /// Fits the model's duration into the track area. Emits nothing.
    pub fn fit(&mut self, cx: &mut Cx) {
        if self.view.w > 1.0 {
            self.fit_view();
            self.fitted = true;
        } else {
            self.fitted = false; // fitted at the first draw
        }
        self.draw_bg.redraw(cx);
    }

    /// Pans so time `t` is in view (it lands 10% in), only when it is not.
    pub fn scroll_to(&mut self, cx: &mut Cx, t: f64) {
        if !t.is_finite() || self.view.zoom <= 0.0 {
            return;
        }
        if t < self.view.offset || t > self.view.t_end() {
            self.view.offset = t - 0.1 * self.view.w / self.view.zoom;
            self.clamp_offset();
            self.draw_bg.redraw(cx);
        }
    }

    // ---- playhead ----------------------------------------------------------

    pub fn playhead(&self) -> f64 {
        self.playhead
    }

    /// Moves the playhead. While the view holds still the quad is moved in
    /// place (no redraw); with `follow` (and no drag) a playhead leaving
    /// the view pans it (a redraw).
    pub fn set_playhead(&mut self, cx: &mut Cx, t: f64) {
        let t = finite_or(t, 0.0);
        if t == self.playhead {
            return;
        }
        self.playhead = t;
        self.model.playhead = t;
        if self.follow && self.drag.is_none() && self.view.zoom > 0.0 {
            if t < self.view.offset || t > self.view.t_end() {
                self.view.offset = t - 0.1 * self.view.w / self.view.zoom;
                self.clamp_offset();
                self.draw_bg.redraw(cx);
                return;
            }
        }
        self.move_head(cx);
    }

    /// The playhead quad for a widget drawn at `origin` with `height`
    /// (absolute); zero-sized when the playhead is out of view.
    fn head_rect_at(&self, origin: DVec2, height: f64) -> Rect {
        let v = self.view;
        let x = v.x_at(self.playhead);
        let top = (self.ruler_height - HEAD_H - 1.0).max(0.0);
        if v.zoom <= 0.0 || x < v.x0 - 0.5 || x > v.x0 + v.w + 0.5 {
            return Rect {
                pos: origin,
                size: dvec2(0.0, 0.0),
            };
        }
        Rect {
            pos: origin + dvec2(x - HEAD_W * 0.5, top),
            size: dvec2(HEAD_W, (height - top).max(0.0)),
        }
    }

    fn move_head(&mut self, cx: &mut Cx) {
        let r = self.draw_bg.area().rect(cx);
        if r.size.x <= 0.0 {
            return;
        }
        let rect = self.head_rect_at(r.pos, r.size.y);
        if rect != self.head_rect {
            self.head_rect = rect;
            self.draw_head.update_abs(cx, rect);
        }
    }

    // ---- selection and hover ----------------------------------------------

    pub fn selected(&self) -> Option<u64> {
        self.selected
    }

    /// Selects item `id` (or nothing). Emits nothing.
    pub fn select(&mut self, cx: &mut Cx, id: Option<u64>) {
        let id = id.filter(|i| self.model.find(*i).is_some());
        if id != self.selected {
            self.selected = id;
            self.draw_bg.redraw(cx);
        }
    }

    fn select_emit(&mut self, cx: &mut Cx, id: Option<u64>) {
        if id != self.selected {
            self.selected = id;
            cx.widget_action(self.uid, SequencerAction::Selected(id));
            self.draw_bg.redraw(cx);
        }
    }

    /// The tooltip of the hovered item: "name  caption  start 1.250 s  dur
    /// 0.600 s [delay 0.100 s] [x3 | x inf] [yoyo] [gap 0.200 s]" (x3: three
    /// iterations).
    fn refresh_tip(&mut self) {
        let id = match self.hover {
            Hover::Item(id, _) => Some(id),
            _ => None,
        };
        if id == self.tip_for {
            return;
        }
        self.tip_for = id;
        self.tip.clear();
        self.tip_w = None;
        let Some((ti, ii)) = id.and_then(|id| self.model.find(id)) else {
            return;
        };
        let t = &self.model.tracks[ti];
        let it = &t.items[ii];
        let s = &mut self.tip;
        s.push_str(&t.name);
        if !it.label.is_empty() && it.label != t.name {
            s.push_str("  ");
            s.push_str(&it.label);
        }
        let _ = write!(
            s,
            "  start {:.3} s  dur {:.3} s",
            clean(it.start),
            clean(it.duration)
        );
        if it.delay > 0.0 {
            let _ = write!(s, "  delay {:.3} s", it.delay);
        }
        if it.repeat < 0 {
            s.push_str("  x inf");
        } else if it.repeat > 0 {
            let _ = write!(s, "  x{}", it.repeat as i64 + 1);
        }
        if it.yoyo {
            s.push_str("  yoyo");
        }
        if it.repeat != 0 && it.repeat_delay > 0.0 {
            let _ = write!(s, "  gap {:.3} s", it.repeat_delay);
        }
        if it.locked {
            s.push_str("  (locked)");
        }
    }

    fn set_hover(&mut self, cx: &mut Cx, h: Hover) {
        if h != self.hover {
            self.hover = h;
            self.refresh_tip();
            self.draw_bg.redraw(cx);
        }
    }

    // ---- hit testing ----------------------------------------------------------

    /// The row under local y, if any.
    fn row_at(&self, y: f64) -> Option<usize> {
        let r = ((y - self.ruler_height) / self.row_height.max(1.0)).floor();
        (r >= 0.0 && (r as usize) < self.rows.len()).then_some(r as usize)
    }

    fn pick(&self, p: DVec2) -> Hover {
        for (i, b) in self.buttons.iter().enumerate() {
            if b.contains(p) {
                return Hover::Button(i as u8);
            }
        }
        let v = self.view;
        if p.x < 0.0 || p.y < 0.0 {
            return Hover::None;
        }
        if p.y < self.ruler_height {
            if p.x < v.x0 {
                return Hover::None;
            }
            // The nearest label line within reach, or a flag box.
            let mut best: Option<(u64, f64)> = None;
            for &(li, x0, x1) in &self.flags {
                let Some(l) = self.model.labels.get(li as usize) else {
                    continue;
                };
                let d = (p.x - x0).abs();
                let on_flag = p.y <= FLAG_H + 1.0 && p.x >= x0 && p.x <= x1;
                if (d <= FLAG_GRAB || on_flag) && best.map_or(true, |(_, bd)| d < bd) {
                    best = Some((l.id, d));
                }
            }
            return match best {
                Some((id, _)) => Hover::Label(id),
                None => Hover::Ruler,
            };
        }
        let Some(ri) = self.row_at(p.y) else {
            return if p.x >= v.x0 {
                Hover::Empty
            } else {
                Hover::None
            };
        };
        if p.x < v.x0 {
            let r = self.rows[ri];
            let ax = 8.0 + r.depth as f64 * self.indent;
            if r.openable && p.x >= ax - 3.0 && p.x <= ax + 13.0 {
                return Hover::Fold(ri as u32);
            }
            return Hover::Name(ri as u32);
        }
        let e = self.edge_px.max(1.0);
        for h in self.hits.iter().rev() {
            if p.y < h.y0 || p.y > h.y1 {
                continue;
            }
            if h.marker {
                let c = dvec2((h.x0 + h.x1) * 0.5, (h.y0 + h.y1) * 0.5);
                let reach = self.row_height * 0.35;
                if (p.x - c.x).abs() <= reach && (p.y - c.y).abs() <= reach {
                    return Hover::Item(h.id, Zone::Body);
                }
                continue;
            }
            if p.x < h.x0 - e || p.x > h.x1 + e {
                continue;
            }
            // The edge bands inside the bar shrink with it before the move
            // zone does: `e` each on a bar 3e wide or more, nothing on a bar
            // narrower than e (whose edges are then grabbed just outside it).
            let band = if h.locked {
                0.0
            } else {
                ((h.x1 - h.x0 - e) * 0.5).clamp(0.0, e)
            };
            if p.x >= h.x0 + band && p.x <= h.x1 - band {
                return Hover::Item(h.id, Zone::Body);
            }
            if !h.locked {
                if p.x > h.x1 - band {
                    return Hover::Item(h.id, Zone::Right);
                }
                if p.x < h.x0 + band {
                    return Hover::Item(h.id, Zone::Left);
                }
            }
        }
        Hover::Empty
    }

    fn cursor_for(h: Hover) -> MouseCursor {
        match h {
            Hover::Item(_, Zone::Left) | Hover::Item(_, Zone::Right) => MouseCursor::EwResize,
            Hover::Item(..)
            | Hover::Label(_)
            | Hover::Ruler
            | Hover::Fold(_)
            | Hover::Button(_)
            // Empty track space pans when dragged.
            | Hover::Empty => MouseCursor::Hand,
            _ => MouseCursor::Default,
        }
    }

    // ---- snapping ------------------------------------------------------------

    /// The snap radius in seconds: `snap_px` at the current zoom.
    fn snap_radius(&self) -> f64 {
        if self.snap_px > 0.0 && self.view.zoom > 0.0 {
            self.snap_px / self.view.zoom
        } else {
            0.0
        }
    }

    // ---- gestures --------------------------------------------------------------

    fn emit(&self, cx: &mut Cx, a: SequencerAction) {
        cx.widget_action(self.uid, a);
    }

    fn scrub_to(&mut self, cx: &mut Cx, t: f64) {
        let t = t.clamp(0.0, self.duration());
        if t != self.playhead {
            self.playhead = t;
            self.model.playhead = t;
            self.emit(cx, SequencerAction::Scrub(t));
            self.move_head(cx);
        }
    }

    fn press(&mut self, cx: &mut Cx, p: DVec2) {
        self.pointer = p;
        let h = self.pick(p);
        let blank = Drag {
            kind: DragKind::Pan,
            id: 0,
            grab_dt: 0.0,
            start0: 0.0,
            dur0: 0.0,
            time0: 0.0,
            offset0: self.view.offset,
            press: p,
            moved: false,
            edited: false,
        };
        match h {
            Hover::Button(i) => {
                let before = (self.view.zoom, self.view.offset);
                let mid = self.view.x0 + self.view.w * 0.5;
                match i {
                    0 => self.zoom_about(mid, 1.0 / ZOOM_STEP),
                    1 => self.zoom_about(mid, ZOOM_STEP),
                    _ => {
                        self.fit_view();
                        self.fitted = true;
                    }
                }
                // Clamped at min_zoom / max_zoom (or already fitted): nothing
                // changed, nothing to report.
                if (self.view.zoom, self.view.offset) != before {
                    self.emit(cx, SequencerAction::Zoom(self.view.zoom, self.view.offset));
                    self.draw_bg.redraw(cx);
                }
            }
            Hover::Label(id) => {
                let time = self
                    .model
                    .labels
                    .iter()
                    .find(|l| l.id == id)
                    .map_or(0.0, |l| l.time);
                self.model
                    .label_snap_targets(id, self.playhead, &mut self.snap_targets);
                self.drag = Some(Drag {
                    kind: DragKind::Label,
                    id,
                    grab_dt: self.view.t_at(p.x) - time,
                    time0: time,
                    ..blank
                });
                self.draw_bg.redraw(cx);
            }
            Hover::Ruler => {
                self.drag = Some(Drag {
                    kind: DragKind::Scrub,
                    ..blank
                });
                self.emit(cx, SequencerAction::ScrubStart);
                self.scrub_to(cx, self.view.t_at(p.x));
            }
            Hover::Fold(ri) => {
                let r = self.rows[ri as usize];
                let id = self.model.tracks[r.track as usize].id;
                self.set_expanded(cx, id, !r.open);
            }
            Hover::Name(ri) => {
                let r = self.rows[ri as usize];
                let first = self.model.tracks[r.track as usize]
                    .items
                    .first()
                    .map(|i| i.id);
                if first.is_some() {
                    self.select_emit(cx, first);
                }
            }
            Hover::Item(id, zone) => {
                self.select_emit(cx, Some(id));
                // A locked item selects only.
                if let Some(it) = self.model.item(id).filter(|it| !it.locked) {
                    let t = self.view.t_at(p.x);
                    let (kind, edge) = match zone {
                        Zone::Body => (DragKind::Move, it.start),
                        Zone::Left => (DragKind::ResizeLeft, it.start),
                        Zone::Right => (DragKind::ResizeRight, it.start + it.duration),
                    };
                    self.drag = Some(Drag {
                        kind,
                        id,
                        grab_dt: t - edge,
                        start0: it.start,
                        dur0: it.duration,
                        ..blank
                    });
                    // The targets are taken now, once: see item_snap_targets.
                    self.model
                        .item_snap_targets(id, self.playhead, &mut self.snap_targets);
                    self.draw_bg.redraw(cx);
                }
            }
            Hover::Empty => {
                self.drag = Some(blank);
            }
            Hover::None => {}
        }
        self.hover = h;
        self.refresh_tip();
        cx.set_cursor(Self::cursor_for(h));
    }

    fn drag_to(&mut self, cx: &mut Cx, p: DVec2, alt: bool) {
        let Some(mut d) = self.drag else {
            return;
        };
        self.pointer = p;
        if !d.moved && (p - d.press).length() >= TAP_SLOP {
            d.moved = true;
        }
        let v = self.view;
        let t = v.t_at(p.x);
        let uid = self.uid;
        match d.kind {
            DragKind::Scrub => self.scrub_to(cx, t),
            DragKind::Pan => {
                let o = d.offset0 - (p.x - d.press.x) / v.zoom;
                self.view.offset = o;
                self.clamp_offset();
                if self.view.offset != v.offset {
                    cx.widget_action(uid, SequencerAction::Zoom(self.view.zoom, self.view.offset));
                    self.draw_bg.redraw(cx);
                }
            }
            DragKind::Label => {
                let radius = self.snap_radius();
                let targets: &[f64] = if alt { &[] } else { &self.snap_targets };
                let raw = t - d.grab_dt;
                let snap = snap_nearest(targets, &[raw], radius);
                self.snap_guide = snap.map(|s| s.1);
                let time = (raw + snap.map_or(0.0, |s| s.0)).clamp(0.0, self.duration());
                if let Some(l) = self.model.labels.iter_mut().find(|l| l.id == d.id) {
                    if l.time != time {
                        l.time = time;
                        d.edited = true;
                        cx.widget_action(uid, SequencerAction::LabelMoved { id: d.id, time });
                    }
                }
                self.draw_bg.redraw(cx);
            }
            DragKind::Move | DragKind::ResizeLeft | DragKind::ResizeRight => {
                let Some(it) = self.model.item(d.id) else {
                    self.drag = None;
                    return;
                };
                let (start, duration) = (it.start, it.duration);
                // The model's pure drag maths against the targets taken at
                // the press (Alt: none).
                let radius = self.snap_radius();
                let targets: &[f64] = if alt { &[] } else { &self.snap_targets };
                let raw = t - d.grab_dt;
                let m = &self.model;
                let r = match d.kind {
                    DragKind::Move => m.drag_move(d.id, raw, targets, radius),
                    DragKind::ResizeRight => {
                        m.drag_resize_end(d.id, raw, targets, radius, MIN_DURATION)
                    }
                    // The end stays where it was at the press.
                    _ => m.drag_resize_start(
                        d.id,
                        raw,
                        d.start0 + d.dur0,
                        targets,
                        radius,
                        MIN_DURATION,
                    ),
                };
                let Some(r) = r else {
                    self.drag = None;
                    return;
                };
                self.snap_guide = r.guide;
                let moved = r.start != start;
                let resized = r.duration != duration;
                if moved || resized {
                    if let Some((ti, ii)) = self.model.find(d.id) {
                        let item = &mut self.model.tracks[ti].items[ii];
                        item.start = r.start;
                        item.duration = r.duration;
                    }
                    d.edited = true;
                    if moved {
                        cx.widget_action(
                            uid,
                            SequencerAction::ItemMoved {
                                id: d.id,
                                start: r.start,
                            },
                        );
                    }
                    if resized {
                        cx.widget_action(
                            uid,
                            SequencerAction::ItemResized {
                                id: d.id,
                                duration: r.duration,
                            },
                        );
                    }
                }
                self.tip_for = None;
                self.refresh_tip();
                self.draw_bg.redraw(cx);
            }
        }
        self.drag = Some(d);
    }

    fn release(&mut self, cx: &mut Cx) {
        let Some(d) = self.drag.take() else {
            return;
        };
        self.snap_guide = None;
        match d.kind {
            DragKind::Scrub => self.emit(cx, SequencerAction::ScrubEnd),
            DragKind::Pan if !d.moved => self.select_emit(cx, None),
            _ => {}
        }
        if d.edited {
            self.emit(cx, SequencerAction::EditEnd);
        }
        self.draw_bg.redraw(cx);
    }

    /// Escape: puts the dragged item, edge, label or view back.
    fn cancel(&mut self, cx: &mut Cx) {
        let Some(d) = self.drag.take() else {
            return;
        };
        self.snap_guide = None;
        let uid = self.uid;
        match d.kind {
            DragKind::Move | DragKind::ResizeLeft | DragKind::ResizeRight => {
                if let Some((ti, ii)) = self.model.find(d.id) {
                    let it = &mut self.model.tracks[ti].items[ii];
                    if it.start != d.start0 {
                        it.start = d.start0;
                        cx.widget_action(
                            uid,
                            SequencerAction::ItemMoved {
                                id: d.id,
                                start: d.start0,
                            },
                        );
                    }
                    if it.duration != d.dur0 {
                        it.duration = d.dur0;
                        cx.widget_action(
                            uid,
                            SequencerAction::ItemResized {
                                id: d.id,
                                duration: d.dur0,
                            },
                        );
                    }
                }
            }
            DragKind::Label => {
                if let Some(l) = self.model.labels.iter_mut().find(|l| l.id == d.id) {
                    if l.time != d.time0 {
                        l.time = d.time0;
                        cx.widget_action(
                            uid,
                            SequencerAction::LabelMoved {
                                id: d.id,
                                time: d.time0,
                            },
                        );
                    }
                }
            }
            DragKind::Pan => {
                if self.view.offset != d.offset0 {
                    self.view.offset = d.offset0;
                    self.emit(cx, SequencerAction::Zoom(self.view.zoom, self.view.offset));
                }
            }
            DragKind::Scrub => self.emit(cx, SequencerAction::ScrubEnd),
        }
        if d.edited {
            self.emit(cx, SequencerAction::EditEnd);
        }
        self.tip_for = None;
        self.refresh_tip();
        self.draw_bg.redraw(cx);
    }

    /// Left / Right: the selected unlocked item by one minor tick (a major
    /// one with Shift).
    fn nudge(&mut self, cx: &mut Cx, dir: f64, major: bool) {
        let Some(id) = self.selected else {
            return;
        };
        let Some((ti, ii)) = self.model.find(id) else {
            return;
        };
        if self.model.tracks[ti].items[ii].locked {
            return;
        }
        self.ensure_view();
        let (maj, min, _) = ticks_for(self.view.zoom);
        let step = if major { maj } else { min };
        let floor = self.model.min_start(id).unwrap_or(0.0);
        let it = &mut self.model.tracks[ti].items[ii];
        let s = (it.start + dir * step).max(floor);
        if s != it.start {
            it.start = s;
            self.emit(cx, SequencerAction::ItemMoved { id, start: s });
            self.emit(cx, SequencerAction::EditEnd);
            self.tip_for = None;
            self.refresh_tip();
            self.draw_bg.redraw(cx);
        }
    }

    // ---- drawing -------------------------------------------------------------

    fn kind_color(&self, t: &SequencerTrack) -> Vec4f {
        match t.color {
            Some([r, g, b, a]) => vec4(r, g, b, a),
            None => match t.kind {
                SequencerKind::Tween => self.color_tween,
                SequencerKind::Timeline => self.color_timeline,
                SequencerKind::Group => self.color_group,
                SequencerKind::Call | SequencerKind::Pause => self.color_marker,
            },
        }
    }

    fn item_state(&self, id: u64) -> f32 {
        if self.drag.is_some_and(|d| {
            d.id == id
                && matches!(
                    d.kind,
                    DragKind::Move | DragKind::ResizeLeft | DragKind::ResizeRight
                )
        }) {
            3.0
        } else if self.selected == Some(id) {
            2.0
        } else if matches!(self.hover, Hover::Item(h, _) if h == id) {
            1.0
        } else {
            0.0
        }
    }

    fn line_h(&self) -> f64 {
        self.draw_text.text_style.font_size as f64 * 1.3
    }

    /// Measures the captions, label names and button texts once per model
    /// (and font size).
    fn ensure_widths(&mut self, cx: &mut Cx2d) {
        let key = (self.generation, self.draw_text.text_style.font_size);
        if self.widths_for == Some(key) {
            return;
        }
        self.widths_for = Some(key);
        self.track_base.clear();
        self.cap_w.clear();
        self.label_w.clear();
        for t in &self.model.tracks {
            self.track_base.push(self.cap_w.len() as u32);
            for it in &t.items {
                let w = if it.label.is_empty() {
                    0.0
                } else {
                    measure(&self.draw_text, cx, &it.label)
                };
                self.cap_w.push(w);
            }
        }
        for l in &self.model.labels {
            self.label_w.push(measure(&self.draw_text, cx, &l.name));
        }
        for (i, (text, _)) in BUTTONS.iter().enumerate() {
            self.button_w[i] = measure(&self.draw_text, cx, text);
        }
    }

    /// Bars, delays, markers and captions of every visible row (inside the
    /// track area's clip); records the first iterations as hits.
    fn draw_items(&mut self, cx: &mut Cx2d, o: DVec2) {
        let v = self.view;
        let (t0, t1) = (v.offset, v.t_end());
        let rh = self.row_height;
        let text_dy = (rh - self.line_h()) * 0.5;
        self.hits.clear();
        for ri in 0..self.rows.len() {
            let row = self.rows[ri];
            let ti = row.track as usize;
            let y0 = self.ruler_height + ri as f64 * rh;
            let color = self.kind_color(&self.model.tracks[ti]);
            for ii in 0..self.model.tracks[ti].items.len() {
                let it = &self.model.tracks[ti].items[ii];
                let (id, start, delay, dur, locked) =
                    (it.id, it.start, it.delay, it.duration, it.locked);
                let (repeat, rdelay, yoyo, rev) =
                    (it.repeat, it.repeat_delay, it.yoyo, it.reversed);
                let state = self.item_state(id);
                if !(dur > 0.0) {
                    // A marker: a diamond with its caption to the right.
                    let x = v.x_at(start);
                    let s = rh * 0.7;
                    self.draw_marker.color = color;
                    self.draw_marker.state = state;
                    self.draw_marker.draw_abs(
                        cx,
                        Rect {
                            pos: o + dvec2(x - s * 0.5, y0 + (rh - s) * 0.5),
                            size: dvec2(s, s),
                        },
                    );
                    self.hits.push(BarHit {
                        id,
                        x0: x - s * 0.5,
                        x1: x + s * 0.5,
                        y0,
                        y1: y0 + rh,
                        marker: true,
                        locked,
                    });
                    let label = &self.model.tracks[ti].items[ii].label;
                    if !label.is_empty() && x + s < v.x0 + v.w {
                        self.draw_text.color = self.color_dim;
                        self.draw_text.draw_abs(
                            cx,
                            o + dvec2(x + s * 0.5 + 4.0, y0 + text_dy),
                            label,
                        );
                    }
                    continue;
                }
                let bar_y = y0 + 3.0;
                let bar_h = (rh - 6.0).max(2.0);
                // The delay, hatched, before the first iteration.
                if delay > 0.0 && start > t0 && start - delay < t1 {
                    let xa = v.x_at(start - delay).max(v.x0 - 2.0);
                    let xb = v.x_at(start);
                    self.draw_delay.color = color;
                    self.draw_delay.draw_abs(
                        cx,
                        Rect {
                            pos: o + dvec2(xa, bar_y),
                            size: dvec2((xb - xa).max(0.0), bar_h),
                        },
                    );
                }
                // One segment per visible iteration.
                let cycle = dur + rdelay.max(0.0);
                let k_lo = ((t0 - start - dur) / cycle).ceil().max(0.0) as i64;
                let mut k_hi = ((t1 - start) / cycle).floor() as i64;
                if repeat >= 0 {
                    k_hi = k_hi.min(repeat as i64);
                }
                k_hi = k_hi.min(k_lo + MAX_SEGMENTS - 1);
                let editable = if locked { 0.0 } else { 1.0 };
                for k in k_lo..=k_hi {
                    let seg = start + k as f64 * cycle;
                    let xa = v.x_at(seg);
                    let xb = v.x_at(seg + dur);
                    let mut c = color;
                    if k > 0 {
                        c.w *= 0.65;
                    }
                    self.draw_bar.color = c;
                    // A yoyo pass and a backwards item both run the ramp
                    // the other way (both: forwards again).
                    let back = (yoyo && k % 2 == 1) != rev;
                    self.draw_bar.mirror = if back { 1.0 } else { 0.0 };
                    self.draw_bar.state = if k == 0 { state } else { 0.0 };
                    self.draw_bar.open = editable;
                    self.draw_bar.draw_abs(
                        cx,
                        Rect {
                            pos: o + dvec2(xa, bar_y),
                            size: dvec2((xb - xa).max(1.0), bar_h),
                        },
                    );
                    // The repeat delay's gap: a hatched connector.
                    let more = repeat < 0 || k < repeat as i64;
                    if more && rdelay > 0.0 {
                        let xc = v.x_at(seg + cycle);
                        self.draw_delay.color = color;
                        self.draw_delay.draw_abs(
                            cx,
                            Rect {
                                pos: o + dvec2(xb, y0 + rh * 0.5 - 2.0),
                                size: dvec2((xc - xb).max(0.0), 4.0),
                            },
                        );
                    }
                }
                let (xa, xb) = (v.x_at(start), v.x_at(start + dur));
                self.hits.push(BarHit {
                    id,
                    x0: xa,
                    x1: xb,
                    y0: bar_y,
                    y1: bar_y + bar_h,
                    marker: false,
                    locked,
                });
                // The caption inside the first iteration when it fits.
                let vis0 = xa.max(v.x0);
                let vis1 = xb.min(v.x0 + v.w);
                if vis1 - vis0 > 36.0 {
                    let label = &self.model.tracks[ti].items[ii].label;
                    let base = self.track_base.get(ti).map_or(usize::MAX, |b| *b as usize);
                    let w = self
                        .cap_w
                        .get(base.wrapping_add(ii))
                        .copied()
                        .unwrap_or(0.0);
                    if !label.is_empty() && w + 10.0 <= vis1 - vis0 {
                        self.draw_text.color = self.color_caption;
                        self.draw_text
                            .draw_abs(cx, o + dvec2(vis0 + 6.0, y0 + text_dy), label);
                    }
                }
            }
        }
    }

    /// The ruler's tick labels, the label lines and flags, the snap guide.
    fn draw_ruler(&mut self, cx: &mut Cx2d, o: DVec2, height: f64, major: f64, dec: usize) {
        let v = self.view;
        // Tick labels right of each major tick.
        self.draw_text.color = self.color_dim;
        let first = (v.offset / major).ceil() * major;
        let text_y = (self.ruler_height * 0.4 - self.line_h()).max(0.0) + 1.0;
        for i in 0..512 {
            let t = first + i as f64 * major;
            let x = v.x_at(t);
            if x > v.x0 + v.w {
                break;
            }
            self.num.clear();
            let _ = write!(self.num, "{:.*}s", dec, clean(t));
            self.draw_text
                .draw_abs(cx, o + dvec2(x + 3.0, text_y), &self.num);
        }
        // Label lines (dashed, through the rows) with their flags.
        self.flags.clear();
        for li in 0..self.model.labels.len() {
            let x = v.x_at(self.model.labels[li].time);
            let id = self.model.labels[li].id;
            let w = self.label_w.get(li).copied().unwrap_or(0.0) + 8.0;
            let hot = self.hover == Hover::Label(id)
                || self
                    .drag
                    .is_some_and(|d| d.kind == DragKind::Label && d.id == id);
            self.draw_flag.color = self.color_marker;
            self.draw_flag.state = if hot { 1.0 } else { 0.0 };
            self.draw_flag.head = FLAG_H as f32;
            self.draw_flag.draw_abs(
                cx,
                Rect {
                    pos: o + dvec2(x - 1.0, 1.0),
                    size: dvec2(w, (height - 1.0).max(0.0)),
                },
            );
            self.flags.push((li as u32, x, x + w));
        }
        if let Some(g) = self.snap_guide {
            let x = v.x_at(g);
            self.draw_flag.state = 3.0;
            self.draw_flag.head = 0.0;
            self.draw_flag.draw_abs(
                cx,
                Rect {
                    pos: o + dvec2(x - 1.0, self.ruler_height),
                    size: dvec2(3.0, (height - self.ruler_height).max(0.0)),
                },
            );
        }
        // The names sit on their flags: a text call above the flags.
        if !self.model.labels.is_empty() {
            self.draw_text.new_draw_call(cx);
            self.draw_text.color = self.color_caption;
            let dy = (FLAG_H - self.line_h()) * 0.5 + 1.0;
            for i in 0..self.flags.len() {
                let (li, x0, _) = self.flags[i];
                let name = &self.model.labels[li as usize].name;
                self.draw_text.draw_abs(cx, o + dvec2(x0 + 4.0, dy), name);
            }
        }
    }

    /// Fold arrows and names (inside the name column's clip).
    fn draw_names(&mut self, cx: &mut Cx2d, o: DVec2) {
        let rh = self.row_height;
        let text_dy = (rh - self.line_h()) * 0.5;
        let text_color = self.draw_text.color;
        for ri in 0..self.rows.len() {
            let r = self.rows[ri];
            let y0 = self.ruler_height + ri as f64 * rh;
            let ax = 8.0 + r.depth as f64 * self.indent;
            if r.openable {
                self.draw_fold.color = self.color_dim;
                self.draw_fold.open = if r.open { 1.0 } else { 0.0 };
                self.draw_fold.draw_abs(
                    cx,
                    Rect {
                        pos: o + dvec2(ax, y0 + (rh - 10.0) * 0.5),
                        size: dvec2(10.0, 10.0),
                    },
                );
            }
            self.draw_text.color = text_color;
            let name = &self.model.tracks[r.track as usize].name;
            self.draw_text
                .draw_abs(cx, o + dvec2(ax + 14.0, y0 + text_dy), name);
        }
    }

    /// The "-", "+" and "Fit" buttons in the corner cell.
    fn draw_buttons(&mut self, cx: &mut Cx2d, o: DVec2) {
        let y = ((self.ruler_height - BUTTON_H) * 0.5).max(0.0);
        let mut x = 6.0;
        for (i, (_, w)) in BUTTONS.iter().enumerate() {
            let r = Rect {
                pos: dvec2(x, y),
                size: dvec2(*w, BUTTON_H),
            };
            self.buttons[i] = r;
            self.draw_button.state = if self.hover == Hover::Button(i as u8) {
                1.0
            } else {
                0.0
            };
            self.draw_button.draw_abs(cx, r.translate(o));
            x += w + 4.0;
        }
        self.draw_text.new_draw_call(cx);
        let dy = (BUTTON_H - self.line_h()) * 0.5;
        for (i, (text, _)) in BUTTONS.iter().enumerate() {
            let r = self.buttons[i];
            let w = self.button_w[i];
            self.draw_text.draw_abs(
                cx,
                o + dvec2(r.pos.x + (r.size.x - w) * 0.5 + 1.0, y + dy),
                text,
            );
        }
    }

    /// The drag's time tag at the pointer, or the hovered item's tooltip.
    fn draw_tip_box(&mut self, cx: &mut Cx2d, o: DVec2, size: DVec2) {
        let pad = 5.0;
        let lh = self.line_h();
        let (pos, text_is_num) = match self.drag {
            Some(d)
                if matches!(
                    d.kind,
                    DragKind::Move | DragKind::ResizeLeft | DragKind::ResizeRight | DragKind::Label
                ) =>
            {
                self.num.clear();
                if d.kind == DragKind::Label {
                    let t = self
                        .model
                        .labels
                        .iter()
                        .find(|l| l.id == d.id)
                        .map_or(0.0, |l| l.time);
                    let _ = write!(self.num, "{:.3} s", clean(t));
                } else if let Some(it) = self.model.item(d.id) {
                    if d.kind == DragKind::ResizeRight {
                        let _ = write!(self.num, "dur {:.3} s", clean(it.duration));
                    } else {
                        let _ = write!(self.num, "{:.3} s", clean(it.start));
                    }
                }
                (self.pointer + dvec2(14.0, -lh - 2.0 * pad - 4.0), true)
            }
            None if !self.tip.is_empty() => {
                let Hover::Item(id, _) = self.hover else {
                    return;
                };
                let Some(h) = self.hits.iter().find(|h| h.id == id).copied() else {
                    return;
                };
                let below = h.y1 + 4.0;
                let y = if below + lh + 2.0 * pad <= size.y {
                    below
                } else {
                    h.y0 - lh - 2.0 * pad - 4.0
                };
                (dvec2(h.x0.max(self.view.x0) + 4.0, y), false)
            }
            _ => return,
        };
        let text: &str = if text_is_num { &self.num } else { &self.tip };
        if text.is_empty() {
            return;
        }
        let w = if text_is_num {
            measure(&self.draw_text, cx, text)
        } else {
            match self.tip_w {
                Some(w) => w,
                None => {
                    let w = measure(&self.draw_text, cx, text);
                    self.tip_w = Some(w);
                    w
                }
            }
        } + 2.0 * pad;
        let x = pos.x.min(size.x - w - 2.0).max(2.0);
        let y = pos.y.max(0.0);
        self.draw_tip.draw_abs(
            cx,
            Rect {
                pos: o + dvec2(x, y),
                size: dvec2(w, lh + 2.0 * pad),
            },
        );
        self.draw_text.new_draw_call(cx);
        let text: &str = if text_is_num { &self.num } else { &self.tip };
        self.draw_text
            .draw_abs(cx, o + dvec2(x + pad, y + pad), text);
    }
}

fn finite_or(x: f64, d: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        d
    }
}

impl Widget for Sequencer {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let hit = event.hits(cx, self.draw_bg.area());
        if matches!(hit, Hit::Nothing) {
            return;
        }
        // Local pixels come from the drawn rect (a finger event's rect is the
        // clipped one).
        let r = self.draw_bg.area().rect(cx);
        self.set_width(r.size.x);
        self.ensure_view();
        // A model or a fold may have arrived since the last draw: the rows
        // the hit test reads must describe the current model.
        self.ensure_rows();
        match hit {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let p = fe.abs - r.pos;
                self.pointer = p;
                if self.drag.is_some() {
                    return;
                }
                let h = self.pick(p);
                cx.set_cursor(Self::cursor_for(h));
                self.set_hover(cx, h);
            }
            Hit::FingerHoverOut(_) => {
                if self.drag.is_none() {
                    self.set_hover(cx, Hover::None);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                cx.set_key_focus(self.draw_bg.area());
                self.press(cx, fe.abs - r.pos);
            }
            Hit::FingerMove(fe) => {
                if let Some(d) = self.drag {
                    cx.set_cursor(match d.kind {
                        DragKind::ResizeLeft | DragKind::ResizeRight => MouseCursor::EwResize,
                        _ => MouseCursor::Hand,
                    });
                }
                self.drag_to(cx, fe.abs - r.pos, fe.modifiers.alt);
            }
            Hit::FingerUp(fe) => {
                self.release(cx);
                let h = if fe.is_over && fe.device.has_hovers() {
                    self.pick(fe.abs - r.pos)
                } else {
                    Hover::None
                };
                self.set_hover(cx, h);
            }
            Hit::FingerScroll(fe) => {
                let p = fe.abs - r.pos;
                if p.x < self.view.x0 {
                    return;
                }
                let before = (self.view.zoom, self.view.offset);
                let (mut used_x, mut used_y) = (false, false);
                if fe.modifiers.is_primary() {
                    if fe.scroll.y != 0.0 {
                        self.zoom_about(p.x, (2.0f64).powf(-fe.scroll.y / 240.0));
                        used_y = true;
                    }
                } else {
                    let mut dx = fe.scroll.x;
                    used_x = fe.scroll.x != 0.0;
                    if fe.modifiers.shift && fe.scroll.y != 0.0 {
                        dx += fe.scroll.y;
                        used_y = true;
                    }
                    if dx != 0.0 {
                        self.view.offset += dx / self.view.zoom;
                        self.clamp_offset();
                    }
                }
                if let Event::Scroll(se) = event {
                    if used_x {
                        se.handled_x.set(true);
                    }
                    if used_y {
                        se.handled_y.set(true);
                    }
                }
                if (self.view.zoom, self.view.offset) != before {
                    self.emit(cx, SequencerAction::Zoom(self.view.zoom, self.view.offset));
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::KeyDown(ke) => match ke.key_code {
                KeyCode::Escape => self.cancel(cx),
                KeyCode::ArrowLeft => self.nudge(cx, -1.0, ke.modifiers.shift),
                KeyCode::ArrowRight => self.nudge(cx, 1.0, ke.modifiers.shift),
                KeyCode::Home | KeyCode::End => {
                    let t = if ke.key_code == KeyCode::Home {
                        0.0
                    } else {
                        self.duration()
                    };
                    self.emit(cx, SequencerAction::ScrubStart);
                    self.scrub_to(cx, t);
                    self.emit(cx, SequencerAction::ScrubEnd);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_rows();
        self.ensure_widths(cx);
        let mut walk = walk;
        if walk.height.is_fit() {
            walk.height =
                Size::Fixed(self.ruler_height + self.rows.len() as f64 * self.row_height + 4.0);
        }
        let rect = cx.walk_turtle(walk);
        let o = rect.pos;
        self.set_width(rect.size.x);
        if (!self.fitted && !self.model.tracks.is_empty()) || self.view.zoom <= 0.0 {
            self.fit_view();
            self.fitted = !self.model.tracks.is_empty();
        }
        self.clamp_offset();
        let v = self.view;
        let (major, minor, dec) = ticks_for(v.zoom);
        let minor_div = if minor * v.zoom >= MINOR_MIN_PX {
            (major / minor).round()
        } else {
            1.0
        };
        let first = (v.offset / major).ceil() * major;

        // The rows the selection and the pointer are on.
        let row_of = |id: u64| {
            self.rows
                .iter()
                .position(|r| {
                    self.model.tracks[r.track as usize]
                        .items
                        .iter()
                        .any(|i| i.id == id)
                })
                .map_or(-1.0, |r| r as f32)
        };
        let sel_row = self.selected.map_or(-1.0, row_of);
        let hover_row = match self.hover {
            Hover::Item(id, _) => row_of(id),
            Hover::Fold(r) | Hover::Name(r) => r as f32,
            _ => -1.0,
        };
        let g = &mut self.draw_bg;
        g.name_w = v.x0 as f32;
        g.ruler_h = self.ruler_height as f32;
        g.row_h = self.row_height as f32;
        g.rows = self.rows.len() as f32;
        g.tick_x0 = ((first - v.offset) * v.zoom) as f32;
        g.tick_px = (major * v.zoom) as f32;
        g.minor_div = minor_div as f32;
        g.sel_row = sel_row;
        g.hover_row = hover_row;
        g.draw_abs(cx, rect);

        // The track area and the ruler above it share one clip.
        let text_color = self.draw_text.color;
        cx.begin_turtle(
            Walk::abs_rect(Rect {
                pos: o + dvec2(v.x0, 0.0),
                size: dvec2(v.w, rect.size.y),
            }),
            Layout::default(),
        );
        self.draw_items(cx, o);
        self.draw_ruler(cx, o, rect.size.y, major, dec);
        cx.end_turtle();
        self.draw_text.color = text_color;

        // The name column.
        cx.begin_turtle(
            Walk::abs_rect(Rect {
                pos: o + dvec2(0.0, self.ruler_height),
                size: dvec2(
                    (v.x0 - 1.0).max(0.0),
                    (rect.size.y - self.ruler_height).max(0.0),
                ),
            }),
            Layout::default(),
        );
        self.draw_names(cx, o);
        cx.end_turtle();

        self.draw_buttons(cx, o);

        // The playhead last: set_playhead moves this quad in place.
        self.draw_head.head = HEAD_H as f32;
        self.head_rect = self.head_rect_at(o, rect.size.y);
        self.draw_head.draw_abs(cx, self.head_rect);

        self.draw_tip_box(cx, o, rect.size);
        self.draw_text.color = text_color;
        cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
        DrawStep::done()
    }

    /// For `/snap`: `t=1.234 dur=3.400 zoom=180.00 off=-0.050 name_w=160
    /// ruler_h=26 row_h=22 sel=<id|-> rows=[name@start+duration, ..]
    /// labels=[name@time, ..]` (every visible row's first item), enough to
    /// compute where each bar is drawn.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        let mut s = String::new();
        let _ = write!(
            s,
            "t={:.3} dur={:.3} zoom={:.2} off={:.3} name_w={} ruler_h={} row_h={} sel=",
            clean(self.playhead),
            self.duration(),
            self.view.zoom,
            clean(self.view.offset),
            self.name_width,
            self.ruler_height,
            self.row_height
        );
        match self.selected {
            Some(id) => {
                let _ = write!(s, "{id}");
            }
            None => s.push('-'),
        }
        s.push_str(" rows=[");
        let mut first = true;
        walk_rows(&self.model.tracks, &self.open, |r| {
            if !first {
                s.push_str(", ");
            }
            first = false;
            let t = &self.model.tracks[r.track as usize];
            s.push_str(&t.name);
            match t.items.first() {
                Some(it) => {
                    let _ = write!(s, "@{:.3}+{:.3}", clean(it.start), clean(it.duration));
                }
                None => s.push_str("@-"),
            }
        });
        s.push_str("] labels=[");
        for (i, l) in self.model.labels.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            let _ = write!(s, "{}@{:.3}", l.name, clean(l.time));
        }
        s.push(']');
        Some(s)
    }
}

impl SequencerRef {
    /// Shows `m` (see [`Sequencer::set_model`]).
    pub fn set_model(&self, cx: &mut Cx, m: SequencerModel) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_model(cx, m);
        }
    }

    /// A copy of the model as the widget has it (edits included).
    pub fn model(&self) -> SequencerModel {
        self.borrow()
            .map(|inner| inner.model.clone())
            .unwrap_or_default()
    }

    /// Reads the model without copying it.
    pub fn with_model<R>(&self, f: impl FnOnce(&SequencerModel) -> R) -> Option<R> {
        self.borrow().map(|inner| f(&inner.model))
    }

    /// Moves the playhead (in place while the view holds still).
    pub fn set_playhead(&self, cx: &mut Cx, t: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_playhead(cx, t);
        }
    }

    pub fn playhead(&self) -> f64 {
        self.borrow().map_or(0.0, |inner| inner.playhead)
    }

    pub fn selected(&self) -> Option<u64> {
        self.borrow().and_then(|inner| inner.selected)
    }

    /// Selects item `id` (or nothing). Emits nothing.
    pub fn select(&self, cx: &mut Cx, id: Option<u64>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.select(cx, id);
        }
    }

    /// Sets pixels per second and the time at the track area's left edge.
    /// Emits nothing.
    pub fn set_zoom(&self, cx: &mut Cx, px_per_s: f64, offset: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_zoom(cx, px_per_s, offset);
        }
    }

    /// (pixels per second, the time at the track area's left edge).
    pub fn zoom(&self) -> (f64, f64) {
        self.borrow().map_or((0.0, 0.0), |inner| inner.zoom())
    }

    /// Fits the model's duration. Emits nothing.
    pub fn fit(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.fit(cx);
        }
    }

    /// Pans only if `t` is out of view (it lands 10% in). Emits nothing.
    pub fn scroll_to(&self, cx: &mut Cx, t: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.scroll_to(cx, t);
        }
    }

    /// Folds or unfolds a track. Emits nothing.
    pub fn set_expanded(&self, cx: &mut Cx, track: u64, open: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_expanded(cx, track, open);
        }
    }

    /// How many rows are shown (every ancestor open).
    pub fn visible_rows(&self) -> usize {
        self.borrow().map_or(0, |inner| inner.visible_rows())
    }

    /// Whether track `track` shows its children.
    pub fn is_expanded(&self, track: u64) -> bool {
        self.borrow().is_some_and(|inner| {
            inner
                .model
                .tracks
                .iter()
                .find(|t| t.id == track)
                .is_some_and(|t| inner.is_open(t))
        })
    }

    /// Every action this widget emitted in `actions`, in order.
    pub fn actions<'a>(&self, actions: &'a Actions) -> impl Iterator<Item = SequencerAction> + 'a {
        actions.filter_widget_actions_cast::<SequencerAction>(self.widget_uid())
    }

    /// The last scrub time in `actions`.
    pub fn scrubbed(&self, actions: &Actions) -> Option<f64> {
        self.actions(actions)
            .filter_map(|a| match a {
                SequencerAction::Scrub(t) => Some(t),
                _ => None,
            })
            .last()
    }
}
