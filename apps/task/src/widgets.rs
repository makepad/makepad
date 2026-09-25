//! The custom widgets: the history band, the metric strip, the pinned strip,
//! the process table and the inspector body.
//!
//! None of them keeps data of its own. Each reads the shared [`Model`] out of
//! the `Scope` `App` passes down on every event and draw, and renders the
//! model's ONE view sample — so scrubbing the band moves the tiles, the
//! table and the inspector together.
//!
//! Visual language (see `Theme` in main.rs for the surface roles):
//! - base: rows and inspector content; raised: bars and summaries; well:
//!   plots and grouped sections, all derived from the WM palette.
//! - IBM Plex Sans for words, the theme's monospace for every figure so
//!   columns of numbers line up, Font Awesome (already in the theme) for pin,
//!   disclosure and kind glyphs.
//! - Traces are 1.5 px lines over a soft fill of the same hue, drawn only
//!   where samples exist; recording gaps are shaded and named.
//!
//! Registered into `mod.widgets` by this module's own `script_mod!`, which
//! `App::script_mod` runs *before* the UI script_mod — a `use mod.widgets.*`
//! glob only imports what exists at the moment it runs.

use crate::backend::{tree_order, Detail, ProcDetail, ProcKey, ProcMeta, ProcState, ThreadState};
use crate::metrics::Measure;
use crate::supp::ThreadKey;
use crate::clock::LocalTime;
use crate::history::{fold_bucket_ms, fold_by_time, is_gap, Point, ProcRecord, Sample, Store, Tier, MINUTE_MS, SECOND_MS};
use crate::columns::{Figures, Graph};
use crate::model::{Column, Model, PinState, SeriesId, SERIES};
use crate::{format_bytes, format_duration_ns, with_alpha, Theme};
use makepad_widgets::*;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

mod inspect;
use inspect::{FigureCache, FileCache, Gesture, GraphCache, ThreadGraph, ThreadRowCache, ThreadTraceCache};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // A filled rounded box with an optional outline: the wells, chips,
    // pills and kind tiles. Square `DrawColor` quads cannot round a corner.
    mod.widgets.DrawTaskRound = set_type_default() do #(DrawTaskRound::script_shader(vm)) {
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let r = min(self.radius, min(self.rect_size.x, self.rect_size.y) * 0.5)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, r)
            sdf.fill_keep(self.color)
            if self.border_width > 0.0 {
                sdf.stroke(self.border_color, self.border_width)
            }
            return sdf.result
        }
    }

    // The soft fill under a trace: one quad per joined pair of points, a
    // vertical gradient anchored to the plot's own top and bottom (not to
    // each segment), so neighbouring quads meet with identical colour.
    mod.widgets.DrawTaskFill = set_type_default() do #(DrawTaskFill::script_shader(vm)) {
        ..mod.draw.DrawQuad
        pixel: fn() {
            let p = self.pos * self.rect_size
            let t = clamp((p.x - self.line_a.x) / max(self.line_b.x - self.line_a.x, 0.0001), 0.0, 1.0)
            let ly = mix(self.line_a.y, self.line_b.y, t)
            let cover = clamp(p.y - ly + 0.5, 0.0, 1.0)
            let g = clamp((p.y - self.plot_top) / max(self.plot_height, 1.0), 0.0, 1.0)
            let alpha = self.color.a * cover * (1.0 - 0.85 * g)
            return vec4(self.color.rgb * alpha, alpha)
        }
    }

    mod.widgets.SeriesLegendBase = #(SeriesLegend::register_widget(vm))
    mod.widgets.SeriesLegend = set_type_default() do mod.widgets.SeriesLegendBase{
        width: Fill
        height: 24
        draw_round: mod.widgets.DrawTaskRound{}
        draw_label +: {text_style: theme.font_regular{font_size: 10.0}}
    }

    mod.widgets.HistoryBandBase = #(HistoryBand::register_widget(vm))
    mod.widgets.HistoryBand = set_type_default() do mod.widgets.HistoryBandBase{
        width: Fill
        height: 100
        draw_round: mod.widgets.DrawTaskRound{}
        draw_fill: mod.widgets.DrawTaskFill{}
        draw_label +: {text_style: theme.font_regular{font_size: 10.0}}
        draw_mono +: {text_style: theme.font_code{font_size: 9.5}}
    }

    mod.widgets.MetricStripBase = #(MetricStrip::register_widget(vm))
    mod.widgets.MetricStrip = set_type_default() do mod.widgets.MetricStripBase{
        width: Fill
        height: 90
        draw_round: mod.widgets.DrawTaskRound{}
        draw_fill: mod.widgets.DrawTaskFill{}
        draw_label +: {text_style: theme.font_regular{font_size: 10.5}}
        draw_value +: {text_style: theme.font_code{font_size: 14.0}}
        draw_small +: {text_style: theme.font_regular{font_size: 9.0}}
    }

    mod.widgets.PinnedStripBase = #(PinnedStrip::register_widget(vm))
    mod.widgets.PinnedStrip = set_type_default() do mod.widgets.PinnedStripBase{
        width: Fill
        height: 38
        draw_round: mod.widgets.DrawTaskRound{}
        draw_fill: mod.widgets.DrawTaskFill{}
        draw_label +: {text_style: theme.font_regular{font_size: 10.5}}
        draw_mono +: {text_style: theme.font_code{font_size: 10.0}}
        draw_icon +: {text_style: theme.font_icons{font_size: 8.5}}
    }

    mod.widgets.InspectorBodyBase = #(InspectorBody::register_widget(vm))
    mod.widgets.InspectorBody = set_type_default() do mod.widgets.InspectorBodyBase{
        width: Fill
        height: Fill
        draw_round: mod.widgets.DrawTaskRound{}
        draw_fill: mod.widgets.DrawTaskFill{}
        draw_label +: {text_style: theme.font_regular{font_size: 10.5}}
        draw_mono +: {text_style: theme.font_code{font_size: 10.5}}
        draw_title +: {text_style: theme.font_bold{font_size: 15.0}}
        draw_value +: {text_style: theme.font_code{font_size: 14.0}}
        draw_small +: {text_style: theme.font_regular{font_size: 9.5}}
        draw_heading +: {text_style: theme.font_bold{font_size: 10.5}}
        draw_icon +: {text_style: theme.font_icons{font_size: 9.0}}
        draw_letter +: {text_style: theme.font_bold{font_size: 12.0}}
    }

    mod.widgets.ProcessTableBase = #(ProcessTable::register_widget(vm))
    mod.widgets.ProcessTable = set_type_default() do mod.widgets.ProcessTableBase{
        width: Fill
        height: Fill
        flow: Down
        draw_round: mod.widgets.DrawTaskRound{}
        draw_fill: mod.widgets.DrawTaskFill{}
        draw_name +: {text_style: theme.font_regular{font_size: 11.5}}
        draw_icon +: {text_style: theme.font_icons{font_size: 9.0}}
        draw_letter +: {text_style: theme.font_bold{font_size: 8.5}}
        draw_state +: {text_style: theme.font_regular{font_size: 10.5}}
        process_grid := DataGrid{
            width: Fill
            height: Fill
            rows: 0
            cols: 10
            show_row_headers: false
            // A list, not a spreadsheet: a press or an arrow picks the whole
            // row, and a heading press only sorts. In the default Cells mode
            // a heading press selects its column, which reads as a row pick.
            selection: GridSelectMode.Rows
            zebra_stripes: false
            allow_col_resize: true
            allow_row_resize: false
            allow_col_reorder: true
            default_col_width: 80.0
            default_row_height: 30.0
            col_header_height: 28.0
            header_align: 0.0
            cell_pad_x: 8.0
            color_bg: #x1b1c1f
            color_cell: #x1b1c1f
            color_cell_alt: #x1b1c1f
            color_text: #xe3e4e6
            color_header: #x222326
            color_header_active: #x222326
            color_header_text: #x9a9ca3
            color_selection: #x4f9dff2e
            color_selection_border: #x4f9dff00
            color_drag_marker: #x4f9dff
            color_resize_guide: #x4f9dff66
            draw_cell +: {border_color: #x2a2b2f border_size: 0.0}
            // Names in the UI face; every figure in the theme's monospace
            // through the grid's second text slot, so digits line up. The
            // headings have their own slot and stay in the UI face.
            draw_text +: {text_style: theme.font_regular{font_size: 11.5}}
            draw_text_bold +: {text_style: theme.font_code{font_size: 10.5}}
            draw_text_header +: {text_style: theme.font_regular{font_size: 10.5}}
        }
    }
}

// ---- the rounded-box shader ----

/// Instance fields after the draw base, as the instance buffer reads them:
/// the fill's colour, the trace segment's two ends and the plot's top and
/// height, all in the quad's own pixels.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTaskFill {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub line_a: Vec2f,
    #[live]
    pub line_b: Vec2f,
    #[live]
    pub plot_top: f32,
    #[live]
    pub plot_height: f32,
}

/// Instance fields after the draw base, as the instance buffer reads them.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTaskRound {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub border_color: Vec4f,
    #[live(4.0)]
    pub radius: f32,
    #[live]
    pub border_width: f32,
}

// ---- glyphs (Font Awesome 5 solid, already carried by the theme) ----

const ICON_PIN: &str = "\u{f08d}";
const ICON_CHEVRON_LEFT: &str = "\u{f053}";
const ICON_CHEVRON_RIGHT: &str = "\u{f054}";
const ICON_CHEVRON_DOWN: &str = "\u{f078}";
const ICON_TERMINAL: &str = "\u{f120}";
const ICON_CLOSE: &str = "\u{f00d}";

// ---- drawing helpers ----

fn inside(rect: Rect, pos: Vec2d) -> bool {
    pos.x >= rect.pos.x && pos.y >= rect.pos.y && pos.x < rect.pos.x + rect.size.x && pos.y < rect.pos.y + rect.size.y
}

fn text_width(text_draw: &DrawText, cx: &mut Cx2d, text: &str) -> f64 {
    text_draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text).size_in_lpxs.width as f64
}

fn text_height(text_draw: &DrawText, cx: &mut Cx2d, text: &str) -> f64 {
    text_draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text).size_in_lpxs.height as f64
}

/// Text ending at `right`.
fn text_right(text_draw: &mut DrawText, cx: &mut Cx2d, right: f64, y: f64, text: &str) {
    let width = text_width(text_draw, cx, text);
    text_draw.draw_abs(cx, dvec2(right - width, y), text);
}

/// Text centred vertically on `mid_y`.
fn text_mid(text_draw: &mut DrawText, cx: &mut Cx2d, x: f64, mid_y: f64, text: &str) {
    let height = text_height(text_draw, cx, text);
    text_draw.draw_abs(cx, dvec2(x, mid_y - height * 0.5), text);
}

/// Text cut to `max_width` with an ellipsis, measured in the face drawn.
fn text_fit(text_draw: &mut DrawText, cx: &mut Cx2d, pos: Vec2d, max_width: f64, text: &str) {
    if max_width <= 4.0 {
        return;
    }
    if text_width(text_draw, cx, text) <= max_width {
        text_draw.draw_abs(cx, pos, text);
        return;
    }
    // Binary search on the character count: long command lines and paths
    // are measured O(log n) times, not once per dropped character.
    let bounds: Vec<usize> = text.char_indices().map(|(at, _)| at).chain(std::iter::once(text.len())).collect();
    let (mut low, mut high) = (0usize, bounds.len() - 1);
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate = format!("{}…", &text[..bounds[mid]]);
        if text_width(text_draw, cx, &candidate) <= max_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    if low > 0 {
        text_draw.draw_abs(cx, pos, &format!("{}…", &text[..bounds[low]]));
    }
}

fn round(draw: &mut DrawTaskRound, cx: &mut Cx2d, rect: Rect, color: Vec4f, radius: f32) {
    draw.color = color;
    draw.border_width = 0.0;
    draw.radius = radius;
    draw.draw_abs(cx, rect);
}

fn round_outline(draw: &mut DrawTaskRound, cx: &mut Cx2d, rect: Rect, fill: Vec4f, border: Vec4f, radius: f32) {
    draw.color = fill;
    draw.border_color = border;
    draw.border_width = 1.0;
    draw.radius = radius;
    draw.draw_abs(cx, rect);
}

/// One anti-aliased segment through `DrawChartSegment` (the shader
/// `widgets/src/chart.rs` registers).
fn segment(seg: &mut DrawChartSegment, cx: &mut Cx2d, a: Vec2d, b: Vec2d) {
    let margin = seg.thickness as f64 + 2.0;
    let pos = dvec2(a.x.min(b.x) - margin, a.y.min(b.y) - margin);
    let size = dvec2((a.x - b.x).abs() + 2.0 * margin, (a.y - b.y).abs() + 2.0 * margin);
    seg.seg_a = Vec2f { x: (a.x - pos.x) as f32, y: (a.y - pos.y) as f32 };
    seg.seg_b = Vec2f { x: (b.x - pos.x) as f32, y: (b.y - pos.y) as f32 };
    seg.draw_abs(cx, Rect { pos, size });
}

/// How a trace is drawn: line colour and width, and the fill under it.
#[derive(Clone, Copy)]
struct Trace {
    color: Vec4f,
    width: f32,
    /// Fill alpha under the line; 0 for a bare line.
    fill: f32,
}

/// Draw points at their own timestamps against a fractional window: x is
/// `(time - from) / (to - from)` of the plot, so as the window's edge moves
/// every sample glides by the same sub-pixel amount instead of being
/// re-binned. Consecutive points join unless the second follows a recorded
/// gap; a point with no neighbour becomes a short tick. The soft fill runs
/// only under joined stretches. `points` are already folded (see
/// `fold_by_time`) and sorted by time.
#[allow(clippy::too_many_arguments)]
fn draw_trace(seg: &mut DrawChartSegment, fill: &mut DrawTaskFill, cx: &mut Cx2d, rect: Rect, points: &[Point], window: (f64, f64), max: f32, trace: Trace) {
    draw_trace_range(seg, fill, cx, rect, points, window, 0.0, max, trace);
}

/// [`draw_trace`] over a value range `min..max`, for figures that can be
/// negative (nice, priority); the fill then runs down to the plot's bottom.
///
/// The fill tiles the plot with one quad per joined pair whose x edges are
/// snapped to physical pixels by the same rounding on both sides of a
/// shared boundary: every pixel column is covered exactly once, however
/// close two points are (folded peaks keep their real timestamps, so two
/// can sit a fraction of a pixel apart — skipping those left unfilled
/// columns that showed as dark vertical seams).
#[allow(clippy::too_many_arguments)]
fn draw_trace_range(seg: &mut DrawChartSegment, fill: &mut DrawTaskFill, cx: &mut Cx2d, rect: Rect, points: &[Point], window: (f64, f64), min: f32, max: f32, trace: Trace) {
    if points.is_empty() || rect.size.x < 2.0 || rect.size.y < 2.0 {
        return;
    }
    let (from, to) = window;
    let span = (to - from).max(1.0);
    let min = min as f64;
    let range = (max as f64 - min).max(f32::EPSILON as f64);
    let bottom = rect.pos.y + rect.size.y;
    let at = |point: &Point| {
        dvec2(
            rect.pos.x + (point.time_ms as f64 - from) / span * rect.size.x,
            bottom - ((point.value as f64 - min) / range).clamp(0.0, 1.0) * rect.size.y,
        )
    };
    // Only the points on screen, plus one neighbour each side so lines
    // run to the edges (the caller clips).
    let start = points.partition_point(|p| (p.time_ms as f64) < from).saturating_sub(1);
    let end = (points.partition_point(|p| (p.time_ms as f64) <= to) + 1).min(points.len());
    let visible = &points[start..end.max(start)];
    if trace.fill > 0.0 {
        let dpi = cx.current_dpi_factor().max(1.0);
        let snap = |x: f64| (x * dpi).round() / dpi;
        fill.color = with_alpha(trace.color, trace.fill);
        for pair in visible.windows(2) {
            if pair[1].gap_before {
                continue;
            }
            let (a, b) = (at(&pair[0]), at(&pair[1]));
            let (x0, x1) = (snap(a.x), snap(b.x));
            if x1 <= x0 {
                continue;
            }
            let top = a.y.min(b.y).floor() - 1.0;
            let quad = Rect { pos: dvec2(x0, top), size: dvec2(x1 - x0, (bottom - top).max(0.0)) };
            fill.line_a = Vec2f { x: (a.x - x0) as f32, y: (a.y - top) as f32 };
            fill.line_b = Vec2f { x: (b.x - x0) as f32, y: (b.y - top) as f32 };
            fill.plot_top = (rect.pos.y - top) as f32;
            fill.plot_height = rect.size.y as f32;
            fill.draw_abs(cx, quad);
        }
    }
    seg.mode = 0.0;
    seg.color = trace.color;
    seg.thickness = trace.width;
    for (n, point) in visible.iter().enumerate() {
        let here = at(point);
        let joined = n > 0 && !point.gap_before;
        if joined {
            segment(seg, cx, at(&visible[n - 1]), here);
        }
        let next_joins = visible.get(n + 1).is_some_and(|next| !next.gap_before);
        if !joined && !next_joins {
            segment(seg, cx, here - dvec2(1.0, 0.0), here + dvec2(1.0, 0.0));
        }
    }
}

/// Fold points for a plot `px` wide showing `span_ms`.
fn fold_for(points: &[Point], span_ms: f64, px: f64) -> Vec<Point> {
    fold_by_time(points, fold_bucket_ms(span_ms, px))
}

fn peak(points: &[Point]) -> f32 {
    points.iter().fold(0.0f32, |peak, point| peak.max(point.value))
}

/// A 1-pixel rule.
fn hline(draw: &mut DrawColor, cx: &mut Cx2d, x: f64, y: f64, width: f64, color: Vec4f) {
    draw.color = color;
    draw.draw_abs(cx, Rect { pos: dvec2(x, y), size: dvec2(width, 1.0) });
}

fn vline(draw: &mut DrawColor, cx: &mut Cx2d, x: f64, y: f64, height: f64, color: Vec4f) {
    draw.color = color;
    draw.draw_abs(cx, Rect { pos: dvec2(x, y), size: dvec2(1.0, height) });
}

fn fill(draw: &mut DrawColor, cx: &mut Cx2d, rect: Rect, color: Vec4f) {
    draw.color = color;
    draw.draw_abs(cx, rect);
}

fn local_hms(ms: u64) -> String {
    LocalTime::from_epoch_ms(ms).hms()
}

/// How close to a graph's right edge (px) the dragged cursor starts
/// following time forward.
const EDGE_PX: f64 = 14.0;

/// A left drag of the shared cursor on a graph — the history band and the
/// inspector's large graphs share it, so their arithmetic cannot diverge.
///
/// The cursor maps through the model's own (frozen) window, so it never
/// drifts from what is drawn. Held at the right edge it pans the window
/// forward on its own frames, at most half a window a second (slower near
/// the edge), mapping the cursor through the moved window; when that
/// reaches the newest recorded sample the view goes live and the press is
/// spent: later moves and the release do nothing, so nothing jumps.
/// Dragging forward onto the newest sample at the edge does the same. A
/// click, a drag within the window, a right-drag pan and the wheel are
/// unchanged.
#[derive(Default)]
pub struct CursorDrag {
    active: bool,
    rect: Rect,
    last_x: f64,
    following: bool,
    frame: NextFrame,
    frame_time: f64,
    /// The press reached the present and went live.
    latched: bool,
}

/// The time under `x` in a plot `rect` showing `window`, clamped to it.
fn time_under(rect: Rect, window: (f64, f64), x: f64) -> u64 {
    let t = ((x - rect.pos.x) / rect.size.x.max(1.0)).clamp(0.0, 1.0);
    (window.0 + (window.1 - window.0) * t).max(0.0) as u64
}

impl CursorDrag {
    pub fn begin(&mut self, model: &mut Model, rect: Rect, x: f64) {
        *self = CursorDrag { active: true, rect, last_x: x, ..CursorDrag::default() };
        let window = model.window_f();
        model.scrub_to(time_under(rect, window, x));
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Whether this press is still claimed (dragging, or spent by going
    /// live), so its moves and release are not taken as anything else.
    pub fn claims(&self) -> bool {
        self.active || self.latched
    }

    pub fn moved(&mut self, cx: &mut Cx, model: &mut Model, x: f64) {
        if !self.active {
            return;
        }
        let forward = x > self.last_x;
        self.last_x = x;
        let right = self.rect.pos.x + self.rect.size.x;
        let window = model.window_f();
        model.scrub_to(time_under(self.rect, window, x));
        let at_edge = x >= right - EDGE_PX;
        if forward && at_edge && model.store.newest_ms() == Some(model.cursor_ms) {
            self.reach_present(model);
            return;
        }
        if at_edge && !self.following {
            self.following = true;
            self.frame_time = 0.0;
            self.frame = cx.new_next_frame();
        } else if !at_edge {
            self.following = false;
        }
    }

    /// A frame while held at the edge: move the window forward. Returns
    /// whether anything changed.
    pub fn frame(&mut self, cx: &mut Cx, event: &Event, model: &mut Model) -> bool {
        let Some(tick) = self.frame.is_event(event) else { return false };
        if !(self.active && self.following) {
            return false;
        }
        let dt = if self.frame_time > 0.0 { (tick.time - self.frame_time).clamp(0.0, 0.1) } else { 1.0 / 60.0 };
        self.frame_time = tick.time;
        let Some(newest) = model.store.newest_ms() else {
            self.following = false;
            return false;
        };
        let (from, to) = model.window_f();
        let span = (to - from).max(1.0);
        let right = self.rect.pos.x + self.rect.size.x;
        let depth = ((self.last_x - (right - EDGE_PX)) / (EDGE_PX * 3.0)).clamp(0.15, 1.0);
        let end = to + (span * 0.5 * depth * dt).min(span * 0.05);
        if end >= newest as f64 {
            self.reach_present(model);
            return true;
        }
        model.pan_to(end);
        let window = model.window_f();
        model.scrub_to(time_under(self.rect, window, self.last_x));
        if model.cursor_ms >= newest {
            self.reach_present(model);
            return true;
        }
        self.frame = cx.new_next_frame();
        true
    }

    fn reach_present(&mut self, model: &mut Model) {
        model.go_live();
        self.active = false;
        self.following = false;
        self.latched = true;
    }

    /// The press ended (or its graph went away).
    pub fn end(&mut self) {
        self.active = false;
        self.following = false;
        self.latched = false;
    }
}

/// The zoom factor for one wheel or trackpad scroll step: down zooms out,
/// up zooms in, bounded per event so a fast wheel or a trackpad fling
/// cannot leap.
fn wheel_zoom_factor(scroll_y: f64) -> f64 {
    if scroll_y.abs() < 0.01 {
        return 1.0;
    }
    (scroll_y * 0.004).exp().clamp(0.8, 1.25)
}

/// A time-axis step that puts at most `max_ticks` labels in `span_ms`.
fn tick_step(span_ms: u64, max_ticks: f64) -> u64 {
    const STEPS: [u64; 17] = [
        SECOND_MS, 2 * SECOND_MS, 5 * SECOND_MS, 10 * SECOND_MS, 15 * SECOND_MS, 30 * SECOND_MS,
        MINUTE_MS, 2 * MINUTE_MS, 5 * MINUTE_MS, 10 * MINUTE_MS, 15 * MINUTE_MS, 30 * MINUTE_MS,
        60 * MINUTE_MS, 2 * 60 * MINUTE_MS, 3 * 60 * MINUTE_MS, 6 * 60 * MINUTE_MS, 12 * 60 * MINUTE_MS,
    ];
    STEPS
        .iter()
        .copied()
        .find(|step| (span_ms as f64 / *step as f64) <= max_ticks.max(1.0))
        .unwrap_or(24 * 60 * MINUTE_MS)
}

/// A process' identity glyph: an application gets a tile with its initial,
/// anything else the terminal mark. Real icons need an OS icon lookup this
/// app does not have; this is the consistent neutral fallback.
#[allow(clippy::too_many_arguments)]
fn kind_glyph(round_draw: &mut DrawTaskRound, letter: &mut DrawText, icon: &mut DrawText, cx: &mut Cx2d, theme: &Theme, meta: &ProcMeta, center: Vec2d, size: f64) {
    if meta.is_app {
        let tile = Rect { pos: center - dvec2(size * 0.5, size * 0.5), size: dvec2(size, size) };
        round(round_draw, cx, tile, theme.well_strong(), (size * 0.26) as f32);
        let initial: String = meta.name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "·".to_string());
        letter.color = theme.foreground;
        let w = text_width(letter, cx, &initial);
        text_mid(letter, cx, center.x - w * 0.5, center.y, &initial);
    } else {
        icon.color = theme.tertiary();
        let w = text_width(icon, cx, ICON_TERMINAL);
        text_mid(icon, cx, center.x - w * 0.5, center.y, ICON_TERMINAL);
    }
}

/// A thin scroll thumb along the right edge of `view` when content overflows.
fn scroll_thumb(round_draw: &mut DrawTaskRound, cx: &mut Cx2d, theme: &Theme, view: Rect, content: f64, scroll: f64) {
    if content <= view.size.y + 1.0 || view.size.y < 24.0 {
        return;
    }
    let track = Rect { pos: dvec2(view.pos.x + view.size.x - 6.0, view.pos.y + 4.0), size: dvec2(3.0, view.size.y - 8.0) };
    let thumb_h = (track.size.y * view.size.y / content).max(18.0);
    let travel = track.size.y - thumb_h;
    let t = (scroll / (content - view.size.y)).clamp(0.0, 1.0);
    round(round_draw, cx, track, with_alpha(theme.foreground, 0.05), 1.5);
    round(round_draw, cx, Rect { pos: dvec2(track.pos.x, track.pos.y + travel * t), size: dvec2(3.0, thumb_h) }, with_alpha(theme.foreground, 0.30), 1.5);
}

// ---- SeriesLegend ----

const CHIP_H: f64 = 20.0;
const CHIP_GAP: f64 = 6.0;
const CHIP_ROW: f64 = 24.0;

/// The series chips above the history plot: a colour dot and a name each,
/// filled when shown and outlined when hidden; a click toggles the series.
/// They wrap onto more rows when the space is narrow, so every series stays
/// reachable; [`SeriesLegend::rows_for`] tells the layout how tall to be.
#[derive(Script, ScriptHook, Widget)]
pub struct SeriesLegend {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_round: DrawTaskRound,
    #[live]
    draw_label: DrawText,
    #[rust]
    chips: Vec<(usize, Rect)>,
}

impl SeriesLegend {
    pub fn set_height(&mut self, height: f64) {
        self.walk.height = Size::Fixed(height);
    }

    /// Rows the chips need in `width`, from an estimate of their widths
    /// (10 pt label + dot and padding), for the layout pass.
    pub fn rows_for(width: f64) -> usize {
        let mut rows = 1;
        let mut x = 0.0;
        for id in SERIES.iter() {
            let chip = id.label().len() as f64 * 6.2 + 26.0;
            if x > 0.0 && x + chip > width {
                rows += 1;
                x = 0.0;
            }
            x += chip + CHIP_GAP;
        }
        rows
    }
}

impl Widget for SeriesLegend {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let Some(model) = scope.data.get_mut::<Model>() else { return };
        if let Hit::FingerDown(e) = event.hits(cx, self.draw_bg.area()) {
            if let Some((index, _)) = self.chips.iter().find(|(_, rect)| inside(*rect, e.abs)) {
                model.series_on[*index] = !model.series_on[*index];
                model.changed();
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let Some(model) = scope.data.get::<Model>() else { return DrawStep::done() };
        let theme = model.theme;
        fill(&mut self.draw_bg, cx, rect, with_alpha(theme.background, 0.0));
        let view = model.view_sample().cloned();
        self.chips.clear();
        let mut x = rect.pos.x;
        let mut y = rect.pos.y + (CHIP_ROW - CHIP_H) * 0.5;
        for (index, id) in SERIES.iter().enumerate() {
            let on = model.series_on[index];
            let available = view.as_ref().map(|sample| id.value(sample).is_some()).unwrap_or(true);
            let label = if available { id.label().to_string() } else { format!("{} n/a", id.label()) };
            let width = text_width(&self.draw_label, cx, &label) + 26.0;
            if x + width > rect.pos.x + rect.size.x && x > rect.pos.x {
                x = rect.pos.x;
                y += CHIP_ROW;
            }
            let chip = Rect { pos: dvec2(x, y), size: dvec2(width, CHIP_H) };
            let color = id.color(&theme);
            if on {
                round(&mut self.draw_round, cx, chip, with_alpha(color, 0.16), 10.0);
            } else {
                round_outline(&mut self.draw_round, cx, chip, with_alpha(theme.background, 0.0), theme.rule(), 10.0);
            }
            round(&mut self.draw_round, cx, Rect { pos: chip.pos + dvec2(8.0, 6.5), size: dvec2(7.0, 7.0) }, if on { color } else { with_alpha(color, 0.45) }, 3.5);
            self.draw_label.color = if on && available { theme.foreground } else { theme.secondary() };
            text_mid(&mut self.draw_label, cx, chip.pos.x + 19.0, chip.pos.y + CHIP_H * 0.5, &label);
            self.chips.push((index, chip));
            x += width + CHIP_GAP;
        }
        DrawStep::done()
    }
}

// ---- HistoryBand ----

/// What the band keeps between frames: the folded traces, their scales and
/// the recorded gaps, rebuilt only when the stored data, the shown series,
/// the range, the plot width or a frozen (scrubbed) window change. Between
/// those the live window only slides, so a frame is translation alone.
#[derive(Default)]
struct BandCache {
    key: Option<(u64, usize, [bool; SERIES.len()], u64, Option<u64>)>,
    series: Vec<(usize, Vec<Point>)>,
    disk_max: f32,
    net_max: f32,
    gaps: Vec<(u64, u64)>,
    /// The first retained time in the cached span, and whether it is the
    /// oldest sample of all (then the stretch before it is "no history").
    first: Option<(u64, bool)>,
}

/// The history plot: lines at their real timestamps against a window whose
/// live edge moves continuously (so traces glide rather than hop a sample at
/// a time), a soft fill under the lead series, recording gaps shaded and
/// named, a local time axis, a hover readout and the scrub cursor. Drag or
/// click to put every view on a past sample; arrows step (shift ×10), Home
/// goes to the oldest, End/Escape return to Live. The chips, range, stepping
/// and Live live around it in main.rs.
#[derive(Script, ScriptHook, Widget)]
pub struct HistoryBand {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_rect: DrawColor,
    #[live]
    draw_round: DrawTaskRound,
    #[live]
    draw_seg: DrawChartSegment,
    #[live]
    draw_fill: DrawTaskFill,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_mono: DrawText,
    #[rust]
    plot: Rect,
    #[rust]
    window: (f64, f64),
    #[rust]
    cursor: CursorDrag,
    /// A right-button pan: where it began and the window drawn then.
    #[rust]
    pan: Option<(f64, (f64, f64))>,
    #[rust]
    cache: BandCache,
}

impl HistoryBand {
    pub fn set_height(&mut self, height: Size) {
        self.walk.height = height;
    }

    fn time_at(&self, x: f64) -> u64 {
        let (from, to) = self.window;
        if self.plot.size.x <= 0.0 {
            return to.max(0.0) as u64;
        }
        let t = ((x - self.plot.pos.x) / self.plot.size.x).clamp(0.0, 1.0);
        (from + (to - from) * t).max(0.0) as u64
    }

    fn x_at(&self, time: f64) -> f64 {
        let (from, to) = self.window;
        self.plot.pos.x + (time - from) / (to - from).max(1.0) * self.plot.size.x
    }

    /// Shade a stretch without samples and name it when there is room.
    fn shade_gap(&mut self, cx: &mut Cx2d, theme: &Theme, from_x: f64, to_x: f64, label: &str) {
        let plot = self.plot;
        let x0 = from_x.max(plot.pos.x);
        let x1 = to_x.min(plot.pos.x + plot.size.x);
        if x1 - x0 < 2.0 {
            return;
        }
        fill(&mut self.draw_rect, cx, Rect { pos: dvec2(x0, plot.pos.y), size: dvec2(x1 - x0, plot.size.y) }, with_alpha(theme.background, 0.55));
        let w = text_width(&self.draw_label, cx, label);
        if x1 - x0 > w + 16.0 {
            self.draw_label.color = theme.tertiary();
            text_mid(&mut self.draw_label, cx, (x0 + x1) * 0.5 - w * 0.5, plot.pos.y + plot.size.y * 0.5, label);
        }
    }

    /// Rebuild the cached traces when the data or the view changed.
    fn refresh_cache(&mut self, model: &Model) {
        let (from, to) = self.window;
        let span = to - from;
        let bucket = fold_bucket_ms(span, self.plot.size.x);
        let frozen = if model.live { None } else { Some(to.max(0.0) as u64) };
        let key = (model.store.generation, model.range, model.series_on, bucket, frozen);
        if self.cache.key == Some(key) {
            return;
        }
        // A margin before the window so the live edge can slide until the
        // next sample without leaving the cached span; after it, everything
        // up to the newest sample.
        let lo = (from - span * 0.1 - bucket as f64).max(0.0) as u64;
        let hi = if model.live { model.store.newest_ms().unwrap_or(to as u64) } else { to.max(0.0) as u64 + bucket };
        let mut series = Vec::new();
        for (index, id) in SERIES.iter().enumerate() {
            if model.series_on[index] {
                let points = model.store.series(lo, hi, |sample| id.value(sample));
                series.push((index, fold_by_time(&points, bucket)));
            }
        }
        let pair_peak = |a: SeriesId, b: SeriesId| {
            series
                .iter()
                .filter(|(index, _)| SERIES[*index] == a || SERIES[*index] == b)
                .fold(0.0f32, |acc, (_, points)| acc.max(peak(points)))
                * 1.1
        };
        let disk_max = pair_peak(SeriesId::DiskRead, SeriesId::DiskWrite).max(1024.0);
        let net_max = pair_peak(SeriesId::NetIn, SeriesId::NetOut).max(1024.0);
        let range = model.store.range_indices(lo, hi);
        let first = model.store.entry(range.start).map(|entry| (entry.sample.time_ms, model.store.oldest_ms() == Some(entry.sample.time_ms)));
        let mut gaps = Vec::new();
        for index in range.start + 1..range.end {
            if let (Some(previous), Some(next)) = (model.store.entry(index - 1), model.store.entry(index)) {
                if is_gap(previous, next) {
                    gaps.push((previous.sample.time_ms, next.sample.time_ms));
                }
            }
        }
        self.cache = BandCache { key: Some(key), series, disk_max, net_max, gaps, first };
    }
}

impl Widget for HistoryBand {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let Some(model) = scope.data.get_mut::<Model>() else { return };
        if self.cursor.frame(cx, event, model) {
            self.draw_bg.redraw(cx);
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let hover = inside(self.plot, e.abs).then(|| self.time_at(e.abs.x));
                if hover != model.hover_ms {
                    model.hover_ms = hover;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if model.hover_ms.take().is_some() {
                    self.draw_bg.redraw(cx);
                }
            }
            // Left drag moves the cursor; right drag pans the window; the
            // wheel zooms about the pointer. Both of the latter leave live
            // with the cursor where it was.
            Hit::FingerDown(e) => {
                cx.set_key_focus(self.draw_bg.area());
                if inside(self.plot, e.abs) {
                    if e.device.mouse_button().is_some_and(|button| button.is_secondary()) {
                        model.freeze_view();
                        self.pan = Some((e.abs.x, self.window));
                    } else if e.is_primary_hit() {
                        self.cursor.begin(model, self.plot, e.abs.x);
                    }
                }
            }
            Hit::FingerMove(e) => {
                if let Some((start_x, window)) = self.pan {
                    let shift = (e.abs.x - start_x) / self.plot.size.x.max(1.0) * (window.1 - window.0);
                    model.pan_to(window.1 - shift);
                    self.draw_bg.redraw(cx);
                } else if self.cursor.is_active() {
                    self.cursor.moved(cx, model, e.abs.x);
                    model.hover_ms = (!model.live).then_some(model.cursor_ms);
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerUp(_) => {
                self.cursor.end();
                self.pan = None;
            }
            Hit::FingerScroll(e) if inside(self.plot, e.abs) && self.pan.is_none() && !self.cursor.claims() => {
                let factor = wheel_zoom_factor(e.scroll.y);
                if factor != 1.0 {
                    let t = ((e.abs.x - self.plot.pos.x) / self.plot.size.x.max(1.0)).clamp(0.0, 1.0);
                    let anchor = self.window.0 + (self.window.1 - self.window.0) * t;
                    model.zoom(anchor, factor, self.window);
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::KeyDown(key) => match key.key_code {
                KeyCode::ArrowLeft => model.step(if key.modifiers.shift { -10 } else { -1 }),
                KeyCode::ArrowRight => model.step(if key.modifiers.shift { 10 } else { 1 }),
                KeyCode::Home => model.go_oldest(),
                KeyCode::End | KeyCode::Escape => model.go_live(),
                _ => {}
            },
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let Some(model) = scope.data.get::<Model>() else { return DrawStep::done() };
        let theme = model.theme;
        fill(&mut self.draw_bg, cx, rect, theme.background);
        if rect.size.x < 80.0 || rect.size.y < 40.0 {
            return DrawStep::done();
        }
        self.window = model.window_f();
        let (from, to) = self.window;
        let pad = 12.0;
        let axis_h = 18.0;
        self.plot = Rect {
            pos: dvec2(rect.pos.x + pad, rect.pos.y + 2.0),
            size: dvec2((rect.size.x - 2.0 * pad).max(10.0), (rect.size.y - axis_h - 2.0).max(10.0)),
        };
        let plot = self.plot;

        // The well, with quiet quarter rules.
        round(&mut self.draw_round, cx, plot, theme.well(), 6.0);
        for quarter in [0.25, 0.5, 0.75] {
            hline(&mut self.draw_rect, cx, plot.pos.x + 6.0, plot.pos.y + plot.size.y * quarter, plot.size.x - 12.0, with_alpha(theme.foreground, 0.05));
        }
        if to <= from || model.store.is_empty() {
            self.draw_label.color = theme.tertiary();
            text_mid(&mut self.draw_label, cx, plot.pos.x + 12.0, plot.pos.y + plot.size.y * 0.5, "Waiting for the first samples…");
            return DrawStep::done();
        }
        self.refresh_cache(model);

        cx.push_clip_rect(plot);
        // Where nothing was recorded: before the retained history begins,
        // and between sessions or across a stall. Shaded and named, never
        // bridged.
        if let Some((first, oldest)) = self.cache.first {
            if first as f64 > from {
                let (x0, x1) = (self.x_at(from), self.x_at(first as f64));
                self.shade_gap(cx, &theme, x0, x1, if oldest { "no history yet" } else { "not recording" });
            }
        }
        let gaps = std::mem::take(&mut self.cache.gaps);
        for (start, end) in &gaps {
            if (*end as f64) >= from && (*start as f64) <= to {
                let (x0, x1) = (self.x_at(*start as f64), self.x_at(*end as f64));
                self.shade_gap(cx, &theme, x0, x1, "not recording");
            }
        }
        self.cache.gaps = gaps;

        // Series: percentages on 0..100; rates share a scale with their pair
        // (fixed per data update, so it never jitters between frames). Only
        // the first shown series is filled, so traces never turn to mud.
        let inset = Rect { pos: plot.pos + dvec2(0.0, 4.0), size: plot.size - dvec2(0.0, 6.0) };
        let series = std::mem::take(&mut self.cache.series);
        for (n, (index, points)) in series.iter().enumerate() {
            let id = SERIES[*index];
            let max = match id {
                SeriesId::DiskRead | SeriesId::DiskWrite => self.cache.disk_max,
                SeriesId::NetIn | SeriesId::NetOut => self.cache.net_max,
                _ => 100.0,
            };
            let trace = Trace { color: id.color(&theme), width: 1.5, fill: if n == 0 { 0.20 } else { 0.0 } };
            draw_trace(&mut self.draw_seg, &mut self.draw_fill, cx, inset, points, (from, to), max, trace);
        }
        self.cache.series = series;
        cx.pop_clip_rect();

        // A zoomed span has no preset to name it: say it on the plot.
        if model.custom_span_ms.is_some() {
            let label = format!("span {}", crate::persist::format_span_ms(model.range_ms()));
            self.draw_label.color = theme.tertiary();
            let w = text_width(&self.draw_label, cx, &label);
            self.draw_label.draw_abs(cx, dvec2(plot.pos.x + plot.size.x - w - 6.0, plot.pos.y + 3.0), &label);
        }
        // Time axis in the figure face; ticks ride the same fractional
        // window, so they slide with the data.
        let axis_y = plot.pos.y + plot.size.y + 4.0;
        let span = (to - from).max(1.0) as u64;
        let step = tick_step(span, plot.size.x / 90.0);
        let mut tick = (from.max(0.0) as u64).div_ceil(step) * step;
        self.draw_mono.color = theme.tertiary();
        while (tick as f64) <= to {
            let tx = self.x_at(tick as f64);
            let label = if step < MINUTE_MS { local_hms(tick) } else { LocalTime::from_epoch_ms(tick).hm() };
            let w = text_width(&self.draw_mono, cx, &label);
            if tx - w * 0.5 > plot.pos.x && tx + w * 0.5 < plot.pos.x + plot.size.x {
                self.draw_mono.draw_abs(cx, dvec2(tx - w * 0.5, axis_y), &label);
            }
            tick += step;
        }

        // Hover hairline and a grouped readout.
        if let Some(hover) = model.hover_ms.filter(|t| (*t as f64) >= from && (*t as f64) <= to) {
            let hx = self.x_at(hover as f64);
            vline(&mut self.draw_rect, cx, hx, plot.pos.y, plot.size.y, with_alpha(theme.foreground, 0.35));
            if let Some(sample) = model.store.at(hover) {
                let mut text = local_hms(sample.time_ms);
                for (index, id) in SERIES.iter().enumerate() {
                    if model.series_on[index] {
                        if let Some(value) = id.value(sample) {
                            text.push_str(&format!("   {} {}", id.label(), id.format(value)));
                        }
                    }
                }
                let w = text_width(&self.draw_mono, cx, &text) + 16.0;
                let bx = if hx + w + 8.0 < plot.pos.x + plot.size.x { hx + 8.0 } else { (hx - w - 8.0).max(plot.pos.x) };
                let box_rect = Rect { pos: dvec2(bx, plot.pos.y + 6.0), size: dvec2(w, 20.0) };
                round_outline(&mut self.draw_round, cx, box_rect, theme.raised(), theme.rule(), 5.0);
                self.draw_mono.color = theme.foreground;
                text_mid(&mut self.draw_mono, cx, box_rect.pos.x + 8.0, box_rect.pos.y + 10.0, &text);
            }
        }

        // The committed cursor: a hairline and a time pill on the axis, with
        // the retained resolution when the sample was thinned.
        if !model.live {
            let x = self.x_at(model.cursor_ms as f64);
            if x >= plot.pos.x - 1.0 && x <= plot.pos.x + plot.size.x + 1.0 {
                fill(&mut self.draw_rect, cx, Rect { pos: dvec2(x - 0.75, plot.pos.y), size: dvec2(1.5, plot.size.y) }, theme.accent);
                let tier = model.view_tier();
                let label = if tier == Tier::Raw { local_hms(model.cursor_ms) } else { format!("{} · {}", local_hms(model.cursor_ms), tier.label()) };
                let w = text_width(&self.draw_mono, cx, &label) + 14.0;
                let lx = (x - w * 0.5).clamp(plot.pos.x, plot.pos.x + plot.size.x - w);
                let pill = Rect { pos: dvec2(lx, axis_y - 3.0), size: dvec2(w, 17.0) };
                round(&mut self.draw_round, cx, pill, theme.accent, 8.5);
                self.draw_mono.color = theme.background;
                text_mid(&mut self.draw_mono, cx, pill.pos.x + 7.0, pill.pos.y + 8.5, &label);
            }
        }
        DrawStep::done()
    }
}

// ---- MetricStrip ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tile {
    Cpu,
    Gpu,
    Memory,
    Disk,
    Network,
    Power,
}

/// Equal tiles across the window — CPU, GPU, memory, disk, network, and
/// power only where the OS measures it — each a title, a large figure at the
/// view time and a filled 60 s trace ending at the view time. A figure the
/// OS does not give us says "Unavailable" instead of a line.
#[derive(Script, ScriptHook, Widget)]
pub struct MetricStrip {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_rect: DrawColor,
    #[live]
    draw_round: DrawTaskRound,
    #[live]
    draw_seg: DrawChartSegment,
    #[live]
    draw_fill: DrawTaskFill,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_value: DrawText,
    #[live]
    draw_small: DrawText,
    #[rust]
    cache: ShortCache,
}

/// The 60 s traces of every series, folded for one trace width and kept
/// until the stored data (or a frozen view) changes; frames in between only
/// slide them.
#[derive(Default)]
struct ShortCache {
    key: Option<(u64, Option<u64>, i64)>,
    series: Vec<Vec<Point>>,
    disk_max: f32,
    net_max: f32,
}

impl ShortCache {
    fn refresh(&mut self, model: &Model, end: f64, px: f64) {
        let frozen = if model.live { None } else { Some(end.max(0.0) as u64) };
        let key = (model.store.generation, frozen, px as i64);
        if self.key == Some(key) {
            return;
        }
        let span = MINUTE_MS as f64;
        let bucket = fold_bucket_ms(span, px);
        let lo = (end - span * 1.1 - bucket as f64).max(0.0) as u64;
        let hi = if model.live { model.store.newest_ms().unwrap_or(end as u64) } else { end.max(0.0) as u64 };
        self.series = SERIES.iter().map(|id| fold_by_time(&model.store.series(lo, hi, |sample| id.value(sample)), bucket)).collect();
        let pair = |a: SeriesId, b: SeriesId, series: &[Vec<Point>]| {
            let at = |id: SeriesId| SERIES.iter().position(|s| *s == id).unwrap_or(0);
            (peak(&series[at(a)]).max(peak(&series[at(b)])) * 1.15).max(1024.0)
        };
        self.disk_max = pair(SeriesId::DiskRead, SeriesId::DiskWrite, &self.series);
        self.net_max = pair(SeriesId::NetIn, SeriesId::NetOut, &self.series);
        self.key = Some(key);
    }

    fn points(&self, id: SeriesId) -> &[Point] {
        SERIES.iter().position(|s| *s == id).and_then(|at| self.series.get(at)).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

impl MetricStrip {
    fn tiles_for(width: f64, power: bool) -> Vec<Tile> {
        let mut all = vec![Tile::Cpu, Tile::Gpu, Tile::Memory, Tile::Disk, Tile::Network];
        if power {
            all.push(Tile::Power);
        }
        let fit = ((width / 160.0) as usize).clamp(1, all.len());
        // Lowest priority goes first: power, GPU, disk.
        let priority = [Tile::Cpu, Tile::Memory, Tile::Network, Tile::Disk, Tile::Gpu, Tile::Power];
        let keep: Vec<Tile> = priority.iter().copied().filter(|t| all.contains(t)).take(fit).collect();
        all.into_iter().filter(|tile| keep.contains(tile)).collect()
    }

    fn trace(&mut self, cx: &mut Cx2d, rect: Rect, window: (f64, f64), id: SeriesId, max: f32, color: Vec4f) {
        let points = self.cache.points(id).to_vec();
        draw_trace(&mut self.draw_seg, &mut self.draw_fill, cx, rect, &points, window, max, Trace { color, width: 1.5, fill: 0.22 });
    }
}

impl Widget for MetricStrip {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let Some(model) = scope.data.get::<Model>() else { return DrawStep::done() };
        let theme = model.theme;
        fill(&mut self.draw_bg, cx, rect, theme.background);
        let view = model.view_sample().cloned();
        let power = view.as_ref().is_some_and(|s| s.system.power.value().is_some());
        let tiles = Self::tiles_for(rect.size.x - 24.0, power);
        let gap = 10.0;
        let width = (rect.size.x - 24.0 - gap * (tiles.len().saturating_sub(1)) as f64) / tiles.len() as f64;
        // The last 60 s up to the moving live edge (or the scrubbed time).
        let to = model.graph_end();
        let window = (to - MINUTE_MS as f64, to);
        self.cache.refresh(model, to, (width - 2.0).max(10.0));
        for (index, tile) in tiles.iter().enumerate() {
            let tile_rect = Rect { pos: dvec2(rect.pos.x + 12.0 + index as f64 * (width + gap), rect.pos.y + 8.0), size: dvec2(width, rect.size.y - 16.0) };
            let (title, value, unavailable): (&str, String, Option<&str>) = match (tile, view.as_ref()) {
                (_, None) => ("", String::new(), Some("waiting for the first sample")),
                (Tile::Cpu, Some(s)) => ("CPU", format!("{:.0}%", s.system.cpu_total), None),
                (Tile::Gpu, Some(s)) => match s.system.gpu {
                    crate::backend::Reading::Value(value) => ("GPU", format!("{value:.0}%"), None),
                    crate::backend::Reading::Unavailable(reason) => ("GPU", String::new(), Some(reason)),
                },
                (Tile::Memory, Some(s)) => ("Memory", format_bytes(s.system.mem_used), None),
                (Tile::Disk, Some(s)) => match s.system.disk {
                    crate::backend::Reading::Value((read, write)) => ("Disk", format!("{}/s", format_bytes((read + write).max(0.0) as u64)), None),
                    crate::backend::Reading::Unavailable(reason) => ("Disk", String::new(), Some(reason)),
                },
                (Tile::Network, Some(s)) => ("Network", format!("{}/s", format_bytes((s.system.net_rx + s.system.net_tx).max(0.0) as u64)), None),
                (Tile::Power, Some(s)) => match s.system.power {
                    crate::backend::Reading::Value(value) => ("Power", format!("{value:.2} W"), None),
                    crate::backend::Reading::Unavailable(reason) => ("Power", String::new(), Some(reason)),
                },
            };
            let header_mid = tile_rect.pos.y + 10.0;
            self.draw_label.color = theme.secondary();
            text_mid(&mut self.draw_label, cx, tile_rect.pos.x, header_mid, title);
            if unavailable.is_none() {
                // The figure takes the room the title leaves: its large face
                // when it fits, the label face when not, never on the title.
                let room = tile_rect.size.x - text_width(&self.draw_label, cx, title) - 8.0;
                let right = tile_rect.pos.x + tile_rect.size.x;
                if text_width(&self.draw_value, cx, &value) <= room {
                    self.draw_value.color = theme.foreground;
                    let h = text_height(&self.draw_value, cx, &value);
                    text_right(&mut self.draw_value, cx, right, header_mid - h * 0.5, &value);
                } else {
                    self.draw_label.color = theme.foreground;
                    let w = text_width(&self.draw_label, cx, &value).min(room.max(0.0));
                    let h = text_height(&self.draw_label, cx, &value);
                    text_fit(&mut self.draw_label, cx, dvec2(right - w, header_mid - h * 0.5), room, &value);
                }
            }
            let chart = Rect { pos: tile_rect.pos + dvec2(0.0, 24.0), size: dvec2(tile_rect.size.x, tile_rect.size.y - 24.0) };
            round(&mut self.draw_round, cx, chart, theme.well(), 5.0);
            if let Some(reason) = unavailable {
                // Plain words in the chrome; the backend's reason is in the log.
                let text = if view.is_some() { "Unavailable" } else { reason };
                self.draw_small.color = theme.tertiary();
                text_mid(&mut self.draw_small, cx, chart.pos.x + 10.0, chart.pos.y + chart.size.y * 0.5, text);
                continue;
            }
            let inner = Rect { pos: chart.pos + dvec2(1.0, 4.0), size: chart.size - dvec2(2.0, 5.0) };
            cx.push_clip_rect(chart);
            match tile {
                Tile::Cpu => self.trace(cx, inner, window, SeriesId::Cpu, 100.0, SeriesId::Cpu.color(&theme)),
                Tile::Gpu => self.trace(cx, inner, window, SeriesId::Gpu, 100.0, SeriesId::Gpu.color(&theme)),
                Tile::Memory => {
                    let line = Rect { pos: inner.pos, size: dvec2(inner.size.x, inner.size.y - 7.0) };
                    self.trace(cx, line, window, SeriesId::Mem, 100.0, SeriesId::Mem.color(&theme));
                    if let Some(s) = view.as_ref() {
                        // Used and file cache as the OS reports them, shares of total.
                        let total = s.system.mem_total.max(1) as f64;
                        let bar = Rect { pos: dvec2(chart.pos.x + 6.0, chart.pos.y + chart.size.y - 7.0), size: dvec2(chart.size.x - 12.0, 3.0) };
                        let used = bar.size.x * (s.system.mem_used as f64 / total).clamp(0.0, 1.0);
                        let cache = (bar.size.x * (s.system.mem_cache as f64 / total)).clamp(0.0, bar.size.x - used);
                        round(&mut self.draw_round, cx, bar, with_alpha(theme.foreground, 0.08), 1.5);
                        round(&mut self.draw_round, cx, Rect { pos: bar.pos, size: dvec2(used, bar.size.y) }, SeriesId::Mem.color(&theme), 1.5);
                        fill(&mut self.draw_rect, cx, Rect { pos: bar.pos + dvec2(used, 0.0), size: dvec2(cache, bar.size.y) }, with_alpha(theme.cyan, 0.55));
                    }
                }
                Tile::Disk | Tile::Network => {
                    let (a, b, la, lb) = if *tile == Tile::Disk {
                        (SeriesId::DiskRead, SeriesId::DiskWrite, "R", "W")
                    } else {
                        (SeriesId::NetIn, SeriesId::NetOut, "R", "S")
                    };
                    let pa = self.cache.points(a).to_vec();
                    let pb = self.cache.points(b).to_vec();
                    let max = if *tile == Tile::Disk { self.cache.disk_max } else { self.cache.net_max };
                    let half = (inner.size.y - 2.0) * 0.5;
                    let top = Rect { pos: inner.pos + dvec2(14.0, 0.0), size: dvec2(inner.size.x - 14.0, half - 1.0) };
                    let bottom = Rect { pos: inner.pos + dvec2(14.0, half + 2.0), size: dvec2(inner.size.x - 14.0, half - 1.0) };
                    draw_trace(&mut self.draw_seg, &mut self.draw_fill, cx, top, &pa, window, max, Trace { color: a.color(&theme), width: 1.4, fill: 0.20 });
                    draw_trace(&mut self.draw_seg, &mut self.draw_fill, cx, bottom, &pb, window, max, Trace { color: b.color(&theme), width: 1.4, fill: 0.20 });
                    self.draw_small.color = theme.tertiary();
                    text_mid(&mut self.draw_small, cx, chart.pos.x + 5.0, top.pos.y + top.size.y * 0.5, la);
                    text_mid(&mut self.draw_small, cx, chart.pos.x + 5.0, bottom.pos.y + bottom.size.y * 0.5, lb);
                }
                // A measured power figure has no stored series yet; the tile
                // shows the reading and an empty well rather than a borrowed line.
                Tile::Power => {}
            }
            cx.pop_clip_rect();
        }
        DrawStep::done()
    }
}

// ---- PinnedStrip ----

/// Card widths: a running pin shows figures, a stopped one a word.
const PIN_CARD_RUNNING: f64 = 280.0;
const PIN_CARD_STOPPED: f64 = 200.0;
const PIN_CARD_GAP: f64 = 8.0;
const PIN_ARROW: f64 = 24.0;

/// One compact entry per pinned process: the name, then CPU and memory at
/// the view time with a small trace. A pinned process that is not running at
/// the view time stays listed — "Exited" after its last record, "Not
/// started" before its birth — and still selects, so its retained history
/// remains reachable. The cross unpins.
///
/// Every element of a card has its own bounded region, laid out right to
/// left: memory, the trace (dropped first when room is short), CPU, then the
/// name takes what is left and is the only thing ever truncated. When the
/// pins are wider than the strip it scrolls sideways (wheel or trackpad, or
/// the arrow buttons at its ends), so every pin stays reachable.
#[derive(Script, ScriptHook, Widget)]
pub struct PinnedStrip {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_round: DrawTaskRound,
    #[live]
    draw_seg: DrawChartSegment,
    #[live]
    draw_fill: DrawTaskFill,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_mono: DrawText,
    #[live]
    draw_icon: DrawText,
    /// (pin, visible part of the card, visible part of its close target).
    #[rust]
    cards: Vec<(ProcKey, Rect, Rect)>,
    #[rust]
    scroll_x: f64,
    /// Content wider than the viewport, and the viewport's width.
    #[rust]
    overflow: f64,
    #[rust]
    viewport_w: f64,
    #[rust]
    arrows: (Rect, Rect),
    /// Each pin's folded 60 s CPU trace, kept until the data changes.
    #[rust]
    sparks: HashMap<ProcKey, (Vec<Point>, f32)>,
    #[rust]
    sparks_key: Option<(u64, Option<u64>)>,
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.pos.x.max(b.pos.x);
    let y0 = a.pos.y.max(b.pos.y);
    let x1 = (a.pos.x + a.size.x).min(b.pos.x + b.size.x);
    let y1 = (a.pos.y + a.size.y).min(b.pos.y + b.size.y);
    Rect { pos: dvec2(x0, y0), size: dvec2((x1 - x0).max(0.0), (y1 - y0).max(0.0)) }
}

impl PinnedStrip {
    fn scroll_by(&mut self, cx: &mut Cx, delta: f64) {
        let next = (self.scroll_x + delta).clamp(0.0, self.overflow.max(0.0));
        if next != self.scroll_x {
            self.scroll_x = next;
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for PinnedStrip {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let Some(model) = scope.data.get_mut::<Model>() else { return };
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerScroll(e) => {
                let delta = if e.scroll.x.abs() > 0.0 { e.scroll.x } else { e.scroll.y };
                self.scroll_by(cx, delta);
            }
            Hit::FingerUp(e) if e.is_primary_hit() => {
                if self.overflow > 0.0 {
                    let page = (self.viewport_w * 0.8).max(PIN_CARD_STOPPED);
                    if inside(self.arrows.0, e.abs) {
                        self.scroll_by(cx, -page);
                        return;
                    }
                    if inside(self.arrows.1, e.abs) {
                        self.scroll_by(cx, page);
                        return;
                    }
                }
                let hit = self.cards.iter().find(|(_, card, _)| inside(*card, e.abs)).map(|(key, _, close)| (*key, inside(*close, e.abs)));
                if let Some((key, close)) = hit {
                    if close {
                        model.toggle_pin(key, "");
                    } else {
                        model.select(Some(key));
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let Some(model) = scope.data.get::<Model>() else { return DrawStep::done() };
        let theme = model.theme;
        fill(&mut self.draw_bg, cx, rect, theme.background);
        self.cards.clear();
        let to = model.graph_end();
        let window = (to - MINUTE_MS as f64, to);
        let sparks_key = (model.store.generation, if model.live { None } else { Some(to.max(0.0) as u64) });
        if self.sparks_key != Some(sparks_key) {
            self.sparks_key = Some(sparks_key);
            self.sparks.clear();
        }
        let mid = rect.pos.y + rect.size.y * 0.5;

        // Every card's width first, to know whether the strip must scroll.
        let states: Vec<PinState> = model.pins.iter().map(|pin| model.pin_state(pin.key)).collect();
        let widths: Vec<f64> = states.iter().map(|state| if matches!(state, PinState::Running(_)) { PIN_CARD_RUNNING } else { PIN_CARD_STOPPED }).collect();
        let content: f64 = widths.iter().sum::<f64>() + PIN_CARD_GAP * widths.len().saturating_sub(1) as f64;
        let mut view = Rect { pos: dvec2(rect.pos.x + 96.0, rect.pos.y), size: dvec2((rect.size.x - 96.0 - 12.0).max(0.0), rect.size.y) };
        let scrolls = content > view.size.x;
        if scrolls {
            // Arrow targets at both ends of the viewport.
            self.arrows = (
                Rect { pos: dvec2(view.pos.x, rect.pos.y), size: dvec2(PIN_ARROW, rect.size.y) },
                Rect { pos: dvec2(view.pos.x + view.size.x - PIN_ARROW, rect.pos.y), size: dvec2(PIN_ARROW, rect.size.y) },
            );
            view.pos.x += PIN_ARROW + 4.0;
            view.size.x = (view.size.x - 2.0 * (PIN_ARROW + 4.0)).max(0.0);
        } else {
            self.arrows = (Rect::default(), Rect::default());
        }
        self.viewport_w = view.size.x;
        self.overflow = (content - view.size.x).max(0.0);
        self.scroll_x = self.scroll_x.clamp(0.0, self.overflow);

        // The label says how many there are; the count matters when some
        // are scrolled out of view.
        self.draw_icon.color = theme.secondary();
        text_mid(&mut self.draw_icon, cx, rect.pos.x + 12.0, mid, ICON_PIN);
        self.draw_label.color = theme.secondary();
        text_mid(&mut self.draw_label, cx, rect.pos.x + 26.0, mid, &format!("Pinned {}", model.pins.len()));

        if scrolls {
            for (arrow, glyph, enabled) in [
                (self.arrows.0, ICON_CHEVRON_LEFT, self.scroll_x > 0.5),
                (self.arrows.1, ICON_CHEVRON_RIGHT, self.scroll_x < self.overflow - 0.5),
            ] {
                let button = Rect { pos: arrow.pos + dvec2(0.0, 7.0), size: dvec2(arrow.size.x, arrow.size.y - 14.0) };
                round(&mut self.draw_round, cx, button, if enabled { theme.well() } else { with_alpha(theme.well(), 0.4) }, 5.0);
                self.draw_icon.color = if enabled { theme.foreground } else { theme.tertiary() };
                let w = text_width(&self.draw_icon, cx, glyph);
                text_mid(&mut self.draw_icon, cx, button.pos.x + (button.size.x - w) * 0.5, mid, glyph);
            }
        }

        cx.push_clip_rect(view);
        let mut x = view.pos.x - self.scroll_x;
        for ((pin, state), width) in model.pins.iter().zip(states.iter()).zip(widths.iter().copied()) {
            let card = Rect { pos: dvec2(x, rect.pos.y + 5.0), size: dvec2(width, rect.size.y - 10.0) };
            x += width + PIN_CARD_GAP;
            let shown = intersect(card, view);
            if shown.size.x < 1.0 {
                continue;
            }
            if model.selected == Some(pin.key) {
                round_outline(&mut self.draw_round, cx, card, with_alpha(theme.accent, 0.14), with_alpha(theme.accent, 0.55), 6.0);
            } else {
                round(&mut self.draw_round, cx, card, theme.well(), 6.0);
            }
            let name = model.find_meta(pin.key).map(|m| m.name.clone()).unwrap_or_else(|| pin.name.clone());
            let cmid = card.pos.y + card.size.y * 0.5;
            let close = Rect { pos: dvec2(card.pos.x + width - 24.0, card.pos.y), size: dvec2(24.0, card.size.y) };
            // Regions right to left, each bounded: nothing overlaps.
            let mut right = close.pos.x - 4.0;
            match state {
                PinState::Running(record) => {
                    let cpu_text = format!("{:.1}%", record.cpu);
                    let mem_text = format_bytes(record.rss);
                    let cpu_w = text_width(&self.draw_mono, cx, &cpu_text);
                    let mem_w = text_width(&self.draw_mono, cx, &mem_text);
                    let h = text_height(&self.draw_mono, cx, &mem_text);
                    let name_min = 60.0;
                    let spark_w = 34.0;
                    let with_spark = width - 10.0 - 28.0 - mem_w - 10.0 - spark_w - 8.0 - cpu_w - 10.0 >= name_min;
                    self.draw_mono.color = theme.secondary();
                    self.draw_mono.draw_abs(cx, dvec2(right - mem_w, cmid - h * 0.5), &mem_text);
                    right -= mem_w + 10.0;
                    if with_spark {
                        let (cpu, max) = self
                            .sparks
                            .entry(pin.key)
                            .or_insert_with(|| {
                                let lo = (to - MINUTE_MS as f64 * 1.1 - 2_000.0).max(0.0) as u64;
                                let hi = if model.live { model.store.newest_ms().unwrap_or(to as u64) } else { to.max(0.0) as u64 };
                                let points = model.store.process_series(pin.key, lo, hi);
                                let cpu: Vec<Point> = points.iter().map(|p| Point { time_ms: p.time_ms, value: p.cpu, gap_before: p.gap_before }).collect();
                                let folded = fold_for(&cpu, MINUTE_MS as f64, spark_w);
                                let max = peak(&folded).max(5.0) * 1.1;
                                (folded, max)
                            })
                            .clone();
                        let spark = Rect { pos: dvec2(right - spark_w, cmid - 6.0), size: dvec2(spark_w, 12.0) };
                        draw_trace(&mut self.draw_seg, &mut self.draw_fill, cx, spark, &cpu, window, max, Trace { color: SeriesId::Cpu.color(&theme), width: 1.2, fill: 0.2 });
                        right -= spark_w + 8.0;
                    }
                    self.draw_mono.color = theme.foreground;
                    self.draw_mono.draw_abs(cx, dvec2(right - cpu_w, cmid - h * 0.5), &cpu_text);
                    right -= cpu_w + 10.0;
                    self.draw_label.color = theme.foreground;
                }
                PinState::Exited | PinState::NotStarted => {
                    let word = if matches!(state, PinState::Exited) { "Exited" } else { "Not started" };
                    let w = text_width(&self.draw_label, cx, word);
                    self.draw_label.color = theme.tertiary();
                    text_mid(&mut self.draw_label, cx, right - w, cmid, word);
                    right -= w + 10.0;
                    self.draw_label.color = theme.secondary();
                }
            }
            let h = text_height(&self.draw_label, cx, &name);
            text_fit(&mut self.draw_label, cx, dvec2(card.pos.x + 10.0, cmid - h * 0.5), right - card.pos.x - 10.0, &name);
            self.draw_icon.color = theme.tertiary();
            let w = text_width(&self.draw_icon, cx, ICON_CLOSE);
            text_mid(&mut self.draw_icon, cx, close.pos.x + (close.size.x - w) * 0.5, cmid, ICON_CLOSE);
            self.cards.push((pin.key, shown, intersect(close, view)));
        }
        cx.pop_clip_rect();
        DrawStep::done()
    }
}

// ---- ProcessTable ----

/// A drawn row: which record of the view sample, and where it sits in the tree.
#[derive(Clone, Copy, Debug)]
struct Row {
    index: usize,
    depth: usize,
    children: usize,
}

/// One graph column's traces for one row, folded into sparkline slots: a
/// single line, or a pair (in and out, read and write) on one scale.
struct Spark {
    lines: Vec<Vec<Point>>,
    max: f32,
}

/// The sparkline width the traces are folded for (they are drawn by time,
/// so a resized column only changes the fold resolution).
const SPARK_PX: f64 = 110.0;
const ROW_HEIGHT: f64 = 30.0;
/// The column chooser's menu.
fn columns_menu_owner() -> LiveId {
    live_id!(task_columns)
}

/// The process list for the view sample: sortable, searchable, list or
/// tree, pinned rows first, with the columns the user chose (a right press
/// on a heading), in the order they dragged them to, and a row order that
/// can be frozen while the numbers keep moving.
#[derive(Script, ScriptHook, Widget)]
pub struct ProcessTable {
    #[deref]
    view: View,
    #[live]
    draw_spark: DrawChartSegment,
    #[live]
    draw_fill: DrawTaskFill,
    #[live]
    draw_round: DrawTaskRound,
    #[live]
    draw_name: DrawText,
    #[live]
    draw_icon: DrawText,
    #[live]
    draw_letter: DrawText,
    #[live]
    draw_state: DrawText,
    #[rust]
    sample: Option<Arc<Sample>>,
    /// The view sample's figures, rates taken against an earlier sample.
    #[rust]
    figures: Option<Figures>,
    #[rust]
    rows: Vec<Row>,
    #[rust]
    built_version: Option<u64>,
    /// The frozen order (keys) while Freeze is on.
    #[rust]
    frozen: Vec<ProcKey>,
    #[rust]
    sparks: HashMap<(ProcKey, Graph), Spark>,
    /// The columns drawn, which are also the grid's data columns: the pin,
    /// then as many of the chosen ones as fit, in the chosen order.
    #[rust]
    fit: GridColumnFit<Column>,
    #[rust]
    theme_applied: Option<Vec4f>,
}

impl ProcessTable {
    /// Filter → sort → (freeze) → (tree + fold) → rows, all on the view sample.
    fn rebuild(&mut self, model: &Model) {
        self.sparks.clear();
        let Some(sample) = model.view_sample().cloned() else {
            self.rows.clear();
            self.sample = None;
            self.figures = None;
            return;
        };
        let figures = Figures::new(&model.store, &sample);
        let records = &sample.processes;
        let needle = model.filter.trim().to_ascii_lowercase();
        let matches = |record: &ProcRecord| {
            (!model.apps_only || record.meta.is_app)
                && (needle.is_empty()
                    || record.meta.name.to_ascii_lowercase().contains(&needle)
                    || record.meta.cmdline.to_ascii_lowercase().contains(&needle)
                    || record.meta.key.pid.to_string() == needle)
        };
        let mut kept: Vec<usize> = (0..records.len()).filter(|&i| matches(&records[i])).collect();
        if model.tree && (!needle.is_empty() || model.apps_only) {
            kept = with_ancestors(records, kept);
        }
        let sort = model.columns.sort().unwrap_or(GridSort { column: Column::Cpu, descending: true });
        let column = sort.column;
        // Read each row's figure once; a row without one sorts below every
        // row that has one.
        let values: HashMap<usize, Option<f64>> = match column {
            Column::Name | Column::User | Column::State | Column::Pin => HashMap::new(),
            _ => kept.iter().map(|&i| (i, figures.value(&sample, &records[i], column))).collect(),
        };
        kept.sort_by(|&a, &b| {
            let (ra, rb) = (&records[a], &records[b]);
            let order = match column {
                Column::Name => ra.meta.name.to_ascii_lowercase().cmp(&rb.meta.name.to_ascii_lowercase()),
                Column::User => ra.meta.user.cmp(&rb.meta.user),
                Column::State => ra.state.as_str().cmp(rb.state.as_str()),
                Column::Pin => Ordering::Equal,
                _ => {
                    let value = |i: usize| values.get(&i).copied().flatten().unwrap_or(f64::NEG_INFINITY);
                    value(a).partial_cmp(&value(b)).unwrap_or(Ordering::Equal)
                }
            };
            let order = if sort.descending { order.reverse() } else { order };
            order.then_with(|| ra.meta.key.cmp(&rb.meta.key))
        });
        if model.freeze && !self.frozen.is_empty() {
            let position: HashMap<ProcKey, usize> = self.frozen.iter().enumerate().map(|(i, k)| (*k, i)).collect();
            // Known rows keep their place; newcomers follow in sort order.
            kept.sort_by_key(|&i| position.get(&records[i].key()).copied().unwrap_or(usize::MAX));
        }
        if !model.tree && !model.pins.is_empty() {
            // Pinned rows first, in their sorted order.
            let (pinned, rest): (Vec<usize>, Vec<usize>) = kept.iter().partition(|&&i| model.is_pinned(records[i].key()));
            kept = pinned.into_iter().chain(rest).collect();
        }
        // Remember the order shown; while frozen it is the order kept.
        self.frozen = kept.iter().map(|&i| records[i].key()).collect();
        self.rows = if model.tree {
            let pairs: Vec<(u32, u32)> = kept.iter().map(|&i| (records[i].meta.key.pid, records[i].meta.ppid)).collect();
            let mut rows = Vec::with_capacity(kept.len());
            let mut fold_depth: Option<usize> = None;
            for row in tree_order(&pairs) {
                match fold_depth {
                    Some(depth) if row.depth > depth => continue,
                    _ => fold_depth = None,
                }
                let index = kept[row.index];
                if row.children > 0 && model.collapsed.contains(&records[index].key()) {
                    fold_depth = Some(row.depth);
                }
                rows.push(Row { index, depth: row.depth, children: row.children });
            }
            rows
        } else {
            kept.into_iter().map(|index| Row { index, depth: 0, children: 0 }).collect()
        };
        self.sample = Some(sample);
        self.figures = Some(figures);
    }

    fn row_of(&self, key: ProcKey) -> Option<usize> {
        let sample = self.sample.as_ref()?;
        self.rows.iter().position(|row| sample.processes[row.index].key() == key)
    }

    fn key_at(&self, row: usize) -> Option<(ProcKey, String)> {
        let sample = self.sample.as_ref()?;
        let row = self.rows.get(row)?;
        let record = &sample.processes[row.index];
        Some((record.key(), record.meta.name.clone()))
    }

    fn spark_for(&mut self, model: &Model, key: ProcKey, graph: Graph) {
        // Built once per data update (the cache is cleared on rebuild),
        // over a little more than the 60 s shown so the moving edge stays
        // covered until the next sample.
        self.sparks.entry((key, graph)).or_insert_with(|| {
            let end = model.graph_end();
            let lo = (end - MINUTE_MS as f64 * 1.1 - 2_000.0).max(0.0) as u64;
            let hi = if model.live { model.store.newest_ms().unwrap_or(end as u64) } else { end.max(0.0) as u64 };
            let pair = |a: Measure, b: Measure, floor: f32| {
                let bucket = fold_bucket_ms(MINUTE_MS as f64, SPARK_PX);
                let a = fold_for(&counter_trace(&model.store, key, a, lo, hi, bucket), MINUTE_MS as f64, SPARK_PX);
                let b = fold_for(&counter_trace(&model.store, key, b, lo, hi, bucket), MINUTE_MS as f64, SPARK_PX);
                Spark { max: (peak(&a).max(peak(&b)) * 1.25).max(floor), lines: vec![a, b] }
            };
            match graph {
                Graph::Cpu | Graph::Memory => {
                    let points = model.store.process_series(key, lo, hi);
                    let cpu = graph == Graph::Cpu;
                    let line: Vec<Point> = points.iter().map(|p| Point { time_ms: p.time_ms, value: if cpu { p.cpu } else { p.rss as f32 }, gap_before: p.gap_before }).collect();
                    let line = fold_for(&line, MINUTE_MS as f64, SPARK_PX);
                    let max = if cpu { (peak(&line) * 1.15).max(5.0) } else { (peak(&line) * 1.25).max(1.0) };
                    Spark { lines: vec![line], max }
                }
                Graph::Disk => pair(Measure::DiskRead, Measure::DiskWritten, 4096.0),
                Graph::Network => pair(Measure::NetReceived, Measure::NetSent, 1024.0),
            }
        });
    }

    pub fn apply_theme(&mut self, cx: &mut Cx, theme: Theme) {
        if self.theme_applied == Some(theme.background) {
            return;
        }
        self.theme_applied = Some(theme.background);
        let background = theme.background;
        let raised = theme.raised();
        let foreground = theme.foreground;
        let secondary = theme.secondary();
        let accent = theme.accent;
        let selection = with_alpha(theme.accent, 0.18);
        let clear = with_alpha(theme.accent, 0.0);
        let guide = with_alpha(theme.accent, 0.4);
        let line = theme.rule();
        let mut grid = self.view.data_grid(cx, ids!(process_grid));
        script_apply_eval!(cx, grid, {
            color_bg: #(background)
            color_cell: #(background)
            color_cell_alt: #(background)
            color_text: #(foreground)
            color_header: #(raised)
            color_header_active: #(raised)
            color_header_text: #(secondary)
            color_selection: #(selection)
            color_selection_border: #(clear)
            color_drag_marker: #(accent)
            color_resize_guide: #(guide)
            draw_cell +: {border_color: #(line)}
            draw_text +: {color: #(foreground)}
            draw_text_bold +: {color: #(foreground)}
            draw_text_header +: {color: #(secondary)}
        });
    }
}

/// A counter's rate over the window for a sparkline: readings about one
/// fold bucket apart rather than every sample, so a 100 ms interval costs
/// no more than a 1 s one and each rate is the exact average over its
/// bucket. A pause or restart between two readings breaks the line.
fn counter_trace(store: &Store, key: ProcKey, measure: Measure, from_ms: u64, to_ms: u64, bucket_ms: u64) -> Vec<Point> {
    let range = store.range_indices(from_ms, to_ms);
    let mut out = Vec::new();
    let mut previous: Option<(crate::metrics::Reading, u64, u64)> = None;
    let mut joined = false;
    let mut index = range.start;
    while index < range.end {
        let Some(entry) = store.entry(index) else { break };
        let sample = &entry.sample;
        let stride = (entry.stride_ms as f64).max(1.0);
        let step = ((bucket_ms as f64 / stride).floor() as usize).max(1);
        let reading = sample
            .process(key)
            .and_then(|record| crate::metrics::basic_value(sample, record, measure))
            .map(|value| crate::metrics::Reading { time_ms: sample.time_ms, value, gap_before: false });
        match (reading, previous) {
            (Some(now), Some((then, session, then_stride))) => {
                let expected = step as u64 * then_stride.max(entry.stride_ms as u64).max(100);
                let apart = now.time_ms.saturating_sub(then.time_ms) > expected * 3 / 2 + 500;
                match (!apart && session == sample.session).then(|| crate::metrics::rate_between(measure, then, now)).flatten() {
                    Some(value) => {
                        out.push(Point { time_ms: now.time_ms, value, gap_before: !joined });
                        joined = true;
                    }
                    None => joined = false,
                }
            }
            _ => joined = false,
        }
        previous = reading.map(|r| (r, sample.session, entry.stride_ms as u64));
        index += step;
    }
    out
}

/// Every index in `seed`, plus the chain of parents above each one, so a
/// filtered tree stays connected.
fn with_ancestors(records: &[ProcRecord], seed: Vec<usize>) -> Vec<usize> {
    let mut index_of_pid: HashMap<u32, usize> = HashMap::with_capacity(records.len());
    for (index, record) in records.iter().enumerate() {
        index_of_pid.entry(record.meta.key.pid).or_insert(index);
    }
    let mut wanted: HashSet<usize> = HashSet::with_capacity(seed.len() * 2);
    for index in seed {
        let mut at = index;
        let mut hops = 0;
        while wanted.insert(at) && hops < 64 {
            let parent = records[at].meta.ppid;
            if parent == 0 || parent == records[at].meta.key.pid {
                break;
            }
            let Some(&next) = index_of_pid.get(&parent) else { break };
            at = next;
            hops += 1;
        }
    }
    let mut out: Vec<usize> = wanted.into_iter().collect();
    out.sort_unstable();
    out
}

/// A process' scheduler state in words, and whether it deserves full ink.
fn state_words(state: ProcState) -> (&'static str, bool) {
    match state {
        ProcState::Running => ("running", true),
        ProcState::Sleeping => ("sleeping", false),
        ProcState::Waiting => ("disk wait", true),
        ProcState::Idle => ("idle", false),
        ProcState::Stopped => ("stopped", true),
        ProcState::Zombie => ("zombie", true),
        ProcState::Unknown => ("—", false),
    }
}

impl Widget for ProcessTable {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let width = cx.peek_walk_turtle(walk).size.x;
        let Some(model) = scope.data.get::<Model>() else {
            return self.view.draw_walk(cx, scope, walk);
        };
        let theme = model.theme;
        // Fitted again when the width or the chosen columns change, never on
        // a sample, so a column the user is resizing is left alone.
        let reconfigure = self.fit.fit(width, &model.columns);
        let rebuilt = self.built_version != Some(model.version);
        if rebuilt {
            self.built_version = Some(model.version);
            self.rebuild(model);
        }
        let selected_row = model.selected.and_then(|key| self.row_of(key));
        let sort = self.fit.sort_indicator(&model.columns);
        // `model` borrows scope; take what the cells need up front.
        let pinned: HashSet<ProcKey> = model.pins.iter().map(|pin| pin.key).collect();
        let collapsed = model.collapsed.clone();
        let tree = model.tree;
        let spark_end = model.graph_end();
        let spark_window = (spark_end - MINUTE_MS as f64, spark_end);
        let visible_keys: Vec<ProcKey>;
        {
            let grid_ref = self.view.data_grid(cx, ids!(process_grid));
            let (first, count) = grid_ref
                .borrow()
                .map(|g| {
                    let (rows, _) = g.visible_counts();
                    ((g.scroll_pos().y / ROW_HEIGHT) as usize, rows + 2)
                })
                .unwrap_or((0, 40));
            let sample = self.sample.clone();
            visible_keys = self.rows.iter().skip(first).take(count).filter_map(|row| sample.as_ref().map(|s| s.processes[row.index].key())).collect();
        }
        let graphs: Vec<Graph> = self.fit.columns().iter().filter_map(|c| c.graph()).collect();
        for key in visible_keys {
            for graph in &graphs {
                self.spark_for(model, key, *graph);
            }
        }
        let columns = self.fit.columns().to_vec();
        while let Some(step) = self.view.draw_walk(cx, &mut Scope::empty(), walk).step() {
            if let Some(mut grid) = step.as_data_grid().borrow_mut() {
                // Widths only when the table's width or column set changes, so
                // a column the user resized keeps its width across samples.
                if reconfigure {
                    self.fit.configure(cx, &mut grid, self.rows.len(), width, &model.columns);
                }
                grid.set_sort_indicator(sort);
                grid.set_grid_size(self.rows.len(), columns.len());
                if rebuilt || reconfigure {
                    grid.set_selection(cx, selected_row.map(row_selection));
                }
                let (Some(sample), Some(figures)) = (self.sample.clone(), self.figures.as_ref()) else { continue };
                // Figures in the monospace slot, padded to a fixed width.
                let figure = CellStyle { bold: true, ..CellStyle::default() };
                let quiet = CellStyle { bold: true, color: Some(theme.secondary()), ..CellStyle::default() };
                let absent = CellStyle { bold: true, color: Some(with_alpha(theme.secondary(), 0.45)), ..CellStyle::default() };
                while let Some(cell) = grid.next_cell(cx) {
                    let Some(row) = self.rows.get(cell.row).copied() else { continue };
                    let Some(column) = columns.get(cell.col).copied() else { continue };
                    let record = &sample.processes[row.index];
                    let mid = cell.rect.pos.y + cell.rect.size.y * 0.5;
                    match column {
                        Column::Pin => {
                            grid.cell_bg(cx, &cell, theme.background);
                            // The selected row carries an accent edge.
                            if Some(cell.row) == selected_row {
                                round(&mut self.draw_round, cx, Rect { pos: dvec2(cell.rect.pos.x + 1.0, cell.rect.pos.y + 5.0), size: dvec2(3.0, cell.rect.size.y - 10.0) }, theme.accent, 1.5);
                            }
                            let on = pinned.contains(&record.key());
                            self.draw_icon.color = if on { theme.accent } else { with_alpha(theme.secondary(), 0.35) };
                            let w = text_width(&self.draw_icon, cx, ICON_PIN);
                            text_mid(&mut self.draw_icon, cx, cell.rect.pos.x + (cell.rect.size.x - w) * 0.5 + 2.0, mid, ICON_PIN);
                        }
                        Column::Name => {
                            grid.cell_bg(cx, &cell, theme.background);
                            cx.push_clip_rect(cell.rect);
                            let mut x = cell.rect.pos.x + 8.0;
                            if tree {
                                x += row.depth.min(24) as f64 * 14.0;
                                if row.children > 0 {
                                    let chevron = if collapsed.contains(&record.key()) { ICON_CHEVRON_RIGHT } else { ICON_CHEVRON_DOWN };
                                    self.draw_icon.color = theme.secondary();
                                    let w = text_width(&self.draw_icon, cx, chevron);
                                    text_mid(&mut self.draw_icon, cx, x + (10.0 - w) * 0.5, mid, chevron);
                                }
                                x += 14.0;
                            }
                            kind_glyph(&mut self.draw_round, &mut self.draw_letter, &mut self.draw_icon, cx, &theme, &record.meta, dvec2(x + 9.0, mid), 18.0);
                            x += 26.0;
                            self.draw_name.color = theme.foreground;
                            let h = text_height(&self.draw_name, cx, &record.meta.name);
                            text_fit(&mut self.draw_name, cx, dvec2(x, mid - h * 0.5), cell.rect.pos.x + cell.rect.size.x - x - 6.0, &record.meta.name);
                            cx.pop_clip_rect();
                        }
                        Column::User => grid.cell_text_styled(cx, &cell, &record.meta.user, CellStyle { color: Some(theme.secondary()), ..CellStyle::default() }),
                        Column::State => {
                            grid.cell_bg(cx, &cell, theme.background);
                            let (words, notable) = state_words(record.state);
                            let dot = match record.state {
                                ProcState::Running => theme.green,
                                ProcState::Zombie => theme.red,
                                ProcState::Stopped | ProcState::Waiting => theme.yellow,
                                _ => with_alpha(theme.secondary(), 0.4),
                            };
                            let x = cell.rect.pos.x + 8.0;
                            round(&mut self.draw_round, cx, Rect { pos: dvec2(x, mid - 3.0), size: dvec2(6.0, 6.0) }, dot, 3.0);
                            self.draw_state.color = if notable { theme.foreground } else { theme.secondary() };
                            text_mid(&mut self.draw_state, cx, x + 12.0, mid, words);
                        }
                        _ if column.graph().is_some() => {
                            grid.cell_bg(cx, &cell, theme.background);
                            let Some(graph) = column.graph() else { continue };
                            if let Some(spark) = self.sparks.get(&(record.key(), graph)) {
                                let inset = Rect { pos: cell.rect.pos + dvec2(4.0, 7.0), size: cell.rect.size - dvec2(16.0, 14.0) };
                                let colors: &[Vec4f] = match graph {
                                    Graph::Cpu => &[SeriesId::Cpu.color(&theme)],
                                    Graph::Memory => &[SeriesId::Mem.color(&theme)],
                                    Graph::Disk => &[SeriesId::DiskRead.color(&theme), SeriesId::DiskWrite.color(&theme)],
                                    Graph::Network => &[SeriesId::NetIn.color(&theme), SeriesId::NetOut.color(&theme)],
                                };
                                // A pair shares its fill thinner, so the second
                                // line stays readable over the first.
                                let fill = if spark.lines.len() > 1 { 0.14 } else { 0.22 };
                                cx.push_clip_rect(inset);
                                for (line, color) in spark.lines.iter().zip(colors) {
                                    draw_trace(&mut self.draw_spark, &mut self.draw_fill, cx, inset, line, spark_window, spark.max, Trace { color: *color, width: 1.2, fill });
                                }
                                cx.pop_clip_rect();
                            }
                        }
                        _ => {
                            let w = column.chars();
                            match figures.text(&sample, record, column) {
                                Some(text) => {
                                    let style = match column {
                                        Column::Cpu if record.cpu >= 80.0 => CellStyle { color: Some(theme.red), ..figure },
                                        Column::Threads | Column::Pid | Column::Ppid | Column::Priority | Column::Nice => quiet,
                                        _ => figure,
                                    };
                                    grid.cell_text_styled(cx, &cell, &format!("{text:>w$}"), style);
                                }
                                None => grid.cell_text_styled(cx, &cell, &format!("{:>w$}", "—"), absent),
                            }
                        }
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, &mut Scope::empty());
        let Event::Actions(actions) = event else { return };
        let Some(model) = scope.data.get_mut::<Model>() else { return };
        if let Some(id) = menu_picked(actions, columns_menu_owner()) {
            if model.columns.apply_pick(id) {
                model.requests.columns = true;
                model.changed();
            }
            cx.action(MenuAction::Update { owner: columns_menu_owner(), rows: model.columns.menu_rows() });
        }
        let grid = self.view.data_grid(cx, ids!(process_grid));
        for action in grid.actions(actions) {
            match action {
                DataGridAction::HeaderClicked { col, .. } => {
                    let Some(column) = self.fit.column(col) else { continue };
                    if model.columns.press_heading(column) {
                        self.frozen.clear();
                        model.requests.columns = true;
                        model.changed();
                    }
                }
                DataGridAction::HeaderContextMenu { abs, .. } => {
                    let at = Rect { pos: abs, size: dvec2(0.0, 0.0) };
                    cx.action(MenuAction::OpenSet { owner: columns_menu_owner(), rows: model.columns.menu_rows(), anchor: at, place: MenuPlace::At });
                }
                DataGridAction::ColumnOrderChanged { order } => {
                    model.columns.fold_order(&self.fit, &order);
                    model.requests.columns = true;
                    model.changed();
                }
                DataGridAction::ColumnResized { col, width, .. } => {
                    if model.columns.set_width(&self.fit, col, width) {
                        model.requests.columns = true;
                    }
                }
                DataGridAction::SelectionChanged { selection } => {
                    // Rows mode selects whole rows itself; only a row pick
                    // names a process. Anything else (a cleared or column
                    // selection) leaves the chosen process alone.
                    let row = selection.filter(|selection| selection.kind == GridSelectKind::Rows).map(|selection| selection.head.0);
                    if let Some((key, _)) = row.and_then(|row| self.key_at(row)) {
                        model.select(Some(key));
                    }
                }
                DataGridAction::CellClicked { row, col, .. } => {
                    if self.fit.column(col) == Some(Column::Pin) {
                        if let Some((key, name)) = self.key_at(row) {
                            model.toggle_pin(key, &name);
                        }
                    }
                }
                DataGridAction::CellDoubleClicked { row, .. } => {
                    if model.tree {
                        if let Some(entry) = self.rows.get(row).copied() {
                            if entry.children > 0 {
                                if let Some((key, _)) = self.key_at(row) {
                                    if !model.collapsed.remove(&key) {
                                        model.collapsed.insert(key);
                                    }
                                    model.changed();
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// One whole process line, highlighted edge to edge.
fn row_selection(row: usize) -> GridSelection {
    GridSelection { kind: GridSelectKind::Rows, anchor: (row, 0), head: (row, 0) }
}

// ---- InspectorBody ----

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InspectorTab {
    #[default]
    Info,
    Activity,
    History,
    Threads,
    Memory,
    Files,
    Ports,
    Libraries,
}

pub const INSPECTOR_TABS: [InspectorTab; 8] = [
    InspectorTab::Info,
    InspectorTab::Activity,
    InspectorTab::History,
    InspectorTab::Threads,
    InspectorTab::Memory,
    InspectorTab::Files,
    InspectorTab::Ports,
    InspectorTab::Libraries,
];

impl InspectorTab {
    fn index(self) -> usize {
        INSPECTOR_TABS.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// The selected process at the view time: an identity header with evenly
/// spaced figures, then the active tab. Everything shown for a past time is
/// what was recorded then — the sample's own figures, or detail collected at
/// or before the cursor — and anything not recorded says so instead of
/// reading the live process.
#[derive(Script, ScriptHook, Widget)]
pub struct InspectorBody {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_rect: DrawColor,
    #[live]
    draw_round: DrawTaskRound,
    #[live]
    draw_seg: DrawChartSegment,
    #[live]
    draw_fill: DrawTaskFill,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_mono: DrawText,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_value: DrawText,
    #[live]
    draw_small: DrawText,
    #[live]
    draw_heading: DrawText,
    #[live]
    draw_icon: DrawText,
    #[live]
    draw_letter: DrawText,
    #[rust]
    pub tab: InspectorTab,
    #[rust]
    scroll: [f64; 8],
    #[rust]
    content_height: f64,
    /// Horizontal scroll per tab, and the widest content drawn: a table
    /// wider than a narrow inspector scrolls sideways instead of losing
    /// columns.
    #[rust]
    hscroll: [f64; 8],
    #[rust]
    content_width: f64,
    #[rust]
    body: Rect,
    /// The Info tab's "Sampling details" disclosure.
    #[rust]
    details_open: bool,
    #[rust]
    details_toggle: Rect,
    /// The Threads table's sort: (column, descending). Kept across samples
    /// and processes; % CPU descending until a heading is clicked.
    #[rust(ThreadSort::default())]
    thread_sort: ThreadSort,
    /// The sort to draw with the next `table` call, and the headings it drew.
    #[rust]
    table_sort: Option<(usize, bool)>,
    #[rust]
    header_hits: Vec<(usize, Rect)>,
    /// What the History tab graphs, in order.
    #[rust]
    history_measures: Vec<Measure>,
    /// Hit areas drawn this frame: History chips, rows that open a figure
    /// in History, thread rows, timeline rows (time to scrub to).
    #[rust]
    chip_hits: Vec<(Measure, Rect)>,
    #[rust]
    row_links: Vec<(Measure, Rect)>,
    #[rust]
    thread_hits: Vec<(ThreadKey, Rect)>,
    #[rust]
    time_hits: Vec<(u64, Rect)>,
    /// The large graphs drawn this frame (clipped to what is visible), the
    /// window they were drawn with, and a drag in progress on one.
    #[rust]
    graph_hits: Vec<Rect>,
    #[rust]
    drawn_window: (f64, f64),
    #[rust]
    gesture: Option<Gesture>,
    #[rust]
    cursor: CursorDrag,
    /// A primary press in History or Threads whose intent is not yet known:
    /// a mostly vertical drag scrolls the tab, a horizontal one on a graph
    /// moves the cursor, a release without moving is a click.
    #[rust]
    press: Option<Press>,
    /// The per-thread CPU graph shared by History and Threads, its legend's
    /// entries and "Show all" drawn this frame.
    #[rust]
    thread_graph: ThreadGraph,
    #[rust]
    legend_hits: Vec<(ThreadKey, Rect)>,
    #[rust]
    legend_more: Rect,
    #[rust]
    legend_all: bool,
    #[rust]
    available_cache: Option<((u64, u64, ProcKey), Vec<Measure>)>,
    #[rust]
    selected_thread: Option<(ProcKey, ThreadKey)>,
    /// The Memory tab's "Address space walk" disclosure.
    #[rust]
    regions_open: bool,
    #[rust]
    regions_toggle: Rect,
    #[rust]
    history_cache: GraphCache,
    #[rust]
    spark_cache: GraphCache,
    #[rust]
    memory_cache: GraphCache,
    #[rust]
    thread_traces: ThreadTraceCache,
    #[rust]
    activity_rows: FigureCache,
    #[rust]
    memory_rows: FigureCache,
    #[rust]
    counter_rows: FigureCache,
    #[rust]
    thread_rows: ThreadRowCache,
    #[rust]
    file_rows: FileCache,
}

/// How far a press must move before its intent counts, px.
const INTENT_PX: f64 = 6.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PressMode {
    Undecided,
    Scroll,
    Scrub,
}

/// A primary press in a scrollable graph tab (History, Threads).
#[derive(Clone, Copy)]
pub struct Press {
    start: Vec2d,
    /// The graph under the press, if any.
    graph: Option<Rect>,
    /// The tab's scroll when the press began.
    scroll: f64,
    mode: PressMode,
    key: ProcKey,
    tab: InspectorTab,
}

/// Which Threads column orders the rows, and which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadSort {
    column: usize,
    descending: bool,
}

impl Default for ThreadSort {
    fn default() -> Self {
        Self { column: THREAD_COL_CPU, descending: true }
    }
}

const THREAD_COL_NAME: usize = 0;
const THREAD_COL_STATE: usize = 1;
const THREAD_COL_CPU: usize = 2;
const THREAD_COL_TIME: usize = 3;
const THREAD_COL_ID: usize = 4;
const ICON_SORT_DOWN: &str = "\u{f0d7}";
const ICON_SORT_UP: &str = "\u{f0d8}";

/// Order threads by `sort`. Missing values sort last whichever way; the
/// thread id breaks ties so the order is stable from sample to sample. With
/// no per-thread CPU figure on this OS, % CPU orders by CPU time instead.
fn sort_threads(threads: &mut [&crate::backend::ThreadInfo], sort: ThreadSort) {
    let no_cpu = threads.iter().all(|t| t.cpu_pct.is_none());
    let column = if sort.column == THREAD_COL_CPU && no_cpu { THREAD_COL_TIME } else { sort.column };
    let flip = |order: Ordering| if sort.descending { order.reverse() } else { order };
    threads.sort_by(|a, b| {
        let order = match column {
            THREAD_COL_NAME => match (a.name.is_empty(), b.name.is_empty()) {
                (false, false) => flip(a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase())),
                (true, true) => Ordering::Equal,
                (empty_a, _) => if empty_a { Ordering::Greater } else { Ordering::Less },
            },
            THREAD_COL_STATE => flip(a.state.as_str().cmp(b.state.as_str())),
            THREAD_COL_CPU => match (a.cpu_pct, b.cpu_pct) {
                (Some(x), Some(y)) => flip(x.partial_cmp(&y).unwrap_or(Ordering::Equal)),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            THREAD_COL_TIME => match (a.cpu_time_ns, b.cpu_time_ns) {
                (Some(x), Some(y)) => flip(x.cmp(&y)),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            THREAD_COL_ID => flip(a.id.cmp(&b.id)),
            _ => Ordering::Equal,
        };
        order.then_with(|| a.id.cmp(&b.id))
    });
}

/// A layout cursor for the inspector's rows.
struct Pen {
    x: f64,
    y: f64,
    width: f64,
    line: f64,
}

/// Label rail width in key/value sections.
const RAIL: f64 = 118.0;

impl InspectorBody {
    pub fn set_tab(&mut self, cx: &mut Cx, tab: InspectorTab) {
        self.tab = tab;
        self.draw_bg.redraw(cx);
    }

    fn visible(&self, y: f64) -> bool {
        y + 24.0 >= self.body.pos.y && y <= self.body.pos.y + self.body.size.y
    }

    fn text(&mut self, cx: &mut Cx2d, x: f64, y: f64, max: f64, color: Vec4f, text: &str) {
        if self.visible(y) {
            self.draw_label.color = color;
            text_fit(&mut self.draw_label, cx, dvec2(x, y), max, text);
        }
    }

    fn mono(&mut self, cx: &mut Cx2d, x: f64, y: f64, max: f64, color: Vec4f, text: &str) {
        if self.visible(y) {
            self.draw_mono.color = color;
            text_fit(&mut self.draw_mono, cx, dvec2(x, y + 1.0), max, text);
        }
    }

    fn note(&mut self, cx: &mut Cx2d, theme: &Theme, pen: &mut Pen, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.visible(pen.y) {
            self.draw_small.color = theme.tertiary();
            text_fit(&mut self.draw_small, cx, dvec2(pen.x, pen.y), pen.width, text);
        }
        pen.y += 20.0;
    }

    /// `label  value` on the section rail; values in the figure face.
    fn kv(&mut self, cx: &mut Cx2d, theme: &Theme, pen: &mut Pen, label: &str, value: &str) {
        self.text(cx, pen.x, pen.y, RAIL - 8.0, theme.secondary(), label);
        self.mono(cx, pen.x + RAIL, pen.y, pen.width - RAIL - 4.0, theme.foreground, value);
        pen.y += pen.line;
    }

    /// A titled inset section of key/value lines, two columns when there is
    /// room, stacked otherwise.
    fn section(&mut self, cx: &mut Cx2d, theme: &Theme, pen: &mut Pen, title: &str, left: &[(&str, String)], right: &[(&str, String)]) {
        let two = pen.width >= 620.0 && !right.is_empty();
        let rows = if two { left.len().max(right.len()) } else { left.len() + right.len() };
        let height = 34.0 + rows as f64 * pen.line + 8.0;
        let panel = Rect { pos: dvec2(pen.x, pen.y), size: dvec2(pen.width, height) };
        if self.visible(panel.pos.y) || self.visible(panel.pos.y + height) {
            round(&mut self.draw_round, cx, panel, theme.well(), 7.0);
        }
        if self.visible(pen.y + 10.0) {
            self.draw_heading.color = theme.secondary();
            self.draw_heading.draw_abs(cx, dvec2(pen.x + 14.0, pen.y + 10.0), title);
        }
        let mut column = Pen { x: pen.x + 14.0, y: pen.y + 34.0, width: if two { pen.width * 0.5 - 20.0 } else { pen.width - 28.0 }, line: pen.line };
        for (label, value) in left {
            self.kv(cx, theme, &mut column, label, value);
        }
        if two {
            column = Pen { x: pen.x + pen.width * 0.5 + 6.0, y: pen.y + 34.0, width: pen.width * 0.5 - 20.0, line: pen.line };
        }
        for (label, value) in right {
            self.kv(cx, theme, &mut column, label, value);
        }
        pen.y += height + 10.0;
    }

    /// A table: a quiet header rule, then rows. `columns` are (title, width,
    /// right-aligned, figure face); a width of 0 is the flexible column,
    /// capped so the figures stay near it, with any spare room at the end.
    /// Returns the column widths and the y of the first row, for callers
    /// that draw into a cell or hit-test rows.
    fn table(&mut self, cx: &mut Cx2d, theme: &Theme, pen: &mut Pen, columns: &[(&str, f64, bool, bool)], rows: &[Vec<(String, Vec4f)>]) -> (Vec<f64>, f64) {
        let fixed: f64 = columns.iter().map(|c| c.1).sum();
        // Too narrow for every column: the table keeps its widths (paths
        // keep 200 px) and the tab scrolls sideways to reach them.
        let flexible = if pen.width - fixed >= 200.0 { (pen.width - fixed).clamp(120.0, (pen.width * 0.45).max(260.0)) } else { 200.0 };
        let widths: Vec<f64> = columns.iter().map(|c| if c.1 == 0.0 { flexible } else { c.1 }).collect();
        let body_left = self.body.pos.x + 14.0;
        self.content_width = self.content_width.max(pen.x + widths.iter().sum::<f64>() - body_left + self.hscroll[self.tab.index()]);
        let sort = self.table_sort.take();
        let mut x = pen.x;
        for (index, (title, _, right, _)) in columns.iter().enumerate() {
            if self.visible(pen.y) {
                let active = sort.filter(|(column, _)| *column == index);
                self.draw_small.color = if active.is_some() { theme.foreground } else { theme.secondary() };
                let title_w = text_width(&self.draw_small, cx, title);
                let title_x = if *right { x + widths[index] - 10.0 - title_w } else { x };
                self.draw_small.draw_abs(cx, dvec2(title_x, pen.y), title);
                if let Some((_, descending)) = active {
                    // The mark sits beside the title, on the side away from
                    // the figures, with room to spare.
                    let glyph = if descending { ICON_SORT_DOWN } else { ICON_SORT_UP };
                    let gw = text_width(&self.draw_icon, cx, glyph);
                    let gx = if *right { title_x - gw - 6.0 } else { title_x + title_w + 6.0 };
                    self.draw_icon.color = theme.accent;
                    text_mid(&mut self.draw_icon, cx, gx, pen.y + 7.0, glyph);
                }
                if sort.is_some() {
                    self.header_hits.push((index, Rect { pos: dvec2(x, pen.y - 4.0), size: dvec2(widths[index], 22.0) }));
                }
            }
            x += widths[index];
        }
        pen.y += 20.0;
        if self.visible(pen.y) {
            hline(&mut self.draw_rect, cx, pen.x, pen.y - 4.0, pen.width, theme.rule());
        }
        let first = pen.y;
        for row in rows {
            if self.visible(pen.y) {
                let mut x = pen.x;
                for (index, (text, color)) in row.iter().enumerate() {
                    let Some(&(_, _, right, figure)) = columns.get(index) else { break };
                    let draw = if figure { &mut self.draw_mono } else { &mut self.draw_label };
                    draw.color = *color;
                    let y = if figure { pen.y + 1.0 } else { pen.y };
                    if right {
                        text_right(draw, cx, x + widths[index] - 10.0, y, text);
                    } else {
                        text_fit(draw, cx, dvec2(x, y), widths[index] - 12.0, text);
                    }
                    x += widths[index];
                }
            }
            pen.y += pen.line;
        }
        (widths, first)
    }

    /// Which recorded detail to show, and a note saying when it was taken —
    /// empty while live and fresh, since that is the ordinary case.
    fn detail_for<'a>(&self, model: &'a Model, key: ProcKey, libraries: bool) -> (Option<&'a Arc<ProcDetail>>, String) {
        let view = model.view_ms();
        let found = if libraries {
            model.details.libraries_at_or_before(key, view, (15 * SECOND_MS).max(model.interval_ms * 3))
        } else if model.live {
            model.details.latest(key).filter(|d| view.saturating_sub(d.time_ms) <= model.detail_tolerance_ms().max(5 * SECOND_MS))
        } else {
            model.details.at_or_before(key, view, model.detail_tolerance_ms())
        };
        let note = match found {
            Some(detail) if libraries && model.live => format!("Read {}", local_hms(detail.time_ms)),
            Some(_) if model.live => String::new(),
            Some(detail) => format!("Recorded {} · {:.1} s before the cursor", local_hms(detail.time_ms), view.saturating_sub(detail.time_ms) as f64 / 1000.0),
            None if model.live => "Collecting…".to_string(),
            None => format!("Not recorded at {}: detail is kept for about 15 minutes, and only while a process is inspected.", local_hms(view)),
        };
        (found, note)
    }
}

fn detail_block<T>(detail: Option<&Arc<ProcDetail>>, pick: impl Fn(&ProcDetail) -> &Detail<T>) -> Result<&T, String> {
    match detail.map(|d| pick(d)) {
        Some(Detail::Ready(value)) => Ok(value),
        Some(Detail::Unavailable(reason)) => Err(reason.clone()),
        None => Err(String::new()),
    }
}

impl InspectorBody {
    /// A click (a press released without moving) in History or Threads:
    /// the History chips, the thread legend (pick a thread's line, show all
    /// entries), the Threads headings (sort) and rows (pick).
    fn click(&mut self, cx: &mut Cx, abs: Vec2d, scope: &mut Scope) {
        let key = scope.data.get::<Model>().and_then(|model| model.selected);
        let pick = |this: &mut Self, thread: ThreadKey| {
            if let Some(key) = key {
                this.selected_thread = if this.selected_thread == Some((key, thread)) { None } else { Some((key, thread)) };
            }
        };
        if inside(self.legend_more, abs) {
            self.legend_all = !self.legend_all;
        } else if let Some((thread, _)) = self.legend_hits.iter().find(|(_, rect)| inside(*rect, abs)).copied() {
            pick(self, thread);
        } else if self.tab == InspectorTab::History {
            if let Some((measure, _)) = self.chip_hits.iter().find(|(_, rect)| inside(*rect, abs)).copied() {
                match self.history_measures.iter().position(|m| *m == measure) {
                    Some(at) => {
                        self.history_measures.remove(at);
                    }
                    None => self.history_measures.push(measure),
                }
            }
        } else if let Some((column, _)) = self.header_hits.iter().find(|(_, rect)| inside(*rect, abs)).copied() {
            self.thread_sort = if self.thread_sort.column == column {
                ThreadSort { column, descending: !self.thread_sort.descending }
            } else {
                // Figures biggest first; words and ids A→Z / low first.
                ThreadSort { column, descending: matches!(column, THREAD_COL_CPU | THREAD_COL_TIME) }
            };
        } else if let Some((thread, _)) = self.thread_hits.iter().find(|(_, rect)| inside(*rect, abs)).copied() {
            pick(self, thread);
        }
        self.draw_bg.redraw(cx);
    }
}

impl Widget for InspectorBody {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Some(model) = scope.data.get_mut::<Model>() {
            if self.cursor.frame(cx, event, model) {
                self.draw_bg.redraw(cx);
            }
        }
        match event.hits(cx, self.draw_bg.area()) {
            // History and Threads: a press waits for intent. Moving 6 px
            // mostly vertically scrolls the tab by the pointer's travel (no
            // cursor move, no edge following); mostly horizontally on a
            // graph drags the cursor from where the press began; released
            // without moving, it is a click (on a graph: the cursor there).
            Hit::FingerDown(e) if e.is_primary_hit() && matches!(self.tab, InspectorTab::History | InspectorTab::Threads) && inside(self.body, e.abs) => {
                let Some(key) = scope.data.get::<Model>().and_then(|model| model.selected) else { return };
                let graph = self.graph_hits.iter().copied().find(|rect| inside(*rect, e.abs));
                self.gesture = None;
                self.press = Some(Press { start: e.abs, graph, scroll: self.scroll[self.tab.index()], mode: PressMode::Undecided, key, tab: self.tab });
            }
            Hit::FingerMove(e) if self.press.is_some() => {
                let Some(mut press) = self.press else { return };
                let Some(model) = scope.data.get_mut::<Model>() else { return };
                let travel = e.abs - press.start;
                if press.mode == PressMode::Undecided && travel.x.abs().max(travel.y.abs()) >= INTENT_PX {
                    press.mode = match press.graph {
                        Some(rect) if travel.x.abs() >= travel.y.abs() => {
                            self.cursor.begin(model, rect, press.start.x);
                            PressMode::Scrub
                        }
                        _ => PressMode::Scroll,
                    };
                }
                match press.mode {
                    PressMode::Scroll => {
                        let index = press.tab.index();
                        let max = (self.content_height - self.body.size.y + 16.0).max(0.0);
                        let next = (press.scroll - travel.y).clamp(0.0, max);
                        if next != self.scroll[index] {
                            self.scroll[index] = next;
                            self.draw_bg.redraw(cx);
                        }
                    }
                    PressMode::Scrub => {
                        self.cursor.moved(cx, model, e.abs.x);
                        self.draw_bg.redraw(cx);
                    }
                    PressMode::Undecided => {}
                }
                self.press = Some(press);
            }
            Hit::FingerUp(e) if self.press.is_some() => {
                let Some(press) = self.press.take() else { return };
                match (press.mode, press.graph) {
                    (PressMode::Undecided, Some(rect)) => {
                        if let Some(model) = scope.data.get_mut::<Model>() {
                            self.cursor.begin(model, rect, press.start.x);
                        }
                        self.cursor.end();
                    }
                    (PressMode::Undecided, None) => self.click(cx, e.abs, scope),
                    // Nothing moves on release.
                    _ => self.cursor.end(),
                }
                self.draw_bg.redraw(cx);
            }
            // On a large graph: left drag moves the shared cursor, right drag
            // pans the shared window, the wheel zooms it. Elsewhere the wheel
            // scrolls the tab.
            Hit::FingerDown(e) if self.graph_hits.iter().any(|rect| inside(*rect, e.abs)) => {
                let rect = self.graph_hits.iter().copied().find(|rect| inside(*rect, e.abs)).unwrap_or_default();
                let Some(model) = scope.data.get_mut::<Model>() else { return };
                let Some(key) = model.selected else { return };
                let window = self.drawn_window;
                let pan = e.device.mouse_button().is_some_and(|button| button.is_secondary());
                if pan {
                    model.freeze_view();
                } else if e.is_primary_hit() {
                    // The cursor drag, shared with the history band (edge
                    // following, going live at the present).
                    self.cursor.begin(model, rect, e.abs.x);
                } else {
                    return;
                }
                self.gesture = Some(Gesture { pan, rect, window, start_x: e.abs.x, key, tab: self.tab });
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(e) if self.gesture.is_some() || self.cursor.claims() => {
                let Some(model) = scope.data.get_mut::<Model>() else { return };
                match self.gesture {
                    Some(gesture) if gesture.pan => {
                        let shift = (e.abs.x - gesture.start_x) / gesture.rect.size.x.max(1.0) * (gesture.window.1 - gesture.window.0);
                        model.pan_to(gesture.window.1 - shift);
                    }
                    _ => self.cursor.moved(cx, model, e.abs.x),
                }
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(_) if self.gesture.is_some() || self.cursor.claims() => {
                // Nothing moves on release, and the release is not a click.
                self.gesture = None;
                self.cursor.end();
            }
            Hit::FingerScroll(e) if self.gesture.is_none() && !self.cursor.claims() && self.graph_hits.iter().any(|rect| inside(*rect, e.abs)) => {
                let factor = wheel_zoom_factor(e.scroll.y);
                let rect = self.graph_hits.iter().copied().find(|rect| inside(*rect, e.abs)).unwrap_or_default();
                if factor != 1.0 {
                    if let Some(model) = scope.data.get_mut::<Model>() {
                        let window = self.drawn_window;
                        model.zoom(inspect::time_at_f(rect, window, e.abs.x), factor, window);
                        self.draw_bg.redraw(cx);
                    }
                }
            }
            Hit::FingerScroll(e) if e.scroll.x.abs() > e.scroll.y.abs() => {
                let index = self.tab.index();
                let max = (self.content_width - (self.body.size.x - 28.0)).max(0.0);
                let next = (self.hscroll[index] + e.scroll.x).clamp(0.0, max);
                if next != self.hscroll[index] {
                    self.hscroll[index] = next;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerScroll(e) => {
                let index = self.tab.index();
                let max = (self.content_height - self.body.size.y + 16.0).max(0.0);
                let next = (self.scroll[index] + e.scroll.y).clamp(0.0, max);
                if next != self.scroll[index] {
                    self.scroll[index] = next;
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerUp(e) if e.is_primary_hit() && self.tab == InspectorTab::Info && inside(self.details_toggle, e.abs) => {
                self.details_open = !self.details_open;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(e) if e.is_primary_hit() && self.tab == InspectorTab::Memory && inside(self.regions_toggle, e.abs) => {
                self.regions_open = !self.regions_open;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(e) if e.is_primary_hit() && matches!(self.tab, InspectorTab::Activity | InspectorTab::Memory) => {
                // A figure opens its graph in History, first in the list.
                if let Some((measure, _)) = self.row_links.iter().find(|(_, rect)| inside(*rect, e.abs)).copied() {
                    self.history_measures.retain(|m| *m != measure);
                    self.history_measures.insert(0, measure);
                    self.tab = InspectorTab::History;
                    self.scroll[InspectorTab::History.index()] = 0.0;
                    if let Some(model) = scope.data.get_mut::<Model>() {
                        // App restyles the tab strip on a redraw request.
                        model.requests.redraw = true;
                    }
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerUp(e) if e.is_primary_hit() && self.tab == InspectorTab::Files => {
                if let Some((time, _)) = self.time_hits.iter().find(|(_, rect)| inside(*rect, e.abs)).copied() {
                    if let Some(model) = scope.data.get_mut::<Model>() {
                        model.scrub_to(time);
                        model.requests.redraw = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let Some(model) = scope.data.get::<Model>() else { return DrawStep::done() };
        let theme = model.theme;
        fill(&mut self.draw_bg, cx, rect, theme.background);
        self.details_toggle = Rect::default();
        self.regions_toggle = Rect::default();
        self.header_hits.clear();
        self.chip_hits.clear();
        self.row_links.clear();
        self.thread_hits.clear();
        self.time_hits.clear();
        self.graph_hits.clear();
        self.legend_hits.clear();
        self.legend_more = Rect::default();
        if self.press.is_some_and(|p| Some(p.key) != model.selected || p.tab != self.tab) {
            self.press = None;
            self.cursor.end();
        }
        self.drawn_window = model.window_f();
        if self.gesture.is_some_and(|g| Some(g.key) != model.selected || g.tab != self.tab) {
            self.gesture = None;
            self.cursor.end();
        }
        let pad = 14.0;
        let Some(key) = model.selected else {
            self.draw_label.color = theme.secondary();
            text_mid(&mut self.draw_label, cx, rect.pos.x + pad, rect.pos.y + 24.0, "Select a process to inspect it.");
            return DrawStep::done();
        };
        let view = model.view_sample().cloned();
        let record = view.as_ref().and_then(|sample| sample.process(key)).cloned();
        let meta = record.as_ref().map(|r| r.meta.clone()).or_else(|| model.find_meta(key));

        // Identity header on the raised surface: glyph, name, PID and user,
        // then the figures of the view sample in equal columns.
        let header = Rect { pos: rect.pos, size: dvec2(rect.size.x, 62.0) };
        fill(&mut self.draw_rect, cx, header, theme.raised());
        hline(&mut self.draw_rect, cx, header.pos.x, header.pos.y + header.size.y - 1.0, header.size.x, theme.rule());
        let hmid = header.pos.y + header.size.y * 0.5;
        if let Some(meta) = meta.as_ref() {
            kind_glyph(&mut self.draw_round, &mut self.draw_letter, &mut self.draw_icon, cx, &theme, meta, dvec2(rect.pos.x + pad + 15.0, hmid), 30.0);
        }
        let name = meta.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| format!("PID {}", key.pid));
        let ident_x = rect.pos.x + pad + 42.0;
        let ident_w = (rect.size.x * 0.28).clamp(140.0, 300.0);
        self.draw_title.color = theme.foreground;
        text_fit(&mut self.draw_title, cx, dvec2(ident_x, hmid - 20.0), ident_w, &name);
        let mut sub = format!("PID {}", key.pid);
        if let Some(meta) = meta.as_ref() {
            sub.push_str(&format!(" · {}", meta.user));
        }
        if !model.live {
            sub.push_str(&format!(" · at {}", local_hms(model.view_ms())));
        }
        self.draw_mono.color = theme.secondary();
        text_fit(&mut self.draw_mono, cx, dvec2(ident_x, hmid + 4.0), ident_w, &sub);
        let figures: Vec<(String, &str)> = match &record {
            Some(r) => vec![
                (format!("{:.1}%", r.cpu), "CPU"),
                (format_bytes(r.rss), "Memory"),
                (r.threads.to_string(), "Threads"),
                (r.cpu_time().map(format_duration_ns).unwrap_or_else(|| "—".to_string()), "CPU time"),
                (r.state.describe().to_string(), "State"),
            ],
            None => vec![("—".to_string(), "not running at this time")],
        };
        let figures_x = ident_x + ident_w + 20.0;
        let figures_w = rect.pos.x + rect.size.x - pad - figures_x;
        // As many figures as fit whole, in order, measured in their faces;
        // the room left over is shared between them.
        let needs: Vec<f64> = figures.iter().map(|(value, label)| text_width(&self.draw_value, cx, value).max(text_width(&self.draw_small, cx, label)) + 16.0).collect();
        let mut count = 0;
        let mut used = 0.0;
        while count < needs.len() && used + needs[count] <= figures_w {
            used += needs[count];
            count += 1;
        }
        let count = count.max(1);
        let spare = ((figures_w - used) / count as f64).max(0.0);
        let mut fx = figures_x;
        for (index, (value, label)) in figures.iter().take(count).enumerate() {
            let slot = needs[index] + spare;
            self.draw_value.color = theme.foreground;
            text_fit(&mut self.draw_value, cx, dvec2(fx, hmid - 20.0), slot - 12.0, value);
            self.draw_small.color = theme.secondary();
            self.draw_small.draw_abs(cx, dvec2(fx, hmid + 5.0), label);
            fx += slot;
        }

        self.body = Rect { pos: dvec2(rect.pos.x, header.pos.y + header.size.y), size: dvec2(rect.size.x, (rect.size.y - header.size.y).max(0.0)) };
        let tab_index = self.tab.index();
        let scroll = self.scroll[tab_index];
        let start_y = self.body.pos.y + 12.0 - scroll;
        let hscroll = self.hscroll[tab_index];
        self.content_width = 0.0;
        let mut pen = Pen { x: rect.pos.x + pad - hscroll, y: start_y, width: rect.size.x - 2.0 * pad - 8.0, line: 22.0 };
        cx.push_clip_rect(self.body);
        match self.tab {
            InspectorTab::Info => {
                let Some(meta) = meta.clone() else {
                    self.note(cx, &theme, &mut pen, "No record of this process in the retained history.");
                    cx.pop_clip_rect();
                    return DrawStep::done();
                };
                let (detail, note) = self.detail_for(model, key, false);
                let identity = detail_block(detail, |d| &d.identity);
                let parent = view
                    .as_ref()
                    .and_then(|s| s.processes.iter().find(|r| r.meta.key.pid == meta.ppid).map(|r| format!("{} ({})", r.meta.name, meta.ppid)))
                    .unwrap_or_else(|| meta.ppid.to_string());
                let state = record.as_ref().map(|r| r.state.describe().to_string()).unwrap_or_else(|| "not running at this time".to_string());
                let started = if meta.started_secs > 0 { LocalTime::from_epoch_ms(meta.started_secs * 1000).date_hm() } else { "unknown".to_string() };
                self.section(
                    cx,
                    &theme,
                    &mut pen,
                    "Process identity",
                    &[("PID", meta.key.pid.to_string()), ("Parent", parent), ("User", meta.user.clone())],
                    &[("State", state), ("Started", started), ("Kind", if meta.is_app { "application".to_string() } else { "process".to_string() })],
                );
                let path = match &identity {
                    Ok(identity) if !identity.path.is_empty() => identity.path.clone(),
                    Ok(_) => "not readable".to_string(),
                    Err(reason) if !reason.is_empty() => reason.clone(),
                    Err(_) => "not recorded".to_string(),
                };
                self.section(cx, &theme, &mut pen, "Command", &[("Path", path), ("Arguments", meta.cmdline.clone())], &[]);
                // How this was sampled: useful when auditing a view, noise
                // otherwise — behind a disclosure.
                if self.visible(pen.y) {
                    self.draw_icon.color = theme.secondary();
                    text_mid(&mut self.draw_icon, cx, pen.x + 2.0, pen.y + 9.0, if self.details_open { ICON_CHEVRON_DOWN } else { ICON_CHEVRON_RIGHT });
                    self.draw_label.color = theme.secondary();
                    text_mid(&mut self.draw_label, cx, pen.x + 18.0, pen.y + 9.0, "Sampling details");
                    self.details_toggle = Rect { pos: dvec2(pen.x, pen.y - 2.0), size: dvec2(200.0, 22.0) };
                }
                pen.y += 28.0;
                if self.details_open {
                    let mut left = vec![("Identity", if meta.key.verified() { "verified by start time".to_string() } else { "unverified: no start time, never signalled".to_string() })];
                    if let Some(sample) = view.as_ref() {
                        // "journal" when this sample was read back from disk.
                        left.push(("Recorded by", sample.backend.to_string()));
                    }
                    let mut right = Vec::new();
                    if let Ok(identity) = &identity {
                        right.push(("OS status", identity.status.clone()));
                        right.push(("Threads", format!("{} of {} running", identity.running_threads, identity.threads)));
                    }
                    self.section(cx, &theme, &mut pen, "Sampling", &left, &right);
                    self.note(cx, &theme, &mut pen, &note);
                }
            }
            InspectorTab::Activity => self.draw_activity(cx, model, &theme, &mut pen, key),
            InspectorTab::History => self.draw_history(cx, model, &theme, &mut pen, key),
            InspectorTab::Threads => {
                // Nothing recorded yet (the first read is on its way): the
                // latest detail read, as before.
                if !self.draw_threads(cx, model, &theme, &mut pen, key) {
                    let (detail, note) = self.detail_for(model, key, false);
                    self.note(cx, &theme, &mut pen, &note);
                    match detail_block(detail, |d| &d.threads) {
                        Ok(threads) => {
                            let count = |state: ThreadState| threads.iter().filter(|t| t.state == state).count();
                            let stats = [
                                (threads.len(), "threads"),
                                (count(ThreadState::Running), "running"),
                                (count(ThreadState::Waiting), "waiting"),
                                (count(ThreadState::Uninterruptible), "uninterruptible"),
                            ];
                            let slot = (pen.width / stats.len() as f64).min(160.0);
                            for (index, (value, label)) in stats.iter().enumerate() {
                                let x = pen.x + index as f64 * slot;
                                if self.visible(pen.y) {
                                    self.draw_mono.color = theme.foreground;
                                    self.draw_mono.draw_abs(cx, dvec2(x, pen.y), &value.to_string());
                                    self.draw_small.color = theme.secondary();
                                    self.draw_small.draw_abs(cx, dvec2(x, pen.y + 18.0), label);
                                }
                            }
                            pen.y += 44.0;
                            let mut sorted: Vec<_> = threads.iter().collect();
                            sort_threads(&mut sorted, self.thread_sort);
                            // CPU distribution, only where the OS gives a per-thread figure.
                            let total: f64 = threads.iter().filter_map(|t| t.cpu_pct).sum();
                            if threads.iter().all(|t| t.cpu_pct.is_some()) && total > 0.0 {
                                let bar = Rect { pos: dvec2(pen.x, pen.y), size: dvec2(pen.width, 6.0) };
                                let mut by_cpu: Vec<_> = threads.iter().collect();
                                by_cpu.sort_by(|a, b| b.cpu_pct.partial_cmp(&a.cpu_pct).unwrap_or(Ordering::Equal));
                                let colors = [theme.blue, theme.magenta, theme.green, theme.yellow];
                                round(&mut self.draw_round, cx, bar, with_alpha(theme.foreground, 0.08), 3.0);
                                let mut x = bar.pos.x;
                                for (index, thread) in by_cpu.iter().take(4).enumerate() {
                                    let w = bar.size.x * thread.cpu_pct.unwrap_or(0.0) / total;
                                    fill(&mut self.draw_rect, cx, Rect { pos: dvec2(x, bar.pos.y), size: dvec2(w, bar.size.y) }, colors[index]);
                                    x += w;
                                }
                                pen.y += 18.0;
                            }
                            let rows: Vec<Vec<(String, Vec4f)>> = sorted
                                .iter()
                                .map(|t| {
                                    let state_color = if t.state == ThreadState::Running { theme.green } else { theme.secondary() };
                                    vec![
                                        (if t.name.is_empty() { "unnamed".to_string() } else { t.name.clone() }, if t.name.is_empty() { theme.secondary() } else { theme.foreground }),
                                        (t.state.as_str().to_string(), state_color),
                                        (t.cpu_pct.map(|p| format!("{p:.1}")).unwrap_or_else(|| "—".to_string()), theme.foreground),
                                        (t.cpu_time_ns.map(format_duration_ns).unwrap_or_else(|| "—".to_string()), theme.foreground),
                                        (t.id.to_string(), theme.tertiary()),
                                    ]
                                })
                                .collect();
                            let id_title = if cfg!(target_os = "macos") { "Handle" } else { "TID" };
                            self.table_sort = Some((self.thread_sort.column, self.thread_sort.descending));
                            self.table(
                                cx,
                                &theme,
                                &mut pen,
                                &[("Thread", 0.0, false, false), ("State", 120.0, false, false), ("% CPU", 80.0, true, true), ("CPU time", 110.0, true, true), (id_title, 150.0, true, true)],
                                &rows,
                            );
                        }
                        Err(reason) if !reason.is_empty() => self.note(cx, &theme, &mut pen, &reason),
                        Err(_) => {}
                    }
                }
            }
            InspectorTab::Memory => self.draw_memory(cx, model, &theme, &mut pen, key),
            InspectorTab::Files => {
                if !self.draw_files(cx, model, &theme, &mut pen, key) {
                    let (detail, note) = self.detail_for(model, key, false);
                    self.note(cx, &theme, &mut pen, &note);
                    match detail_block(detail, |d| &d.files) {
                        Ok(files) => {
                            let mut kinds: Vec<(&str, usize)> = Vec::new();
                            for file in files.iter() {
                                match kinds.iter_mut().find(|(kind, _)| *kind == file.kind) {
                                    Some(entry) => entry.1 += 1,
                                    None => kinds.push((file.kind, 1)),
                                }
                            }
                            let summary = kinds.iter().map(|(kind, n)| format!("{n} {kind}")).collect::<Vec<_>>().join(" · ");
                            self.text(cx, pen.x, pen.y, pen.width, theme.foreground, &format!("{} descriptors · {summary}", files.len()));
                            pen.y += 28.0;
                            let rows: Vec<Vec<(String, Vec4f)>> = files
                                .iter()
                                .map(|f| vec![(f.fd.to_string(), theme.secondary()), (f.kind.to_string(), theme.secondary()), (f.path.clone(), theme.foreground)])
                                .collect();
                            self.table(cx, &theme, &mut pen, &[("FD", 50.0, true, true), ("Kind", 80.0, false, false), ("Path", 0.0, false, true)], &rows);
                        }
                        Err(reason) if !reason.is_empty() => self.note(cx, &theme, &mut pen, &reason),
                        Err(_) => {}
                    }
                }
            }
            InspectorTab::Ports => {
                let (detail, note) = self.detail_for(model, key, false);
                self.note(cx, &theme, &mut pen, &note);
                match detail_block(detail, |d| &d.ports) {
                    Ok(ports) if ports.is_empty() => self.note(cx, &theme, &mut pen, "No network sockets were open."),
                    Ok(ports) => {
                        let rows: Vec<Vec<(String, Vec4f)>> = ports
                            .iter()
                            .map(|p| vec![(p.protocol.to_string(), theme.secondary()), (p.local.clone(), theme.foreground), (p.remote.clone(), theme.foreground), (p.state.clone(), theme.secondary())])
                            .collect();
                        self.table(cx, &theme, &mut pen, &[("Proto", 64.0, false, false), ("Local", 0.0, false, true), ("Remote", 280.0, false, true), ("State", 120.0, false, false)], &rows);
                    }
                    Err(reason) if !reason.is_empty() => self.note(cx, &theme, &mut pen, &reason),
                    Err(_) => {}
                }
            }
            InspectorTab::Libraries => {
                let (detail, note) = self.detail_for(model, key, true);
                self.note(cx, &theme, &mut pen, &note);
                let libraries = detail.and_then(|d| d.libraries.as_ref());
                match libraries {
                    Some(Detail::Ready(libraries)) => {
                        let total: u64 = libraries.iter().map(|l| l.mapped).sum();
                        self.text(cx, pen.x, pen.y, pen.width, theme.foreground, &format!("{} mapped files · {} mapped", libraries.len(), format_bytes(total)));
                        pen.y += 24.0;
                        if cfg!(target_os = "macos") {
                            self.note(cx, &theme, &mut pen, "System libraries inside the dyld shared cache are one mapping and are not listed individually.");
                        }
                        let rows: Vec<Vec<(String, Vec4f)>> = libraries
                            .iter()
                            .map(|l| vec![(l.path.clone(), theme.foreground), (format_bytes(l.mapped), theme.secondary())])
                            .collect();
                        self.table(cx, &theme, &mut pen, &[("Path", 0.0, false, true), ("Mapped", 100.0, true, true)], &rows);
                    }
                    Some(Detail::Unavailable(reason)) => self.note(cx, &theme, &mut pen, reason),
                    None => {}
                }
            }
        }
        cx.pop_clip_rect();
        self.content_height = pen.y - start_y + 16.0;
        let body = self.body;
        scroll_thumb(&mut self.draw_round, cx, &theme, body, self.content_height, scroll);
        // The sideways thumb, when a table is wider than the inspector.
        let visible_w = body.size.x - 2.0 * pad;
        if self.content_width > visible_w + 1.0 {
            let track = Rect { pos: dvec2(body.pos.x + 4.0, body.pos.y + body.size.y - 6.0), size: dvec2(body.size.x - 8.0, 3.0) };
            let thumb_w = (track.size.x * visible_w / self.content_width).max(24.0);
            let t = (hscroll / (self.content_width - visible_w)).clamp(0.0, 1.0);
            round(&mut self.draw_round, cx, track, with_alpha(theme.foreground, 0.05), 1.5);
            round(&mut self.draw_round, cx, Rect { pos: dvec2(track.pos.x + (track.size.x - thumb_w) * t, track.pos.y), size: dvec2(thumb_w, 3.0) }, with_alpha(theme.foreground, 0.30), 1.5);
        } else if hscroll > 0.0 {
            self.hscroll[tab_index] = 0.0;
        }
        DrawStep::done()
    }
}
