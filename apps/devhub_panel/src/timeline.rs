//! The occupancy timeline: every node the master knows as a swimlane on one time axis. A lane
//! shows the node's leases as thin bars in two to four rows (as many as let every node fit the
//! view; its flagged and running leases first, its history around them, `+n` for what does not
//! fit), its CPU use as a soft area
//! behind them, and in the gutter one header line (name, role, CPU, RAM, GPU and disk meters)
//! over a sparkline of its load. A click on a header expands the node: every lease in its own
//! row, the full header, and its pressure (reserved cores, CPU, RAM, GPU) as four area rows.
//! A node that is down is one line. Above the lanes the fleet: landings and bucket runs, the
//! waiting count over time stacked by what held it, and the requests waiting now.
//!
//! Stuck first: a lease the panel's "needs a look" list flags glows in its severity's colour
//! (red: paused on a panic, its holder stopped beating; amber: the scheduler's `stalled?`), one
//! running past twice its measured run is outlined amber, a flagged queue head glows in the
//! waiting rows. The selected lease glows in the accent colour.
//!
//! The wheel zooms around the pointer (over the gutter it scrolls the lanes), a drag pans, a
//! click on a lease selects it (the panel opens its detail). While following, the right edge
//! is the master's now, drawn as a glowing edge. Tooltips float over the view and are redrawn
//! only when what is under the pointer changes; nothing reflows under the pointer.
//!
//! Cost per draw: per node, a sort and a packing of its leases in the window, one quad per
//! visible bar, and one per area segment (3 px columns, flat runs merged); nothing runs between
//! the master's answers (one a second) but the pointer's changes and a lease's 0.7 s entrance.

use crate::float::{draw_float, Float};
use crate::fmt::{clock, dur, local_offset_s, short};
use crate::model::{blocker_key, pack, pack_capped, Fleet, KEEP_MS, LONG_WAIT_MS};
use crate::needs::Sev;
use makepad_devhub::lease::panel::{Row, Sample};
use makepad_devhub::lease::sched::NodeView;
use makepad_widgets::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // a bar: a rounded fill (hatched for timed and whole-box leases), a crisp outline, and a
    // soft halo `spread` px around it (the quad is that much larger than the bar)
    set_type_default() do #(DrawBar::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: #ffffff
        outline: #00000000
        glow: #00000000
        radius: 2.0
        stripe: 0.0
        spread: 0.0
        pixel: fn() {
            let p = self.pos * self.rect_size
            let sdf = Sdf2d.viewport(p)
            let g = self.spread
            sdf.box(g, g, self.rect_size.x - 2.0 * g, self.rect_size.y - 2.0 * g, self.radius)
            let d = max(sdf.shape, 0.0)
            let halo = self.glow.a * exp(-d / max(g * 0.38, 0.5)) * step(0.0, sdf.shape) * step(0.01, g)
            sdf.clear(vec4(self.glow.rgb, halo))
            let on = step(0.5, fract((p.x + p.y) / 7.0)) * self.stripe
            sdf.fill_keep(mix(self.color, vec4(self.color.rgb * 0.62, self.color.a), on * 0.6))
            sdf.stroke(self.outline, 1.1)
            return sdf.result
        }
    }

    // one segment of an area chart: the band between `lo` and `hi` (shares of the quad's
    // height from its bottom, each going linearly from the left value to the right one),
    // filled from `color` to `color2` left to right, deeper toward its top (`shade`), with a
    // crisp line along its top
    set_type_default() do #(DrawArea::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: #ffffff
        color2: #ffffff
        edge: #00000000
        lo0: 0.0
        lo1: 0.0
        hi0: 1.0
        hi1: 1.0
        shade: 0.55
        pixel: fn() {
            let x = self.pos.x
            let y = 1.0 - self.pos.y
            let h = self.rect_size.y
            let lo = mix(self.lo0, self.lo1, x)
            let hi = mix(self.hi0, self.hi1, x)
            let dt = (hi - y) * h
            let db = (y - lo) * h
            let inside = clamp(dt + 0.5, 0.0, 1.0) * clamp(db + 0.5, 0.0, 1.0)
            let c = mix(self.color, self.color2, x)
            let rise = clamp((y - lo) / max(hi - lo, 0.0001), 0.0, 1.0)
            let a = c.a * mix(1.0 - self.shade, 1.0, rise) * inside
            let line = clamp(1.1 - abs(dt - 0.55), 0.0, 1.0) * self.edge.a * step(0.0005, hi - lo)
            return vec4(self.edge.rgb * line + c.rgb * a * (1.0 - line), line + a * (1.0 - line))
        }
    }

    mod.widgets.FleetTimelineBase = #(FleetTimeline::register_widget(vm))
    mod.widgets.FleetTimeline = set_type_default() do mod.widgets.FleetTimelineBase{
        width: Fill height: Fill
        background: theme.color_chart_ground
        surface: mix(theme.color_chart_ground, theme.color_chart_ink, 0.035)
        edge: mix(theme.color_chart_ground, theme.color_chart_ink, 0.12)
        grid: mix(theme.color_chart_ground, theme.color_chart_ink, 0.07)
        ink: theme.color_chart_ink
        muted: theme.color_chart_ink_dim
        accent: theme.color_focus
        good: theme.color_chart_good
        warn: theme.color_chart_warn
        bad: theme.color_chart_bad
        bar_ink: theme.color_chart_ground
        tip_bg: mix(theme.color_chart_ground, theme.color_chart_ink, 0.92)
        tip_ink: theme.color_chart_ground
        c_build: theme.color_chart_1
        c_test: theme.color_chart_2
        c_gpu: theme.color_chart_3
        c_ci: theme.color_chart_4
        c_gate: theme.color_chart_5
        c_long: theme.color_chart_6
        c_timed: theme.color_chart_7
        c_screen: theme.color_chart_8
        c_inspect: theme.color_chart_ink_dim
        draw_text +: {color: theme.color_chart_ink text_style: theme.font_regular{font_size: 7.5}}
        draw_bold +: {color: theme.color_chart_ink text_style: theme.font_bold{font_size: 7.8}}
        draw_small +: {color: theme.color_chart_ink text_style: theme.font_regular{font_size: 6.8}}
        draw_label +: {color: theme.color_chart_ink text_style: theme.font_regular{font_size: 6.3}}
        draw_tip_text +: {color: theme.color_chart_ink text_style: theme.font_regular{font_size: 7.5}}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawBar {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub outline: Vec4f,
    #[live]
    pub glow: Vec4f,
    #[live]
    pub radius: f32,
    #[live]
    pub stripe: f32,
    #[live]
    pub spread: f32,
}

impl DrawBar {
    /// A plain rounded fill with an outline (no hatch, no halo).
    pub fn plain(&mut self, cx: &mut Cx2d, r: Rect, color: Vec4f, outline: Vec4f, radius: f32) {
        self.color = color;
        self.outline = outline;
        self.glow = vec4(0.0, 0.0, 0.0, 0.0);
        self.spread = 0.0;
        self.stripe = 0.0;
        self.radius = radius;
        self.draw_abs(cx, r);
    }

    /// A fill with a halo of `glow` `spread` px around `r`.
    pub fn glowing(&mut self, cx: &mut Cx2d, r: Rect, color: Vec4f, outline: Vec4f, glow: Vec4f, spread: f64, radius: f32) {
        self.color = color;
        self.outline = outline;
        self.glow = glow;
        self.spread = spread as f32;
        self.stripe = 0.0;
        self.radius = radius;
        self.draw_abs(cx, Rect { pos: r.pos - dvec2(spread, spread), size: r.size + dvec2(spread * 2.0, spread * 2.0) });
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawArea {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub color2: Vec4f,
    #[live]
    pub edge: Vec4f,
    #[live]
    pub lo0: f32,
    #[live]
    pub lo1: f32,
    #[live]
    pub hi0: f32,
    #[live]
    pub hi1: f32,
    #[live]
    pub shade: f32,
}

impl DrawArea {
    /// A smooth area of `vals` (shares 0..1 per column of `col_w` px from `x0`; negative: no
    /// data) over the band `y .. y + h`, a segment between each two columns' centres. Equal
    /// neighbours merge into one quad.
    #[allow(clippy::too_many_arguments)]
    /// `left`: nothing is drawn left of it (a grid's first column starts left of the plot).
    pub fn series(&mut self, cx: &mut Cx2d, x0: f64, left: f64, col_w: f64, y: f64, h: f64, vals: &[f32], color_of: &dyn Fn(f32) -> Vec4f, edge_alpha: f32) {
        let mut i = 0;
        while i + 1 < vals.len() {
            let (a, b) = (vals[i], vals[i + 1]);
            if a < 0.0 || b < 0.0 || (a <= 0.0 && b <= 0.0) {
                i += 1;
                continue;
            }
            // a flat run: one quad to its end
            let mut j = i + 1;
            if (a - b).abs() < 1e-4 {
                while j + 1 < vals.len() && (vals[j + 1] - a).abs() < 1e-4 {
                    j += 1;
                }
            }
            let (xa, xb) = (x0 + (i as f64 + 0.5) * col_w, x0 + (j as f64 + 0.5) * col_w);
            let (va, vb) = (a.min(1.0), vals[j].min(1.0));
            if xb <= left {
                i = j;
                continue;
            }
            let (xa, va) = trim(xa, xb, va, vb, left);
            let (ca, cb) = (color_of(va), color_of(vb));
            self.color = ca;
            self.color2 = cb;
            self.edge = vec4(cb.x, cb.y, cb.z, edge_alpha);
            self.lo0 = 0.0;
            self.lo1 = 0.0;
            self.hi0 = va;
            self.hi1 = vb;
            self.shade = 0.55;
            self.draw_abs(cx, Rect { pos: dvec2(xa, y), size: dvec2(xb - xa, h) });
            i = j;
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum FleetTimelineAction {
    /// the view was zoomed or panned away from now
    Unfollowed,
    /// a lease was clicked (None: a click on nothing let the selection go)
    Selected(Option<String>),
    #[default]
    None,
}

const RULER: f64 = 18.0;
pub const GUTTER: f64 = 272.0;
const ROW_H: f64 = 11.0;
const BAR_H: f64 = 9.0;
const GHOST_PITCH: f64 = 10.0;
const GHOST_H: f64 = 8.0;
const LINE_H: f64 = 10.5;
/// an expanded node's header lines
const HEAD_LINES: f64 = 5.0;
/// a collapsed lane's rows: at least two, more while every node still fits the view
const MIN_ROWS: usize = 2;
const MAX_ROWS: usize = 4;
/// a collapsed lane's height past its rows
const LANE_PAD: f64 = 5.0;
const DOWN_H: f64 = 15.0;
const TRAIN_H: f64 = 14.0;
const AREA_H: f64 = 28.0;
/// an area chart's column
const COL_W: f64 = 3.0;

/// The columns the areas bin into: [`COL_W`] px of the zoom's ms each, on a grid anchored to
/// absolute time, so a pan slides them with the bars and landings (the same time-to-x) and
/// re-bins nothing; only a zoom changes them. Counted from the view's left edge, every fractional
/// pan gave the columns new boundaries, and each area wobbled as it moved (11-08).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Grid {
    /// the first column's start, ms (at or before the view's left edge)
    t0: f64,
    /// a column's width, ms
    per: f64,
    /// the first column's x, at or left of the plot's left edge
    x0: f64,
    /// columns to the right edge and one past it
    n: usize,
}

impl Grid {
    fn of(t_left: f64, ms_per_px: f64, plot_x: f64, right: f64) -> Grid {
        let per = ms_per_px.max(1e-6) * COL_W;
        let t0 = (t_left / per).floor() * per;
        let x0 = plot_x + (t0 - t_left) / ms_per_px.max(1e-6);
        Grid { t0, per, x0, n: ((right - x0) / COL_W).ceil().max(0.0) as usize + 1 }
    }

    /// The column `t` falls in, fractional (left of the first: negative).
    fn col(&self, t: u64) -> f64 {
        (t as f64 - self.t0) / self.per
    }
}

/// A column's x and value cut at `left`: the part of a quad from `xa` to `xb` right of it.
fn trim(xa: f64, xb: f64, va: f32, vb: f32, left: f64) -> (f64, f32) {
    if xa >= left || xb <= xa {
        return (xa, va);
    }
    let t = ((left - xa) / (xb - xa)) as f32;
    (left, va + (vb - va) * t)
}

/// The samples' values per column of `per` ms from `t0` (each the most of those it covers; -1 for
/// none): a sample holds until the next one, the last until `now`.
fn columns(samples: &[Sample], t0: f64, per: f64, n: usize, now: u64, v: &dyn Fn(&Sample) -> f32) -> Vec<f32> {
    let mut out = vec![-1.0f32; n];
    if n == 0 || per <= 0.0 {
        return out;
    }
    let t1 = t0 + per * n as f64;
    let a = samples.partition_point(|s| (s.t_ms as f64) < t0).saturating_sub(1);
    for (i, s) in samples[a..].iter().enumerate() {
        if s.t_ms as f64 > t1 {
            break;
        }
        let next = (samples.get(a + i + 1).map_or(now, |x| x.t_ms) as f64).min(t1);
        let from = (s.t_ms as f64).max(t0);
        if next <= from {
            continue;
        }
        let c0 = ((from - t0) / per) as usize;
        let c1 = (((next - t0) / per).ceil() as usize).min(n);
        let val = v(s);
        if val < 0.0 {
            continue;
        }
        for o in out.iter_mut().take(c1).skip(c0.min(n)) {
            *o = o.max(val);
        }
    }
    out
}
const PRESS_ROW: f64 = 9.0;
const PRESS_ROWS: [&str; 4] = ["reserved", "cpu", "ram", "gpu"];
/// how long a lease that just started or ended glows: one draw shows it and one ends it (a whole
/// timeline draws each time, so no animation frames in between)
const FRESH_S: f64 = 1.5;
const SPARK_W: f64 = 176.0;

#[derive(Clone, Copy, Debug)]
struct Pan {
    start_x: f64,
    start_right: f64,
    start_scroll: f64,
    start_y: f64,
    moved: bool,
}

/// What is under a point of the last draw.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pick {
    /// a lease's run bar or its queue-wait ghost (index into the fleet's rows)
    Lease(usize, bool),
    /// a node's lane, behind its bars: its pressure (node index)
    Press(usize),
    /// a landing (index into the fleet's land entries)
    Landing(usize),
    /// a bucket run (index into the fleet's buckets)
    Bucket(usize),
    /// a node's header (node index)
    Head(usize),
    /// a node's leases that do not fit its collapsed rows
    Hidden(usize),
    /// the waiting count over time
    Queue,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Collapsed,
    Expanded,
    Down,
}

/// One node's lane as laid out: where, how, and its leases placed in rows.
struct Lane {
    ni: usize,
    y: f64,
    h: f64,
    mode: Mode,
    bars_h: f64,
    /// (row index in the fleet, row in the lane)
    placed: Vec<(usize, usize)>,
    /// leases in view that do not fit (and how many of those run)
    hidden: Vec<usize>,
    hidden_running: usize,
}

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct FleetTimeline {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    draw_shape: DrawColor,
    #[live]
    draw_bar: DrawBar,
    #[live]
    draw_area: DrawArea,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_bold: DrawText,
    #[live]
    draw_small: DrawText,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_tip: DrawBar,
    #[live]
    draw_tip_text: DrawText,
    #[live]
    background: Vec4f,
    #[live]
    surface: Vec4f,
    #[live]
    edge: Vec4f,
    #[live]
    grid: Vec4f,
    #[live]
    ink: Vec4f,
    #[live]
    muted: Vec4f,
    #[live]
    accent: Vec4f,
    #[live]
    good: Vec4f,
    #[live]
    warn: Vec4f,
    #[live]
    bad: Vec4f,
    #[live]
    bar_ink: Vec4f,
    #[live]
    tip_bg: Vec4f,
    #[live]
    tip_ink: Vec4f,
    #[live]
    c_build: Vec4f,
    #[live]
    c_test: Vec4f,
    #[live]
    c_gpu: Vec4f,
    #[live]
    c_ci: Vec4f,
    #[live]
    c_gate: Vec4f,
    #[live]
    c_long: Vec4f,
    #[live]
    c_timed: Vec4f,
    #[live]
    c_screen: Vec4f,
    #[live]
    c_inspect: Vec4f,
    #[rust]
    area: Area,
    #[rust]
    view: Rect,
    #[rust]
    fleet: Option<Arc<Fleet>>,
    /// the view's right edge (master ms) and scale
    #[rust]
    t_right: f64,
    #[rust]
    ms_per_px: f64,
    /// the window a fresh view shows
    #[rust(3_600_000.0)]
    window_ms: f64,
    #[rust(true)]
    follow: bool,
    /// what the pointer is over (and, over an area, its column): the tooltip changes with it
    #[rust]
    hover: Option<(Pick, i64)>,
    /// where the pointer was when `hover` last changed: the tooltip's anchor
    #[rust]
    hover_at: DVec2,
    /// the tooltip's overlay
    #[rust]
    float: Option<DrawList2d>,
    #[rust]
    pan: Option<Pan>,
    #[rust]
    scroll: f64,
    #[rust]
    selected: Option<String>,
    /// the needs list's leases: severity and tag
    #[rust]
    flags: HashMap<String, (Sev, &'static str)>,
    /// nodes shown expanded (a click on the header)
    #[rust]
    expanded: HashSet<String>,
    /// a node to bring into view at the next draw (with the lease that asked)
    #[rust]
    reveal: Option<(String, String)>,
    /// leases that just started or ended: since when (seconds of `clock`)
    #[rust]
    fresh: HashMap<String, f64>,
    /// the draw that ends the oldest glow
    #[rust]
    fresh_end: Timer,
    #[rust]
    epoch: Option<Instant>,
    /// what the last draw put where
    #[rust]
    picks: Vec<(Rect, Pick)>,
    /// the leases' caps and labels, drawn after every bar: one draw call each, not one per lease
    #[rust]
    caps_later: Vec<(Rect, Vec4f)>,
    #[rust]
    labels_later: Vec<(Rect, String, Vec4f)>,
    #[rust]
    lanes_top: f64,
    #[rust]
    content_h: f64,
}

impl FleetTimeline {
    pub fn set_fleet(&mut self, cx: &mut Cx, f: Arc<Fleet>) {
        // the leases granted or ended since the last answer glow for a moment (after a hidden
        // stretch only those of the glow's own last moment: the rest happened unseen)
        let prev = self.fleet.as_ref().map_or(0, |p| p.now_ms);
        let t = self.clock();
        if prev > 0 {
            let prev = prev.max(f.now_ms.saturating_sub((FRESH_S * 1000.0) as u64));
            for r in &f.rows {
                if r.ended_ms.is_some_and(|e| e > prev) || (r.ended_ms.is_none() && r.granted_ms.is_some_and(|g| g > prev)) {
                    self.fresh.insert(r.id.clone(), t);
                }
            }
        }
        self.end_fresh(cx, t);
        self.fleet = Some(f);
        self.area.redraw(cx);
    }

    /// The needs list's leases, so the stuck ones glow.
    pub fn set_flags(&mut self, cx: &mut Cx, flags: HashMap<String, (Sev, &'static str)>) {
        if flags != self.flags {
            self.flags = flags;
            self.area.redraw(cx);
        }
    }

    /// Shows the last `ms` and follows now.
    pub fn set_window(&mut self, cx: &mut Cx, ms: f64) {
        self.window_ms = ms;
        self.ms_per_px = 0.0;
        self.follow = true;
        self.area.redraw(cx);
    }

    pub fn set_follow(&mut self, cx: &mut Cx, on: bool) {
        self.follow = on;
        self.area.redraw(cx);
    }

    pub fn follow(&self) -> bool {
        self.follow
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Selects a lease and frames it: a running or waiting one with now at the right edge and
    /// its whole run in view, an ended one centred; its node scrolled into view (and expanded
    /// when its collapsed rows hide it).
    pub fn frame_lease(&mut self, cx: &mut Cx, id: &str) {
        let Some(f) = self.fleet.clone() else { return };
        let Some(r) = f.row(id) else { return };
        self.selected = Some(id.to_string());
        let plot_w = self.plot_w();
        let start = r.asked_ms as f64;
        if r.ended_ms.is_none() {
            let want = (f.now_ms as f64 - start) * 1.3 + 120_000.0;
            self.window_ms = want.clamp(15.0 * 60_000.0, KEEP_MS as f64);
            self.ms_per_px = self.window_ms / plot_w;
            self.follow = true;
        } else {
            let end = f.end_ms(r) as f64;
            let span = (end - start).max(60_000.0);
            self.ms_per_px = (span * 3.0 / plot_w).clamp(200.0, KEEP_MS as f64 / plot_w);
            self.t_right = end + span;
            self.unfollow(cx);
        }
        if !r.node.is_empty() {
            self.reveal = Some((r.node.clone(), r.id.clone()));
        }
        self.area.redraw(cx);
    }

    /// Lets the selection go.
    pub fn clear_selection(&mut self, cx: &mut Cx) {
        if self.selected.take().is_some() {
            self.area.redraw(cx);
        }
    }

    /// Brings a node into view (a need about a node, not a lease).
    pub fn reveal_node(&mut self, cx: &mut Cx, node: &str) {
        self.reveal = Some((node.to_string(), String::new()));
        self.area.redraw(cx);
    }

    /// Lets the glows older than [`FRESH_S`] go, and asks for the draw that ends the oldest left.
    fn end_fresh(&mut self, cx: &mut Cx, t: f64) {
        self.fresh.retain(|_, s| t - *s < FRESH_S);
        if self.fresh_end.is_empty() {
            if let Some(oldest) = self.fresh.values().copied().reduce(f64::min) {
                self.fresh_end = cx.start_timeout((oldest + FRESH_S - t).max(0.05));
            }
        }
    }

    fn clock(&mut self) -> f64 {
        self.epoch.get_or_insert_with(Instant::now).elapsed().as_secs_f64()
    }

    fn plot_x(&self) -> f64 {
        self.view.pos.x + GUTTER
    }

    fn plot_w(&self) -> f64 {
        (self.view.size.x - GUTTER).max(10.0)
    }

    fn right(&self) -> f64 {
        self.view.pos.x + self.view.size.x
    }

    fn t_left(&self) -> f64 {
        self.t_right - self.ms_per_px * self.plot_w()
    }

    fn col_grid(&self) -> Grid {
        Grid::of(self.t_left(), self.ms_per_px, self.plot_x(), self.right())
    }

    fn x_of(&self, t: u64) -> f64 {
        self.plot_x() + (t as f64 - self.t_left()) / self.ms_per_px.max(1e-6)
    }

    fn t_of(&self, x: f64) -> f64 {
        self.t_left() + (x - self.plot_x()) * self.ms_per_px
    }

    pub fn kind_color(&self, kind: &str) -> Vec4f {
        match kind {
            "build" => self.c_build,
            "test" => self.c_test,
            "gpu" => self.c_gpu,
            "ci" => self.c_ci,
            "gate" => self.c_gate,
            "long" => self.c_long,
            "timed" => self.c_timed,
            "screen" => self.c_screen,
            _ => self.c_inspect,
        }
    }

    fn sev_color(&self, s: Sev) -> Vec4f {
        match s {
            Sev::Red => self.bad,
            Sev::Amber => self.warn,
            Sev::Info => self.muted,
        }
    }

    fn rect(&mut self, cx: &mut Cx2d, x0: f64, y: f64, x1: f64, h: f64, color: Vec4f) {
        self.draw_shape.color = color;
        self.draw_shape.draw_abs(cx, Rect { pos: dvec2(x0, y), size: dvec2((x1 - x0).max(1.0), h) });
    }

    fn width_of(draw: &DrawText, cx: &mut Cx2d, s: &str) -> f64 {
        draw.layout(cx, 0.0, 0.0, None, false, Align::default(), s).size_in_lpxs.width as f64
    }

    /// Draws `s` clipped to `max_w` (no cut strings: a label's layout is made once); its width.
    fn text_in(draw: &mut DrawText, cx: &mut Cx2d, x: f64, y: f64, max_w: f64, s: &str, color: Vec4f) -> f64 {
        if max_w < 6.0 || s.is_empty() {
            return 0.0;
        }
        draw.color = color;
        let w = Self::width_of(draw, cx, s);
        if w <= max_w {
            draw.draw_abs(cx, dvec2(x, y), s);
            return w;
        }
        cx.push_clip_rect(Rect { pos: dvec2(x, y - 4.0), size: dvec2(max_w, 24.0) });
        draw.draw_abs(cx, dvec2(x, y), s);
        cx.pop_clip_rect();
        max_w
    }

    fn text(&mut self, cx: &mut Cx2d, x: f64, y: f64, max_w: f64, s: &str, color: Vec4f) -> f64 {
        Self::text_in(&mut self.draw_text, cx, x, y, max_w, s, color)
    }

    fn small(&mut self, cx: &mut Cx2d, x: f64, y: f64, max_w: f64, s: &str, color: Vec4f) -> f64 {
        Self::text_in(&mut self.draw_small, cx, x, y, max_w, s, color)
    }

    fn bold(&mut self, cx: &mut Cx2d, x: f64, y: f64, max_w: f64, s: &str, color: Vec4f) -> f64 {
        Self::text_in(&mut self.draw_bold, cx, x, y, max_w, s, color)
    }

    fn small_right(&mut self, cx: &mut Cx2d, right: f64, y: f64, s: &str, color: Vec4f) -> f64 {
        let w = Self::width_of(&self.draw_small, cx, s);
        self.draw_small.color = color;
        self.draw_small.draw_abs(cx, dvec2(right - w, y), s);
        w
    }

    /// Heat for a share of a budget: cool and calm below half, warm past it, hot past 90 %.
    fn heat(&self, share: f32) -> Vec4f {
        let s = share.clamp(0.0, 1.2);
        let cool = mix(self.c_long, self.good, 0.5);
        let c = if s < 0.6 { mix(cool, self.warn, (s / 0.6).powf(2.0) * 0.3) } else if s < 0.9 { mix(cool, self.warn, 0.3 + (s - 0.6) / 0.3 * 0.7) } else { mix(self.warn, self.bad, ((s - 0.9) / 0.2).min(1.0)) };
        vec4(c.x, c.y, c.z, 1.0)
    }

    fn with_alpha(c: Vec4f, a: f32) -> Vec4f {
        vec4(c.x, c.y, c.z, a)
    }

    /// The window's six biggest blockers in the chart series' hues, the rest as one grey.
    fn blocker_color(&self, i: usize) -> Vec4f {
        let series = [self.c_gate, self.c_build, self.c_ci, self.c_gpu, self.c_long, self.c_timed];
        match series.get(i) {
            Some(&c) => Self::with_alpha(c, 0.85),
            None => Self::with_alpha(mix(self.background, self.ink, 0.35), 0.78),
        }
    }

    /// A value per column over `[t0, t1]` from a node's samples (each holds until the next):
    /// the column's highest; negative where no sample covers it.
    fn draw_fleet(&mut self, cx: &mut Cx2d, f: &Fleet) {
        self.picks.clear();
        let view = self.view;
        let plot_x = self.plot_x();
        let plot_w = self.plot_w();
        let right = self.right();
        if self.ms_per_px <= 0.0 {
            self.ms_per_px = self.window_ms / plot_w;
        }
        if self.follow || self.t_right == 0.0 {
            // a little room right of now, so the live edge reads as live
            self.t_right = f.now_ms as f64 + self.ms_per_px * 16.0;
        }
        let t0 = self.t_left().max(0.0) as u64;
        let t1 = self.t_right.max(0.0) as u64;
        let offset = local_offset_s(f.now_ms);
        let top = view.pos.y;
        // the gutter's ground, a shade off the plot's
        let surface = self.surface;
        self.rect(cx, view.pos.x, top, plot_x, view.size.y, surface);
        // the ruler: clock times at a nice step
        self.rect(cx, view.pos.x, top + RULER - 1.0, right, 1.0, self.edge);
        let step_min = nice_minutes(self.ms_per_px * 110.0 / 60_000.0);
        let step = (step_min * 60_000.0) as u64;
        let mut t = (t0 / step) * step;
        let grid_h = view.size.y - RULER;
        while t <= t1 {
            let x = self.x_of(t);
            if x >= plot_x {
                self.rect(cx, x, top + RULER, x + 1.0, grid_h, self.grid);
                self.rect(cx, x, top + RULER - 4.0, x + 1.0, 4.0, self.edge);
                let label = clock(t, offset, step < 60_000);
                let muted = self.muted;
                self.small(cx, x + 3.0, top + 4.0, 80.0, &label, muted);
            }
            t += step;
        }
        let window = format!("{} \u{2192} {} \u{b7} {}", clock(t0, offset, false), clock(t1.min(f.now_ms), offset, false), dur(t1.saturating_sub(t0)));
        let muted = self.muted;
        self.small(cx, view.pos.x + 8.0, top + 4.0, GUTTER - 16.0, &window, muted);

        // the fleet: landings and buckets, the waiting count by blocker, the waiting requests
        let mut y = top + RULER + 3.0;
        y = self.draw_train(cx, f, y, t0, t1);
        y = self.draw_queue_area(cx, f, y, t0, t1);
        y = self.draw_waiting(cx, f, y, t0, t1);
        self.rect(cx, view.pos.x, y + 1.0, right, 1.0, self.edge);
        self.lanes_top = y + 2.0;

        // the lanes, scrolled under the fleet section
        let clip = Rect { pos: dvec2(view.pos.x, self.lanes_top), size: dvec2(view.size.x, (view.pos.y + view.size.y - self.lanes_top).max(0.0)) };
        let room = clip.size.y - 6.0;
        let mut lanes = self.layout(f, t0, t1, room);
        if let Some((node, lease)) = self.reveal.take() {
            // a lease its node's collapsed rows hide: the node opens
            if let Some(l) = lanes.iter().find(|l| f.nodes[l.ni].name == node) {
                if !lease.is_empty() && l.mode == Mode::Collapsed && !l.placed.iter().any(|(i, _)| f.rows[*i].id == lease) {
                    self.expanded.insert(node.clone());
                    lanes = self.layout(f, t0, t1, room);
                }
            }
            if let Some(l) = lanes.iter().find(|l| f.nodes[l.ni].name == node) {
                let room = clip.size.y;
                if l.y < self.scroll || l.y + l.h > self.scroll + room {
                    self.scroll = (l.y - room * 0.3).max(0.0);
                }
            }
        }
        self.content_h = lanes.last().map_or(0.0, |l| l.y + l.h) + 6.0;
        self.clamp_scroll();
        cx.push_clip_rect(clip);
        let base = self.lanes_top + 3.0 - self.scroll;
        let now_t = self.clock();
        for l in &lanes {
            let y = base + l.y;
            if y + l.h >= clip.pos.y && y <= clip.pos.y + clip.size.y {
                self.draw_lane(cx, f, l, y, t0, t1, now_t);
            }
        }
        for (r, c) in std::mem::take(&mut self.caps_later) {
            self.rect(cx, r.pos.x, r.pos.y, r.pos.x + r.size.x, r.size.y, c);
        }
        for (r, label, c) in std::mem::take(&mut self.labels_later) {
            self.label(cx, r, &label, c);
        }
        cx.pop_clip_rect();
        // the live edge: now, glowing into the past
        let nx = self.x_of(f.now_ms);
        if nx >= plot_x && nx <= right + 1.0 {
            let a = self.accent;
            let h = view.size.y - RULER;
            self.draw_area.color = Self::with_alpha(a, 0.0);
            self.draw_area.color2 = Self::with_alpha(a, 0.09);
            self.draw_area.edge = vec4(0.0, 0.0, 0.0, 0.0);
            self.draw_area.lo0 = 0.0;
            self.draw_area.lo1 = 0.0;
            self.draw_area.hi0 = 1.0;
            self.draw_area.hi1 = 1.0;
            self.draw_area.shade = 0.0;
            let gx = (nx - 36.0).max(plot_x);
            self.draw_area.draw_abs(cx, Rect { pos: dvec2(gx, top + RULER), size: dvec2(nx - gx, h) });
            self.rect(cx, nx - 0.5, top + RULER, nx + 1.0, h, Self::with_alpha(a, 0.9));
            self.draw_bar.plain(cx, Rect { pos: dvec2(nx - 3.0, top + RULER - 6.0), size: dvec2(7.0, 7.0) }, a, vec4(0.0, 0.0, 0.0, 0.0), 3.5);
        }
        // the gutter's edge
        let edge = self.edge;
        self.rect(cx, plot_x - 1.0, top + RULER, plot_x, view.size.y - RULER, edge);
    }

    /// Every node's lane: order, mode, height, and its leases placed in rows. The collapsed
    /// lanes share the `room` the expanded and down ones leave: two to four rows each.
    fn layout(&self, f: &Fleet, t0: u64, t1: u64, room: f64) -> Vec<Lane> {
        let mut order: Vec<usize> = (0..f.nodes.len()).collect();
        // the boxes that are up first, the down ones last; else the master's order
        order.sort_by_key(|&i| !f.nodes[i].up);
        let mut by_node: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, r) in f.rows.iter().enumerate() {
            if !r.node.is_empty() && r.granted_ms.is_some() && r.asked_ms <= t1 && f.end_ms(r) >= t0 {
                by_node.entry(r.node.as_str()).or_default().push(i);
            }
        }
        let pad = (self.ms_per_px * 2.0) as u64;
        let cap = {
            let down = f.nodes.iter().filter(|n| !n.up).count() as f64;
            let collapsed = f.nodes.iter().filter(|n| n.up && !self.expanded.contains(&n.name)).count().max(1) as f64;
            // an expanded lane takes about its header and pressure rows; a busy one more
            let expanded = f.nodes.iter().filter(|n| n.up && self.expanded.contains(&n.name)).count() as f64 * (HEAD_LINES * LINE_H + PRESS_ROWS.len() as f64 * (PRESS_ROW + 1.0) + 10.0);
            let per = (room - down * (DOWN_H + 1.0) - expanded) / collapsed - LANE_PAD - 1.0;
            ((per / ROW_H).floor().max(0.0) as usize).clamp(MIN_ROWS, MAX_ROWS)
        };
        let mut y = 0.0;
        let mut out = Vec::with_capacity(order.len());
        for ni in order {
            let n = &f.nodes[ni];
            let visible = by_node.get(n.name.as_str()).cloned().unwrap_or_default();
            let span = |i: usize| (f.rows[i].granted_ms.unwrap_or(f.rows[i].asked_ms), f.end_ms(&f.rows[i]) + pad);
            let mut lane = Lane { ni, y, h: DOWN_H, mode: Mode::Down, bars_h: 0.0, placed: Vec::new(), hidden: Vec::new(), hidden_running: 0 };
            if !n.up {
                lane.hidden = visible;
            } else if self.expanded.contains(&n.name) {
                let mut v = visible;
                v.sort_by_key(|&i| span(i).0);
                let spans: Vec<(u64, u64)> = v.iter().map(|&i| span(i)).collect();
                let rows = pack(&spans);
                let n_rows = rows.iter().copied().max().map_or(0, |m| m + 1);
                lane.placed = v.into_iter().zip(rows).collect();
                lane.mode = Mode::Expanded;
                lane.bars_h = (n_rows as f64 * ROW_H).max(HEAD_LINES * LINE_H + 2.0);
                lane.h = lane.bars_h + PRESS_ROWS.len() as f64 * (PRESS_ROW + 1.0) + 8.0;
            } else {
                // the selected and flagged leases first, then what runs, then the newest history
                let rank = |i: usize| {
                    let r = &f.rows[i];
                    if self.selected.as_deref() == Some(r.id.as_str()) {
                        0
                    } else if self.flags.contains_key(&r.id) {
                        1
                    } else if r.ended_ms.is_none() {
                        2
                    } else {
                        3
                    }
                };
                let mut v = visible;
                v.sort_by_key(|&i| (rank(i), if rank(i) < 3 { span(i).0 } else { u64::MAX - span(i).1 }));
                let spans: Vec<(u64, u64)> = v.iter().map(|&i| span(i)).collect();
                for (i, row) in v.into_iter().zip(pack_capped(&spans, cap)) {
                    match row {
                        Some(row) => lane.placed.push((i, row)),
                        None => {
                            if f.rows[i].ended_ms.is_none() {
                                lane.hidden_running += 1;
                            }
                            lane.hidden.push(i);
                        }
                    }
                }
                lane.mode = Mode::Collapsed;
                lane.bars_h = cap as f64 * ROW_H;
                lane.h = lane.bars_h + LANE_PAD;
            }
            y += lane.h + 1.0;
            out.push(lane);
        }
        out
    }

    fn draw_train(&mut self, cx: &mut Cx2d, f: &Fleet, y: f64, t0: u64, t1: u64) -> f64 {
        let x = self.view.pos.x + 8.0;
        let in_window = f.land.iter().filter(|e| e.changed_ms >= t0 && e.changed_ms <= t1).count();
        let ink = self.ink;
        let w = self.small(cx, x, y + 2.0, 80.0, "LANDINGS", ink);
        let muted = self.muted;
        self.small(cx, x + w + 6.0, y + 2.0, GUTTER - w - 22.0, &format!("{in_window} in view"), muted);
        let (plot_x, right) = (self.plot_x(), self.right());
        // bucket runs as thin bars at the band's bottom
        for (i, b) in f.buckets.iter().enumerate() {
            if b.ended_ms < t0 || b.started_ms > t1 {
                continue;
            }
            let x0 = self.x_of(b.started_ms).max(plot_x);
            let x1 = self.x_of(b.ended_ms).min(right).max(x0 + 2.0);
            let c = if b.ok { self.good } else { self.bad };
            self.draw_bar.plain(cx, Rect { pos: dvec2(x0, y + TRAIN_H - 4.0), size: dvec2(x1 - x0, 3.0) }, Self::with_alpha(c, 0.75), vec4(0.0, 0.0, 0.0, 0.0), 1.5);
            self.picks.push((Rect { pos: dvec2(x0, y + TRAIN_H - 6.0), size: dvec2(x1 - x0, 7.0) }, Pick::Bucket(i)));
        }
        // landings as ticks: landed, failed, still queued
        for (i, e) in f.land.iter().enumerate() {
            use makepad_devhub::land::St;
            let at = if e.state == St::Queued { e.asked_ms } else { e.changed_ms };
            if at < t0 || at > t1 {
                continue;
            }
            let x = self.x_of(at);
            let c = match e.state {
                St::Landed => self.good,
                St::Failed | St::Conflict => self.bad,
                St::Waiting => self.warn,
                St::Queued => self.accent,
            };
            let r = Rect { pos: dvec2(x - 2.5, y + 1.0), size: dvec2(5.0, 8.0) };
            if matches!(e.state, St::Failed | St::Conflict) {
                self.draw_bar.glowing(cx, r, c, vec4(0.0, 0.0, 0.0, 0.0), Self::with_alpha(c, 0.5), 3.0, 2.0);
            } else {
                self.draw_bar.plain(cx, r, c, vec4(0.0, 0.0, 0.0, 0.0), 2.0);
            }
            self.picks.push((Rect { pos: dvec2(x - 5.0, y), size: dvec2(10.0, TRAIN_H) }, Pick::Landing(i)));
        }
        y + TRAIN_H + 1.0
    }

    /// The waiting count over time, stacked by blocker, as a smooth area; the window's wait
    /// quantiles in the gutter.
    fn draw_queue_area(&mut self, cx: &mut Cx2d, f: &Fleet, y: f64, t0: u64, t1: u64) -> f64 {
        let x = self.view.pos.x + 8.0;
        let q = f.queued();
        let long = q.iter().filter(|r| f.wait_ms(r) >= LONG_WAIT_MS).count();
        let ink = self.ink;
        let w = self.bold(cx, x, y + 1.0, 80.0, "QUEUE", ink);
        let (c, muted) = (if long > 0 { self.warn } else { self.ink }, self.muted);
        let line = if long > 0 { format!("{} waiting \u{b7} {long} over 10 min", q.len()) } else { format!("{} waiting", q.len()) };
        self.text(cx, x + w + 6.0, y + 1.5, GUTTER - w - 22.0, &line, c);
        let stats = match f.wait_stats(t0, t1) {
            Some((p50, p90, max, n)) => format!("wait p50 {} \u{b7} p90 {} \u{b7} max {} ({n})", dur(p50), dur(p90), dur(max)),
            None => "no waits in view".to_string(),
        };
        self.small(cx, x, y + 14.0, GUTTER - 16.0, &stats, muted);
        // per column, the waiting leases by blocker (the window's six biggest, the rest as one)
        let blockers = f.blockers(t0, t1);
        let top: Vec<&str> = blockers.iter().take(6).map(|(b, _)| b.as_str()).collect();
        let plot_x = self.plot_x();
        let grid = self.col_grid();
        let cols = grid.n;
        let mut counts = vec![[0u16; 7]; cols];
        let col_of = |t: u64| grid.col(t);
        for r in &f.rows {
            for w in &r.waits {
                let end = if w.end_ms == 0 { f.now_ms } else { w.end_ms };
                if end < t0 || w.start_ms > t1 {
                    continue;
                }
                let k = top.iter().position(|b| *b == blocker_key(r, &w.blocker)).unwrap_or(6);
                let a = col_of(w.start_ms).max(0.0).floor() as usize;
                let b = (col_of(end).ceil().max(0.0) as usize).min(cols);
                for c in counts.iter_mut().take(b).skip(a) {
                    c[k] += 1;
                }
            }
        }
        let peak = counts.iter().map(|c| c.iter().map(|&v| v as u32).sum::<u32>()).max().unwrap_or(0).max(4) as f32;
        let base = y + AREA_H;
        let line = self.grid;
        self.rect(cx, plot_x, base, self.right(), 1.0, line);
        // the stacked layers, each a smooth band between the ones below and its own top
        let mut lo = vec![0.0f32; cols];
        for k in 0..7 {
            let hi: Vec<f32> = (0..cols).map(|i| lo[i] + counts[i][k] as f32 / peak).collect();
            let col = self.blocker_color(k);
            let mut i = 0;
            while i + 1 < cols {
                if hi[i] <= lo[i] && hi[i + 1] <= lo[i + 1] {
                    i += 1;
                    continue;
                }
                let (xa, xb) = (grid.x0 + (i as f64 + 0.5) * COL_W, grid.x0 + (i as f64 + 1.5) * COL_W);
                if xb <= plot_x {
                    i += 1;
                    continue;
                }
                let (xa_cut, lo0) = trim(xa, xb, lo[i], lo[i + 1], plot_x);
                let (_, hi0) = trim(xa, xb, hi[i], hi[i + 1], plot_x);
                self.draw_area.color = Self::with_alpha(col, 0.7);
                self.draw_area.color2 = Self::with_alpha(col, 0.7);
                self.draw_area.edge = Self::with_alpha(col, 0.95);
                self.draw_area.lo0 = lo0;
                self.draw_area.lo1 = lo[i + 1];
                self.draw_area.hi0 = hi0;
                self.draw_area.hi1 = hi[i + 1];
                self.draw_area.shade = 0.5;
                self.draw_area.draw_abs(cx, Rect { pos: dvec2(xa_cut, base - (AREA_H - 3.0)), size: dvec2(xb - xa_cut, AREA_H - 3.0) });
                i += 1;
            }
            lo = hi;
        }
        self.small_right(cx, plot_x - 6.0, base - 9.0, &format!("{peak:.0}"), muted);
        self.picks.push((Rect { pos: dvec2(plot_x, y), size: dvec2(self.plot_w(), AREA_H) }, Pick::Queue));
        base + 3.0
    }

    /// The requests waiting now, as ghost bars from their request to now; a flagged one glows.
    fn draw_waiting(&mut self, cx: &mut Cx2d, f: &Fleet, y: f64, t0: u64, t1: u64) -> f64 {
        // two rows at least, so the lanes below do not jump as the queue empties and fills
        let q: Vec<usize> = f.rows.iter().enumerate().filter(|(_, r)| r.state == "queued").map(|(i, _)| i).collect();
        let spans: Vec<(u64, u64)> = q.iter().map(|&i| (f.rows[i].asked_ms, f.now_ms + (self.ms_per_px * 4.0) as u64)).collect();
        let rows = pack(&spans);
        let fit = 4;
        let n_rows = rows.iter().copied().max().map_or(0, |m| m + 1).clamp(2, fit);
        let hidden = rows.iter().filter(|&&r| r >= fit).count();
        let (ink, muted) = (self.ink, self.muted);
        let x = self.view.pos.x + 8.0;
        let w = self.small(cx, x, y + 1.0, 80.0, "WAITING NOW", ink);
        let note = if q.is_empty() { "nothing".to_string() } else if hidden > 0 { format!("+{hidden} more in the queue list") } else { "request \u{2192} now".to_string() };
        self.small(cx, x + w + 6.0, y + 1.0, GUTTER - w - 22.0, &note, muted);
        let (plot_x, right) = (self.plot_x(), self.right());
        for (&i, &lr) in q.iter().zip(&rows) {
            let r = &f.rows[i];
            if lr >= fit || f.now_ms < t0 || r.asked_ms > t1 {
                continue;
            }
            let x0 = self.x_of(r.asked_ms).max(plot_x);
            let x1 = self.x_of(f.now_ms).min(right).max(x0 + 3.0);
            let ry = y + lr as f64 * GHOST_PITCH;
            let c = self.kind_color(&r.kind);
            let rect = Rect { pos: dvec2(x0, ry), size: dvec2(x1 - x0, GHOST_H) };
            let flag = self.flags.get(&r.id).map(|(s, _)| *s);
            let selected = self.selected.as_deref() == Some(r.id.as_str());
            match (selected, flag) {
                (true, _) => {
                    let a = self.accent;
                    self.draw_bar.glowing(cx, rect, Self::with_alpha(c, 0.3), a, Self::with_alpha(a, 0.55), 4.0, 2.0)
                }
                (false, Some(s)) => {
                    let sc = self.sev_color(s);
                    self.draw_bar.glowing(cx, rect, Self::with_alpha(c, 0.3), sc, Self::with_alpha(sc, 0.45), 4.0, 2.0)
                }
                (false, None) => {
                    let outline = if f.wait_ms(r) >= LONG_WAIT_MS { self.warn } else { Self::with_alpha(c, 0.75) };
                    self.draw_bar.plain(cx, rect, Self::with_alpha(c, 0.2), outline, 2.0)
                }
            }
            self.label(cx, rect, &r.lane, ink);
            self.picks.push((rect, Pick::Lease(i, true)));
        }
        y + n_rows as f64 * GHOST_PITCH + 2.0
    }

    /// A bar's label: inside it, clipped to it.
    fn label(&mut self, cx: &mut Cx2d, r: Rect, s: &str, color: Vec4f) {
        if r.size.x > 18.0 {
            Self::text_in(&mut self.draw_label, cx, r.pos.x + 3.0, r.pos.y + 0.5, r.size.x - 5.0, s, color);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_lane(&mut self, cx: &mut Cx2d, f: &Fleet, l: &Lane, y: f64, t0: u64, t1: u64, now_t: f64) {
        let view = self.view;
        let (plot_x, right) = (self.plot_x(), self.right());
        let n = &f.nodes[l.ni];
        // the lane's separator
        let grid = self.grid;
        self.rect(cx, view.pos.x, y + l.h, right, 1.0, grid);
        if l.mode == Mode::Down {
            self.draw_down(cx, f, l, y);
            return;
        }
        let g = self.col_grid();
        let samples = f.pressure.get(&n.name).cloned();
        let caps = n.caps.clone();
        // its CPU use as a soft area behind the bars (collapsed), or its four pressure rows
        if let (Some(s), Some(c)) = (samples.as_ref(), caps.as_ref()) {
            if l.mode == Mode::Collapsed {
                let vals = columns(s, g.t0, g.per, g.n, f.now_ms, &|x: &Sample| if x.cpu_pct < 0.0 { -1.0 } else { x.cpu_pct / 100.0 });
                let me = &*self;
                let heat: Vec<Vec4f> = (0..=20).map(|i| me.heat(i as f32 / 20.0)).collect();
                let color_of = |v: f32| Self::with_alpha(heat[(v.clamp(0.0, 1.0) * 20.0) as usize], 0.16);
                self.draw_area.series(cx, g.x0, plot_x, COL_W, y + 1.0, l.bars_h + 2.0, &vals, &color_of, 0.5);
            } else {
                let py = y + l.bars_h + 4.0;
                let muted = self.muted;
                let me = &*self;
                let heat: Vec<Vec4f> = (0..=20).map(|i| me.heat(i as f32 / 20.0)).collect();
                for (k, name) in PRESS_ROWS.iter().enumerate() {
                    let ry = py + k as f64 * (PRESS_ROW + 1.0);
                    self.rect(cx, plot_x, ry + PRESS_ROW, right, 1.0, grid);
                    self.small_right(cx, plot_x - 6.0, ry, name, muted);
                    let (cores, ram) = (c.cores, c.ram_gb);
                    let vals = columns(s, g.t0, g.per, g.n, f.now_ms, &|x: &Sample| shares(x, cores, ram)[k]);
                    let color_of = |v: f32| Self::with_alpha(heat[(v.clamp(0.0, 1.0) * 20.0) as usize], 0.6);
                    self.draw_area.series(cx, g.x0, plot_x, COL_W, ry, PRESS_ROW, &vals, &color_of, 0.95);
                }
            }
        }
        // the queue waits first, as thin tails leading into their runs; the runs cover them
        for &(i, row) in &l.placed {
            let r = &f.rows[i];
            let granted = r.granted_ms.unwrap_or(r.asked_ms);
            if granted <= r.asked_ms || granted < t0 {
                continue;
            }
            let x0 = self.x_of(r.asked_ms).max(plot_x);
            let x1 = self.x_of(granted).min(right);
            if x1 > x0 + 0.5 {
                let ry = y + 1.0 + row as f64 * ROW_H;
                let c = if granted - r.asked_ms >= LONG_WAIT_MS { self.warn } else { self.kind_color(&r.kind) };
                self.rect(cx, x0, ry + BAR_H * 0.5 - 1.0, x1, 2.0, Self::with_alpha(c, 0.4));
                self.picks.push((Rect { pos: dvec2(x0, ry + 1.0), size: dvec2(x1 - x0, BAR_H - 2.0) }, Pick::Lease(i, true)));
            }
        }
        for &(i, row) in &l.placed {
            let r = &f.rows[i];
            let ry = y + 1.0 + row as f64 * ROW_H;
            let granted = r.granted_ms.unwrap_or(r.asked_ms);
            let end = f.end_ms(r);
            if end < t0 {
                continue;
            }
            let x0 = self.x_of(granted).max(plot_x);
            let x1 = self.x_of(end).min(right).max(x0 + 2.0);
            let rect = Rect { pos: dvec2(x0, ry), size: dvec2(x1 - x0, BAR_H) };
            self.draw_lease_bar(cx, f, r, rect, now_t);
            self.picks.push((rect, Pick::Lease(i, false)));
        }
        // a lane's leases that do not fit: a chip at the gutter's right edge
        self.picks.push((Rect { pos: dvec2(plot_x, y), size: dvec2(right - plot_x, l.h) }, Pick::Press(l.ni)));
        if l.mode == Mode::Collapsed {
            self.draw_head_line(cx, f, l, y, t0, t1);
        } else {
            self.draw_head_full(cx, n, y);
        }
        self.picks.push((Rect { pos: dvec2(view.pos.x, y), size: dvec2(GUTTER, l.h) }, Pick::Head(l.ni)));
    }

    /// One lease's bar: its kind's colour (calmer once ended), hatched when it takes the box,
    /// a bright cap at its live end, its flag's or the selection's glow, its entrance.
    fn draw_lease_bar(&mut self, cx: &mut Cx2d, f: &Fleet, r: &Row, rect: Rect, now_t: f64) {
        let c = self.kind_color(&r.kind);
        let running = r.ended_ms.is_none();
        let exclusive = r.kind == "timed" || r.whole;
        let ran = f.end_ms(r).saturating_sub(r.granted_ms.unwrap_or(f.now_ms));
        let over = running && !r.live && r.measured_ms.is_some_and(|m| m >= 30_000 && ran > 2 * m);
        let fresh = self.fresh.get(&r.id).is_some_and(|s| now_t - s < FRESH_S);
        // opaque, so the load behind a lane never muddies a bar's hue; an ended one a deeper shade of it
        let fill = if running { Self::with_alpha(c, 1.0) } else { Self::with_alpha(mix(c, self.background, 0.4), 1.0) };
        let selected = self.selected.as_deref() == Some(r.id.as_str());
        let flag = self.flags.get(&r.id).map(|(s, _)| *s);
        let (outline, glow) = if selected {
            (self.accent, Some((Self::with_alpha(self.accent, 0.6), 5.0)))
        } else if let Some(s) = flag {
            let sc = self.sev_color(s);
            (sc, Some((Self::with_alpha(sc, 0.5), 4.5)))
        } else if over {
            (self.warn, None)
        } else if fresh {
            (mix(c, self.ink, 0.5), Some((Self::with_alpha(c, 0.55), 4.0)))
        } else if running {
            (Self::with_alpha(mix(c, self.ink, 0.3), 0.6), None)
        } else {
            (vec4(0.0, 0.0, 0.0, 0.0), None)
        };
        self.draw_bar.stripe = if exclusive { 1.0 } else { 0.0 };
        self.draw_bar.color = fill;
        self.draw_bar.outline = outline;
        self.draw_bar.radius = 2.0;
        match glow {
            Some((g, spread)) => {
                self.draw_bar.glow = g;
                self.draw_bar.spread = spread as f32;
                self.draw_bar.draw_abs(cx, Rect { pos: rect.pos - dvec2(spread, spread), size: rect.size + dvec2(spread * 2.0, spread * 2.0) });
            }
            None => {
                self.draw_bar.glow = vec4(0.0, 0.0, 0.0, 0.0);
                self.draw_bar.spread = 0.0;
                self.draw_bar.draw_abs(cx, rect);
            }
        }
        // the live end: a bright cap where it meets now
        if running && rect.size.x > 6.0 && self.x_of(f.now_ms) <= rect.pos.x + rect.size.x + 1.0 {
            let cap = mix(c, Self::with_alpha(self.ink, 1.0), 0.55);
            self.caps_later.push((Rect { pos: dvec2(rect.pos.x + rect.size.x - 2.0, rect.pos.y + 1.0), size: dvec2(1.5, BAR_H - 2.0) }, cap));
        }
        let label = match (exclusive, r.kind.as_str()) {
            (true, "timed") => format!("TIMED {}", r.lane),
            (true, _) => format!("WHOLE {}", r.lane),
            _ => r.lane.clone(),
        };
        let ink = if running { self.bar_ink } else { Self::with_alpha(self.ink, 0.75) };
        if rect.size.x > 18.0 {
            self.labels_later.push((rect, label, ink));
        }
    }

    /// A node that is down: one line.
    fn draw_down(&mut self, cx: &mut Cx2d, f: &Fleet, l: &Lane, y: f64) {
        let n = &f.nodes[l.ni];
        let (plot_x, right) = (self.plot_x(), self.right());
        let s = self.surface;
        self.draw_bar.color = Self::with_alpha(s, 1.0);
        self.draw_bar.outline = vec4(0.0, 0.0, 0.0, 0.0);
        self.draw_bar.glow = vec4(0.0, 0.0, 0.0, 0.0);
        self.draw_bar.spread = 0.0;
        self.draw_bar.stripe = 1.0;
        self.draw_bar.radius = 0.0;
        self.draw_bar.draw_abs(cx, Rect { pos: dvec2(plot_x, y + 2.0), size: dvec2(right - plot_x, l.h - 4.0) });
        let x = self.view.pos.x + 8.0;
        let bad = self.bad;
        self.draw_bar.glowing(cx, Rect { pos: dvec2(x, y + 5.0), size: dvec2(6.0, 6.0) }, bad, vec4(0.0, 0.0, 0.0, 0.0), Self::with_alpha(bad, 0.5), 2.5, 3.0);
        let muted = self.muted;
        let nw = self.bold(cx, x + 10.0, y + 2.5, 76.0, &n.name, muted);
        let leases = if l.hidden.is_empty() { String::new() } else { format!(" \u{b7} {} leases in view", l.hidden.len()) };
        self.small(cx, x + 16.0 + nw, y + 3.5, GUTTER - nw - 34.0, &format!("{}{leases}", n.state_word()), bad);
        self.picks.push((Rect { pos: dvec2(self.view.pos.x, y), size: dvec2(GUTTER, l.h) }, Pick::Head(l.ni)));
    }

    /// A collapsed node's gutter: one line (state, name, role and cores, CPU, RAM, GPU and
    /// disk meters) over its load's sparkline and what does not fit.
    #[allow(clippy::too_many_arguments)]
    fn draw_head_line(&mut self, cx: &mut Cx2d, f: &Fleet, l: &Lane, y: f64, t0: u64, t1: u64) {
        let n = &f.nodes[l.ni];
        let x = self.view.pos.x + 8.0;
        let (ink, muted) = (self.ink, self.muted);
        let dot = self.dot_color(n);
        if matches!(node_state(n).0, Dot::Ok | Dot::Off) {
            self.draw_bar.plain(cx, Rect { pos: dvec2(x, y + 4.0), size: dvec2(6.0, 6.0) }, dot, vec4(0.0, 0.0, 0.0, 0.0), 3.0);
        } else {
            self.draw_bar.glowing(cx, Rect { pos: dvec2(x, y + 4.0), size: dvec2(6.0, 6.0) }, dot, vec4(0.0, 0.0, 0.0, 0.0), Self::with_alpha(dot, 0.5), 2.5, 3.0);
        }
        self.bold(cx, x + 10.0, y + 1.5, 70.0, &n.name, ink);
        let Some(c) = n.caps.clone() else {
            self.small(cx, x + 84.0, y + 2.5, GUTTER - 100.0, &n.state_word(), muted);
            return;
        };
        let role = format!("{} {}c", c.role().as_str(), c.cores);
        self.small(cx, x + 84.0, y + 2.5, 44.0, &role, muted);
        // the meters: a letter and a short track, filled by heat
        let cpu = c.cpu_pct.map_or((c.load1 / c.cores.max(1) as f64) as f32, |p| (p / 100.0) as f32);
        let ram = ((c.ram_gb - c.ram_avail_gb) / c.ram_gb.max(1.0)) as f32;
        let gpu = c.gpu_busy.first().map(|(u, _)| (*u / 100.0) as f32).filter(|_| !c.gpus.is_empty());
        let disk = (c.disk_total_gb > 0.0).then(|| (1.0 - c.disk_free_gb / c.disk_total_gb) as f32);
        let mut mx = x + 132.0;
        for (letter, v) in [("C", Some(cpu)), ("R", Some(ram)), ("G", gpu), ("D", disk)] {
            self.small(cx, mx, y + 2.0, 8.0, letter, muted);
            let track = Rect { pos: dvec2(mx + 8.0, y + 5.5), size: dvec2(20.0, 4.0) };
            let g = self.grid;
            self.draw_bar.plain(cx, track, Self::with_alpha(mix(g, self.ink, 0.08), 1.0), vec4(0.0, 0.0, 0.0, 0.0), 2.0);
            if let Some(v) = v {
                let w = (20.0 * v.clamp(0.0, 1.0) as f64).max(2.0);
                let h = self.heat(v);
                self.draw_bar.plain(cx, Rect { pos: track.pos, size: dvec2(w, 4.0) }, h, vec4(0.0, 0.0, 0.0, 0.0), 2.0);
            }
            mx += 31.0;
        }
        // the sparkline: its CPU over the window, as tall as the lane leaves it
        let sy = y + 14.0;
        let sh = (l.h - 17.0).max(8.0);
        if let Some(s) = f.pressure.get(&n.name) {
            let n_cols = (SPARK_W / COL_W) as usize;
            let per = t1.min(f.now_ms).saturating_sub(t0) as f64 / n_cols.max(1) as f64;
            let vals = columns(s, t0 as f64, per, n_cols, f.now_ms, &|x: &Sample| if x.cpu_pct < 0.0 { -1.0 } else { x.cpu_pct / 100.0 });
            let g = self.grid;
            self.rect(cx, x, sy + sh, x + SPARK_W, 1.0, g);
            let me = &*self;
            let heat: Vec<Vec4f> = (0..=20).map(|i| me.heat(i as f32 / 20.0)).collect();
            let color_of = |v: f32| Self::with_alpha(heat[(v.clamp(0.0, 1.0) * 20.0) as usize], 0.6);
            self.draw_area.series(cx, x, x, COL_W, sy, sh, &vals, &color_of, 0.95);
        }
        let chip_x = x + SPARK_W + 8.0;
        let running = l.placed.iter().filter(|(i, _)| f.rows[*i].ended_ms.is_none()).count() + l.hidden_running;
        let on_w = self.small(cx, chip_x, sy, 30.0, &format!("{running} on"), muted);
        if !l.hidden.is_empty() {
            // what the rows do not show: amber when some of it runs
            let chip_x = chip_x + on_w + 5.0;
            let c = if l.hidden_running > 0 { self.warn } else { muted };
            let s = if l.hidden_running > 0 { format!("+{} \u{25be}", l.hidden_running) } else { format!("+{} \u{25be}", l.hidden.len()) };
            let w = Self::width_of(&self.draw_small, cx, &s) + 8.0;
            let r = Rect { pos: dvec2(chip_x, sy - 0.5), size: dvec2(w, 11.0) };
            self.draw_bar.plain(cx, r, Self::with_alpha(c, 0.14), Self::with_alpha(c, 0.6), 3.0);
            self.small(cx, chip_x + 4.0, sy, w, &s, c);
            self.picks.push((r, Pick::Hidden(l.ni)));
        }
    }

    fn dot_color(&self, n: &NodeView) -> Vec4f {
        match node_state(n).0 {
            Dot::Ok => self.good,
            Dot::Warn => self.warn,
            Dot::Off => self.muted,
            Dot::Down => self.bad,
        }
    }

    /// An expanded node's gutter: name and state, the box, its load, its versions.
    fn draw_head_full(&mut self, cx: &mut Cx2d, n: &NodeView, y: f64) {
        let x = self.view.pos.x + 8.0;
        let w = GUTTER - 16.0;
        let (ink, muted) = (self.ink, self.muted);
        let dot = self.dot_color(n);
        self.draw_bar.plain(cx, Rect { pos: dvec2(x, y + 4.0), size: dvec2(6.0, 6.0) }, dot, vec4(0.0, 0.0, 0.0, 0.0), 3.0);
        let nw = self.bold(cx, x + 10.0, y + 1.5, w * 0.5, &n.name, ink);
        let role = n.caps.as_ref().map_or("pool", |c| c.role().as_str());
        self.small(cx, x + 16.0 + nw, y + 2.5, w - nw - 30.0, &format!("{role} \u{b7} {} leases \u{b7} click to fold", n.leases), muted);
        let (_, state) = node_state(n);
        let Some(c) = n.caps.as_ref() else {
            self.small(cx, x, y + LINE_H * 1.3, w, &state, self.bad);
            return;
        };
        let cpu = c.cpu_pct.map_or(String::new(), |p| format!(" \u{b7} cpu {p:.0}%"));
        self.small(cx, x, y + LINE_H * 1.3, w, &format!("{} {} \u{b7} {} cores{cpu} \u{b7} load {:.1}", c.arch, c.os, c.cores, c.load1), muted);
        let mut l3 = format!("RAM {:.0}/{:.0} GB", (c.ram_gb - c.ram_avail_gb).max(0.0), c.ram_gb);
        for (i, (_, vram)) in c.gpus.iter().enumerate() {
            match c.gpu_busy.get(i) {
                Some((u, m)) => l3.push_str(&format!(" \u{b7} GPU {u:.0}% {m:.0}/{vram:.0} GB")),
                None => l3.push_str(&format!(" \u{b7} GPU {vram:.0} GB")),
            }
        }
        self.small(cx, x, y + LINE_H * 2.3, w, &l3, muted);
        let ver = |s: &str| if s.is_empty() { "-".to_string() } else { short(s, 8).to_string() };
        self.small(cx, x, y + LINE_H * 3.3, w, &format!("disk {:.0} GB free \u{b7} hub {} \u{b7} deploy {}", c.disk_free_gb, ver(&c.hub_rev), ver(c.deploy.split(',').next().unwrap_or(""))), muted);
        let sc = if node_state(n).0 == Dot::Warn { self.warn } else { muted };
        self.small(cx, x, y + LINE_H * 4.3, w, &state, sc);
    }

    fn pick_at(&self, p: DVec2) -> Option<Pick> {
        // a lease bar wins over the ghost under it, both over the lanes and headers
        let mut best: Option<Pick> = None;
        for (r, pick) in self.picks.iter().rev() {
            if !r.contains(p) {
                continue;
            }
            match pick {
                Pick::Lease(_, false) | Pick::Landing(_) | Pick::Hidden(_) => return Some(*pick),
                Pick::Lease(_, true) | Pick::Bucket(_) => best = Some(*pick),
                Pick::Press(_) | Pick::Head(_) | Pick::Queue => {
                    if best.is_none() {
                        best = Some(*pick);
                    }
                }
            }
        }
        best
    }

    /// What the pointer is over, and over an area its column (the tooltip's content changes
    /// with either).
    fn hover_key(&self, p: DVec2) -> Option<(Pick, i64)> {
        if !self.view.contains(p) || self.pan.is_some_and(|p| p.moved) {
            return None;
        }
        let pick = self.pick_at(p)?;
        let col = match pick {
            Pick::Press(_) | Pick::Queue => ((p.x - self.plot_x()) / (COL_W * 2.0)) as i64,
            _ => 0,
        };
        Some((pick, col))
    }

    fn lease_tip(&self, f: &Fleet, r: &Row, offset: i64) -> Vec<String> {
        let mut head = format!("{} \u{b7} {} \u{b7} {}", r.id, r.kind, r.lane);
        if let Some((_, tag)) = self.flags.get(&r.id) {
            head.push_str(&format!(" \u{b7} {tag}"));
        }
        let mut v = vec![head];
        let mut who = Vec::new();
        if !r.agent.is_empty() {
            who.push(format!("agent {}", r.agent));
        }
        if !r.node.is_empty() {
            who.push(format!("on {}", r.node));
        }
        who.push(format!("{} \u{b7} {}", r.priority, r.src));
        v.push(who.join(" \u{b7} "));
        if !r.running.is_empty() {
            v.push(format!("running `{}`", r.running));
        } else if !r.class.is_empty() {
            v.push(format!("class {}", r.class));
        }
        let mut cost = format!("cost {} ({}) \u{b7} {:.0} cores \u{b7} {:.0} GB", r.cost, r.cost_source, r.cores, r.ram_gb);
        if r.gpus > 0 {
            cost.push_str(&format!(" \u{b7} {} GPU", r.gpus));
        }
        v.push(cost);
        let asked = clock(r.asked_ms, offset, true);
        match r.granted_ms {
            Some(g) => {
                let ran = f.end_ms(r).saturating_sub(g);
                let measured = r.measured_ms.map_or("not measured".to_string(), |m| format!("measured {}", dur(m)));
                let over = r.measured_ms.is_some_and(|m| m > 0 && ran > m * 2 && r.ended_ms.is_none());
                v.push(format!("{} {} of {measured}{}", if r.ended_ms.is_some() { "ran" } else { "running" }, dur(ran), if over { " \u{2014} over twice its measure" } else { "" }));
                v.push(format!("asked {asked}, granted {} \u{b7} waited {}", clock(g, offset, true), dur(g.saturating_sub(r.asked_ms))));
            }
            None => {
                v.push(format!("asked {asked} \u{b7} waiting {}{}", dur(f.wait_ms(r)), if r.eta_ms > 0 { format!(" \u{b7} expected in {}", dur(r.eta_ms)) } else { String::new() }));
            }
        }
        if let Some(b) = r.beat_ms {
            v.push(format!("last beat {} ago", dur(f.now_ms.saturating_sub(b))));
        }
        if r.state == "queued" && !r.reason.is_empty() {
            v.push(format!("now: {}", r.reason));
        }
        if let Some(e) = r.ended_ms {
            v.push(format!("ended {} ({})", clock(e, offset, true), if r.how.is_empty() { "ended" } else { &r.how }));
        }
        v.push("click: its detail".into());
        v
    }

    fn press_tip(&self, f: &Fleet, n: &NodeView, t: u64, offset: i64) -> Vec<String> {
        let Some(s) = f.pressure.get(&n.name).and_then(|v| {
            let i = v.partition_point(|s| s.t_ms <= t);
            i.checked_sub(1).and_then(|i| v.get(i))
        }) else {
            return vec![format!("{} \u{b7} no samples here", n.name)];
        };
        let c = n.caps.as_ref();
        let cores = c.map_or(0, |c| c.cores);
        let ram = c.map_or(0.0, |c| c.ram_gb);
        let mut v = vec![format!("{} at {}", n.name, clock(s.t_ms, offset, true))];
        v.push(format!("cpu {} \u{b7} load {:.1} \u{b7} RAM {:.0}/{ram:.0} GB used", if s.cpu_pct < 0.0 { "-".to_string() } else { format!("{:.0}%", s.cpu_pct) }, s.load1, s.ram_used_gb));
        v.push(format!("reserved {:.0}/{cores} cores \u{b7} {:.0}/{:.0} GB RAM budget", s.res_cores, s.res_ram_gb, ram * 0.85));
        if s.gpu_pct >= 0.0 {
            v.push(format!("GPU {:.0}% \u{b7} {:.1} GB VRAM used", s.gpu_pct, s.gpu_used_gb));
        }
        v
    }

    fn queue_tip(&self, f: &Fleet, t: u64, offset: i64) -> Vec<String> {
        let mut by: Vec<(String, usize)> = Vec::new();
        for r in &f.rows {
            for w in &r.waits {
                let end = if w.end_ms == 0 { f.now_ms } else { w.end_ms };
                if w.start_ms <= t && end > t {
                    let k = blocker_key(r, &w.blocker);
                    match by.iter_mut().find(|(b, _)| *b == k) {
                        Some(e) => e.1 += 1,
                        None => by.push((k, 1)),
                    }
                }
            }
        }
        by.sort_by(|a, b| b.1.cmp(&a.1));
        let total: usize = by.iter().map(|(_, n)| n).sum();
        let mut v = vec![format!("{total} waiting at {}", clock(t, offset, true))];
        v.extend(by.iter().take(8).map(|(b, n)| format!("{n} \u{b7} {b}")));
        v
    }

    /// The hovered thing's tooltip, kept inside the view: its top left, width and lines.
    fn hover_tip(&self, cx: &mut Cx2d, f: &Fleet, offset: i64) -> Option<(DVec2, f64, Vec<(String, Vec4f)>)> {
        let (pick, _) = self.hover?;
        let lines: Vec<String> = match pick {
            Pick::Lease(i, _) => f.rows.get(i).map(|r| self.lease_tip(f, r, offset)).unwrap_or_default(),
            Pick::Press(ni) => match f.nodes.get(ni) {
                Some(n) => {
                    let t = self.t_of(self.hover_at.x).max(0.0) as u64;
                    self.press_tip(f, n, t, offset)
                }
                None => Vec::new(),
            },
            Pick::Queue => {
                let t = self.t_of(self.hover_at.x).max(0.0) as u64;
                self.queue_tip(f, t.min(f.now_ms), offset)
            }
            Pick::Landing(i) => f
                .land
                .get(i)
                .map(|e| {
                    let mut v = vec![format!("#{} {} \u{b7} {}", e.id, e.label(), e.state.as_str())];
                    if !e.agent.is_empty() {
                        v.push(format!("agent {}", e.agent));
                    }
                    if !e.landed.is_empty() {
                        let shas: Vec<&str> = e.landed.split_whitespace().collect();
                        if let [_, w, _, m] = shas.as_slice() {
                            v.push(format!("work {} \u{b7} main {}", short(w, 10), short(m, 10)));
                        }
                    }
                    v.push(format!("handed in {} \u{b7} {} {}", clock(e.asked_ms, offset, true), e.state.as_str(), clock(e.changed_ms, offset, true)));
                    v.extend(e.detail.iter().take(4).cloned());
                    v
                })
                .unwrap_or_default(),
            Pick::Bucket(i) => f
                .buckets
                .get(i)
                .map(|b| {
                    let mut v = vec![format!("bucket {} \u{b7} {} \u{b7} {}", b.name, if b.ok { "ok" } else { "FAILED" }, b.tip)];
                    v.push(format!("{} \u{2192} {} ({})", clock(b.started_ms, offset, true), clock(b.ended_ms, offset, true), dur(b.ended_ms.saturating_sub(b.started_ms))));
                    if !b.failing.is_empty() {
                        v.push(format!("failing: {}", b.failing.join(", ")));
                    }
                    v
                })
                .unwrap_or_default(),
            Pick::Head(ni) => f
                .nodes
                .get(ni)
                .map(|n| {
                    let mut v = vec![format!("{} \u{b7} {}", n.name, n.state_word())];
                    if let Some(c) = &n.caps {
                        if !c.cpu.is_empty() {
                            v.push(format!("{} \u{b7} {} cores \u{b7} {:.0} GB RAM", c.cpu, c.cores, c.ram_gb));
                        }
                        let cpu = c.cpu_pct.map_or("-".to_string(), |p| format!("{p:.0}%"));
                        v.push(format!("cpu {cpu} \u{b7} load {:.1} \u{b7} RAM {:.0}/{:.0} GB used", c.load1, (c.ram_gb - c.ram_avail_gb).max(0.0), c.ram_gb));
                        for (i, (g, vram)) in c.gpus.iter().enumerate() {
                            let busy = c.gpu_busy.get(i).map_or(String::new(), |(u, m)| format!(" \u{b7} {u:.0}% \u{b7} {m:.0} GB used"));
                            v.push(format!("GPU {g} \u{b7} {vram:.0} GB{busy}"));
                        }
                        v.push(format!("disk {:.0} of {:.0} GB free (floor {:.0})", c.disk_free_gb, c.disk_total_gb, c.disk_floor_gb));
                        if !c.rustc.is_empty() {
                            v.push(format!("rustc {} \u{b7} {} hub features", c.rustc, c.features.len()));
                        }
                        if !c.sick.is_empty() {
                            v.push(format!("cannot link: {}", c.sick));
                        }
                    }
                    if !n.latency.is_empty() {
                        v.push(n.latency.clone());
                    }
                    v.push(if self.expanded.contains(&n.name) { "click: fold".into() } else { "click: every lease and its pressure".into() });
                    v
                })
                .unwrap_or_default(),
            Pick::Hidden(ni) => {
                let name = f.nodes.get(ni).map_or("", |n| n.name.as_str());
                let mut v: Vec<String> = vec![format!("{name}: leases its two rows do not show")];
                let mut hidden: Vec<&Row> = f.rows.iter().map(|r| &**r).filter(|r| r.node == name && r.granted_ms.is_some()).collect();
                hidden.sort_by_key(|r| (r.ended_ms.is_some(), std::cmp::Reverse(r.granted_ms)));
                v.extend(hidden.iter().take(10).map(|r| format!("{} {} {} \u{b7} {}", r.id, r.kind, r.lane, if r.ended_ms.is_none() { "running".to_string() } else { format!("ended {}", clock(r.ended_ms.unwrap_or(0), offset, false)) })));
                v.push("click: every lease".into());
                v
            }
        };
        if lines.is_empty() {
            return None;
        }
        let at = self.hover_at;
        let pad = 7.0;
        let lh = 12.5;
        let mut w: f64 = 0.0;
        for l in &lines {
            w = w.max(Self::width_of(&self.draw_tip_text, cx, l));
        }
        let w = (w + pad * 2.0).min(620.0);
        let h = lines.len() as f64 * lh + pad * 2.0 - 2.0;
        let view = self.view;
        let mut x = at.x + 14.0;
        if x + w > view.pos.x + view.size.x - 4.0 {
            x = (at.x - w - 10.0).max(view.pos.x + 4.0);
        }
        let mut y = at.y + 16.0;
        if y + h > view.pos.y + view.size.y - 4.0 {
            y = (at.y - h - 8.0).max(view.pos.y + 4.0);
        }
        let fg = self.tip_ink;
        Some((dvec2(x, y), w, lines.into_iter().enumerate().map(|(i, l)| (l, if i == 0 { fg } else { Self::with_alpha(fg, 0.82) })).collect()))
    }

    fn zoom(&mut self, factor: f64, x: f64) {
        let t = self.t_of(x);
        self.ms_per_px = (self.ms_per_px * factor).clamp(200.0, KEEP_MS as f64 / self.plot_w() * 1.05);
        self.t_right = t + (self.plot_x() + self.plot_w() - x) * self.ms_per_px;
    }

    fn unfollow(&mut self, cx: &mut Cx) {
        if self.follow {
            self.follow = false;
            cx.widget_action(self.uid, FleetTimelineAction::Unfollowed);
        }
    }

    fn clamp_scroll(&mut self) {
        let room = (self.view.pos.y + self.view.size.y - self.lanes_top).max(0.0);
        self.scroll = self.scroll.clamp(0.0, (self.content_h - room).max(0.0));
    }

    fn toggle(&mut self, cx: &mut Cx, node: &str) {
        if !self.expanded.remove(node) {
            self.expanded.insert(node.to_string());
        }
        self.area.redraw(cx);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dot {
    Ok,
    Warn,
    Off,
    Down,
}

/// The node's state dot and word.
fn node_state(n: &NodeView) -> (Dot, String) {
    let word = n.state_word();
    let dot = if !n.up {
        Dot::Down
    } else if n.updating.is_some() || n.draining || n.user().is_some() || n.caps.as_ref().is_some_and(|c| !c.sick.is_empty()) {
        Dot::Warn
    } else if !n.dev {
        Dot::Off
    } else {
        Dot::Ok
    };
    (dot, word)
}

/// A ruler step in minutes: 1, 2, 5, 10, 15, 30, 60, 120 ... at least `raw`.
fn nice_minutes(raw: f64) -> f64 {
    for m in [0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 180.0] {
        if m >= raw {
            return m;
        }
    }
    360.0
}

pub fn mix(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    vec4(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t, a.w + (b.w - a.w) * t)
}

/// How far layout moved a widget after it drew. A widget after a Fill sibling draws before the
/// fill resolves and is moved once it does, so the rects it recorded while drawing are off by
/// this much: a pointer minus it lands in them (and a tooltip drawn there lands at the pointer).
pub fn moved_by(cx: &Cx, area: Area, drawn: Rect) -> DVec2 {
    let placed = area.rect(cx);
    if placed.size.x > 0.0 { placed.pos - drawn.pos } else { DVec2::default() }
}

impl WidgetNode for FleetTimeline {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
    fn children(&self, _visit: &mut dyn FnMut(LiveId, WidgetRef)) {}
    fn find_widgets_from_point(&self, _cx: &Cx, _p: DVec2, _found: &mut dyn FnMut(&WidgetRef)) {}
}

impl Widget for FleetTimeline {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let view = cx.walk_turtle(walk);
        cx.add_rect_area(&mut self.area, view);
        self.view = view;
        cx.push_clip_rect(view);
        self.draw_shape.color = self.background;
        self.draw_shape.draw_abs(cx, view);
        match self.fleet.clone() {
            Some(f) if f.now_ms > 0 => self.draw_fleet(cx, &f),
            _ => {
                let muted = self.muted;
                self.text(cx, view.pos.x + 12.0, view.pos.y + 12.0, view.size.x - 24.0, "Waiting for the master's first answer\u{2026}", muted);
            }
        }
        cx.pop_clip_rect();
        let tip = match self.fleet.clone() {
            Some(f) if f.now_ms > 0 => self.hover_tip(cx, &f, local_offset_s(f.now_ms)),
            _ => None,
        };
        let float = tip.as_ref().map(|(at, w, lines)| Float { at: *at, w: *w, lines, line_h: 12.5, pad: 7.0, bg: self.tip_bg, shadow: 0.3 });
        draw_float(cx, &mut self.float, self.area, self.view, float, &mut self.draw_tip, &mut self.draw_tip_text);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let shift = moved_by(cx, self.area, self.view);
        if self.fresh_end.is_event(event).is_some() {
            self.fresh_end = Timer::empty();
            let t = self.clock();
            self.end_fresh(cx, t);
            self.area.redraw(cx);
        }
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let key = self.hover_key(e.abs - shift);
                if key != self.hover {
                    self.hover = key;
                    self.hover_at = e.abs - shift;
                    self.area.redraw(cx);
                }
                if matches!(key, Some((Pick::Lease(..) | Pick::Head(_) | Pick::Hidden(_), _))) {
                    cx.set_cursor(MouseCursor::Hand);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.area.redraw(cx);
                }
            }
            Hit::FingerScroll(e) => {
                if (e.abs - shift).x < self.plot_x() {
                    self.scroll += e.scroll.y;
                    self.clamp_scroll();
                } else if e.scroll.x.abs() > e.scroll.y.abs() {
                    self.t_right += e.scroll.x * self.ms_per_px;
                    self.unfollow(cx);
                } else {
                    self.zoom((e.scroll.y * 0.004).exp(), (e.abs - shift).x);
                    self.unfollow(cx);
                }
                self.hover = None;
                self.area.redraw(cx);
            }
            Hit::FingerDown(e) if e.device.is_primary_hit() => {
                self.pan = Some(Pan { start_x: e.abs.x, start_right: self.t_right, start_scroll: self.scroll, start_y: e.abs.y, moved: false });
            }
            Hit::FingerMove(e) => {
                if let Some(p) = self.pan {
                    let (dx, dy) = (e.abs.x - p.start_x, e.abs.y - p.start_y);
                    if p.moved || dx.abs() > 3.0 || dy.abs() > 3.0 {
                        self.pan = Some(Pan { moved: true, ..p });
                        self.t_right = p.start_right - dx * self.ms_per_px;
                        self.scroll = p.start_scroll - dy;
                        self.clamp_scroll();
                        if dx.abs() > 3.0 {
                            self.unfollow(cx);
                        }
                        self.hover = None;
                        cx.set_cursor(MouseCursor::Grabbing);
                        self.area.redraw(cx);
                    }
                }
            }
            Hit::FingerUp(e) => {
                let moved = self.pan.take().is_some_and(|p| p.moved);
                if !moved && e.is_over {
                    let fleet = self.fleet.clone();
                    match (self.pick_at(e.abs - shift), fleet) {
                        (Some(Pick::Lease(i, _)), Some(f)) => {
                            if let Some(r) = f.rows.get(i) {
                                self.selected = Some(r.id.clone());
                                cx.widget_action(self.uid, FleetTimelineAction::Selected(Some(r.id.clone())));
                            }
                        }
                        (Some(Pick::Head(ni) | Pick::Hidden(ni)), Some(f)) => {
                            if let Some(n) = f.nodes.get(ni).filter(|n| n.up) {
                                let name = n.name.clone();
                                self.toggle(cx, &name);
                            }
                        }
                        (Some(Pick::Press(_)) | None, _) => {
                            if self.selected.take().is_some() {
                                cx.widget_action(self.uid, FleetTimelineAction::Selected(None));
                            }
                        }
                        _ => {}
                    }
                }
                self.area.redraw(cx);
            }
            _ => {}
        }
    }
}

/// The shares a sample's pressure rows show: reserved cores of the node's cores, CPU, RAM used
/// of its RAM, the busiest GPU (negative: none).
fn shares(s: &Sample, cores: u32, ram_gb: f64) -> [f32; 4] {
    [s.res_cores / (cores as f32).max(1.0), s.cpu_pct / 100.0, s.ram_used_gb / (ram_gb as f32).max(1.0), s.gpu_pct / 100.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pan by a fraction of a pixel or by whole pixels keeps every column's span and value: the
    /// grid only slides (its first column's x), and only a zoom re-bins (11-08: the areas wobbled
    /// as the view panned).
    #[test]
    fn a_pan_slides_the_columns_and_rebins_nothing() {
        let ms_per_px = 1_250.0;
        let (plot_x, right) = (200.0, 1_000.0);
        let now = 10_000_000u64;
        // one sample every 7 s, its CPU stepping through ten levels
        let samples: Vec<Sample> = (0..1_400u64).map(|i| Sample { t_ms: i * 7_000, cpu_pct: (i % 10) as f32 * 10.0, ..Sample::default() }).collect();
        let cpu = |s: &Sample| s.cpu_pct / 100.0;
        // each column by its start in ms, and its value
        let binned = |t_left: f64| -> (Grid, Vec<(i64, f32)>) {
            let g = Grid::of(t_left, ms_per_px, plot_x, right);
            assert!(g.x0 <= plot_x && g.x0 > plot_x - COL_W, "the first column starts at the plot's left edge or within one column left of it: {g:?}");
            let vals = columns(&samples, g.t0, g.per, g.n, now, &cpu);
            (g, vals.into_iter().enumerate().map(|(i, v)| ((g.t0 / g.per).round() as i64 + i as i64, v)).collect())
        };
        let t_left = 3_000_000.0;
        let (g, base) = binned(t_left);
        for px in [0.25, 0.5, 0.999, 1.0, 2.0, 3.0, 7.5, 40.0] {
            let (h, moved) = binned(t_left + px * ms_per_px);
            assert_eq!(h.per, g.per, "a pan keeps the zoom's column width");
            // the same time sits at the same x relative to the bars: the grid moved by the pan
            let shift = (h.t0 - g.t0) / ms_per_px - (h.x0 - g.x0);
            assert!((shift - px).abs() < 1e-6, "{px} px: the columns slid {shift} px");
            let common: Vec<_> = moved.iter().filter(|(k, _)| base.iter().any(|(b, _)| b == k)).collect();
            assert!(common.len() + 20 >= base.len(), "{px} px: most columns are the same ones");
            for (k, v) in common {
                let b = base.iter().find(|(b, _)| b == k).unwrap().1;
                assert_eq!(*v, b, "{px} px: column {k} kept its value");
            }
        }
        // a zoom is a new grid
        let z = Grid::of(t_left, ms_per_px * 1.5, plot_x, right);
        assert!(z.per > g.per);
    }

    /// The part of a column left of the plot is cut, its value where the cut falls.
    #[test]
    fn a_column_left_of_the_plot_is_cut_at_its_edge() {
        assert_eq!(trim(10.0, 13.0, 0.0, 0.6, 11.5), (11.5, 0.3));
        assert_eq!(trim(12.0, 13.0, 0.2, 0.6, 11.5), (12.0, 0.2), "right of the edge: as it is");
    }
}
