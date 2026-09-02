//! The two custom widgets: the btop-style meter bars and the process table.
//!
//! Registered into `mod.widgets` by this module's own `script_mod!`, which
//! `App::script_mod` runs *before* the UI script_mod — a `use mod.widgets.*`
//! glob only imports what exists at the moment it runs, so a widget defined
//! later in the same block would not be in scope for the UI that uses it.

use crate::backend::{terminate, ProcInfo};
use crate::{format_bytes, Theme};
use makepad_widgets::*;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MeterBarsBase = #(MeterBars::register_widget(vm))
    mod.widgets.MeterBars = set_type_default() do mod.widgets.MeterBarsBase{
        width: Fill
        height: Fill
        columns: 1
        gradient: true
        label_width: 46.0
        value_width: 58.0
        bar_color: #x7aa2f7
        warn_color: #xe0af68
        crit_color: #xf7768e
        track_color: #x24283b
        text_color: #xa9b1d6
        muted_color: #x565f89
        draw_text +: {
            text_style: theme.font_code{font_size: 8.0}
            color: #xa9b1d6
        }
    }

    mod.widgets.AggregateGraphBase = #(AggregateGraph::register_widget(vm))
    mod.widgets.AggregateGraph = set_type_default() do mod.widgets.AggregateGraphBase{
        width: Fill
        height: Fill
        color_bg: #x16161e
        color_grid: #x41486859
        color_text: #x565f89
        line_width: 1.5
        draw_text +: {
            text_style: theme.font_code{font_size: 8.0}
            color: #x565f89
        }
    }

    mod.widgets.ProcessTableBase = #(ProcessTable::register_widget(vm))
    mod.widgets.ProcessTable = set_type_default() do mod.widgets.ProcessTableBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: 4
        accent_color: #x7aa2f7
        warning_color: #xf7768e
        muted_color: #x565f89

        toolbar := View{
            width: Fill
            height: 28
            flow: Right
            spacing: 8
            align: Align{y: 0.5}

            tree_toggle := Button{
                width: 74
                height: 24
                text: "TREE"
                padding: Inset{left: 8 right: 8 top: 2 bottom: 2}
                draw_bg +: {
                    border_radius: 0.0
                    border_size: 1.0
                }
                draw_text +: {
                    text_style: theme.font_code{font_size: 8.5}
                }
            }
            kill_button := Button{
                width: 128
                height: 24
                text: "KILL"
                padding: Inset{left: 8 right: 8 top: 2 bottom: 2}
                draw_bg +: {
                    border_radius: 0.0
                    border_size: 1.0
                }
                draw_text +: {
                    text_style: theme.font_code{font_size: 8.5}
                }
            }
            filter_label := Label{
                text: "FILTER"
                draw_text +: {color: #x7aa2f7 text_style: theme.font_code{font_size: 9.0}}
            }
            // The well behind the text is makepad_wm_theme's business (it flattens the
            // stock inset gradient app-wide); only the monospace face is ours.
            filter_input := TextInput{
                width: 260
                height: 24
                empty_text: "program name"
                padding: Inset{left: 8 right: 8 top: 3 bottom: 3}
                draw_text +: {
                    text_style: theme.font_code{font_size: 9.0}
                }
            }
            process_status := Label{
                width: Fill
                text: "waiting for the first sample"
                draw_text +: {color: #x565f89 text_style: theme.font_code{font_size: 8.0}}
            }
        }

        confirm_row := RectView{
            width: Fill
            height: 20
            flow: Right
            padding: Inset{left: 8 right: 8 top: 2 bottom: 2}
            align: Align{y: 0.5}
            draw_bg +: {
                color: #x16161e
                border_color: #x24283b
                border_size: 1.0
            }
            confirm_label := Label{
                width: Fill
                text: ""
                draw_text +: {color: #x565f89 text_style: theme.font_code{font_size: 8.0}}
            }
        }

        process_grid := DataGrid{
            width: Fill
            height: Fill
            rows: 0
            cols: 8
            show_row_headers: false
            zebra_stripes: true
            allow_col_resize: true
            allow_row_resize: false
            allow_col_reorder: false
            default_col_width: 110.0
            default_row_height: 21.0
            col_header_height: 24.0
            color_bg: #x1a1b26
            color_cell: #x1a1b26
            color_cell_alt: #x1e2030
            color_text: #xa9b1d6
            color_header: #x16161e
            color_header_active: #x24283b
            color_header_text: #x7aa2f7
            color_selection: #x7aa2f729
            color_selection_border: #x7aa2f7
            color_drag_marker: #x7aa2f7
            color_resize_guide: #x7aa2f766
            draw_cell +: {border_color: #x292e42 border_size: 1.0}
            draw_text +: {text_style: theme.font_code{font_size: 8.5}}
            draw_text_bold +: {text_style: theme.font_code{font_size: 8.5}}
        }
    }
}

// ---- MeterBars ----

/// One horizontal bar: `LABEL [#####      ]  42.0%`.
#[derive(Clone, Debug)]
pub struct MeterRow {
    pub label: String,
    pub value: String,
    /// 0..1 fill.
    pub fraction: f64,
}

/// A column-packed stack of labelled bars — the per-core CPU meters and the
/// memory breakdown. Everything is drawn absolutely so it composes under any
/// parent, and the fill switches to the warn/crit colours as it saturates.
#[derive(Script, ScriptHook, Widget)]
pub struct MeterBars {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_track: DrawColor,
    #[live]
    draw_fill: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live(1usize)]
    columns: usize,
    /// Load meters recolour as they saturate; a breakdown (memory) must not —
    /// there "100% of total" is not a warning.
    #[live(true)]
    gradient: bool,
    #[live(46.0)]
    label_width: f64,
    #[live(58.0)]
    value_width: f64,
    #[live]
    bar_color: Vec4f,
    #[live]
    warn_color: Vec4f,
    #[live]
    crit_color: Vec4f,
    #[live]
    track_color: Vec4f,
    #[live]
    text_color: Vec4f,
    #[live]
    muted_color: Vec4f,
    #[rust]
    rows: Vec<MeterRow>,
}

impl MeterBars {
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<MeterRow>, columns: usize) {
        self.rows = rows;
        self.columns = columns.max(1);
        self.draw_track.redraw(cx);
    }

    fn fill_color(&self, fraction: f64) -> Vec4f {
        if !self.gradient {
            self.bar_color
        } else if fraction >= 0.85 {
            self.crit_color
        } else if fraction >= 0.6 {
            self.warn_color
        } else {
            self.bar_color
        }
    }
}

impl Widget for MeterBars {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        if self.rows.is_empty() || rect.size.x < 40.0 || rect.size.y < 8.0 {
            return DrawStep::done();
        }
        let columns = self.columns.min(self.rows.len()).max(1);
        let per_column = self.rows.len().div_ceil(columns);
        let gap = 6.0;
        let column_width = (rect.size.x - gap * (columns.saturating_sub(1)) as f64) / columns as f64;
        let row_height = (rect.size.y / per_column as f64).min(22.0);
        let bar_height = (row_height - 4.0).clamp(4.0, 16.0);
        // The label and value gutters shrink first on a narrow panel.
        let label_width = self.label_width.min(column_width * 0.28);
        let value_width = self.value_width.min(column_width * 0.34);
        let track_width = (column_width - label_width - value_width).max(8.0);
        let text_drop = (bar_height - 10.0) * 0.5;

        for (index, row) in self.rows.iter().enumerate() {
            let column = index / per_column;
            let line = index % per_column;
            let origin = dvec2(
                rect.pos.x + column as f64 * (column_width + gap),
                rect.pos.y + line as f64 * row_height + 2.0,
            );
            let fraction = row.fraction.clamp(0.0, 1.0);

            self.draw_text.color = self.muted_color;
            self.draw_text.draw_abs(cx, origin + dvec2(0.0, text_drop), &row.label);

            let track = Rect {
                pos: dvec2(origin.x + label_width, origin.y),
                size: dvec2(track_width, bar_height),
            };
            self.draw_track.color = self.track_color;
            self.draw_track.draw_abs(cx, track);
            if fraction > 0.0 {
                self.draw_fill.color = self.fill_color(fraction);
                self.draw_fill.draw_abs(
                    cx,
                    Rect { pos: track.pos, size: dvec2((track.size.x * fraction).max(1.0), track.size.y) },
                );
            }

            self.draw_text.color = self.text_color;
            self.draw_text.draw_abs(
                cx,
                dvec2(track.pos.x + track.size.x + 6.0, origin.y + text_drop),
                &row.value,
            );
        }
        DrawStep::done()
    }
}

// ---- AggregateGraph ----

/// One overlaid line in the aggregate graph.
#[derive(Clone, Debug)]
pub struct GraphSeries {
    /// Legend name, e.g. "CPU".
    pub label: String,
    /// Current reading with its own unit, e.g. "41.1%" or "45.8 KiB/s".
    pub value: String,
    pub color: Vec4f,
    /// History mapped onto the shared 0..100 axis, oldest first.
    pub points: Vec<f64>,
}

/// The one graph that is always on screen: CPU, memory and network overlaid on
/// a single 0..100 axis with a legend inside the plot. When the window shrinks
/// to a WM tile this is the only thing left, so it has to carry the machine's
/// whole story on its own.
///
/// Lines are drawn with `DrawChartSegment` — the anti-aliased segment shader
/// `widgets/src/chart.rs` already registers — so this adds no new shader.
#[derive(Script, ScriptHook, Widget)]
pub struct AggregateGraph {
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
    draw_grid: DrawColor,
    #[live]
    draw_seg: DrawChartSegment,
    #[live]
    draw_text: DrawText,
    #[live]
    color_bg: Vec4f,
    #[live]
    color_grid: Vec4f,
    #[live]
    color_text: Vec4f,
    #[live(1.5)]
    line_width: f64,
    #[rust]
    series: Vec<GraphSeries>,
}

/// Below this width the legend stacks instead of running along one row.
const LEGEND_ROW_MIN_WIDTH: f64 = 520.0;

impl AggregateGraph {
    pub fn set_series(&mut self, cx: &mut Cx, series: Vec<GraphSeries>) {
        self.series = series;
        self.draw_bg.redraw(cx);
    }

    /// Legend as coloured swatch + `LABEL value`, laid out inside the plot so
    /// the graph keeps its full height even in a tiny window.
    fn draw_legend(&mut self, cx: &mut Cx2d, plot: Rect) {
        let one_row = plot.size.x >= LEGEND_ROW_MIN_WIDTH;
        let item_width = (plot.size.x / self.series.len().max(1) as f64).min(190.0);
        // A panel behind the legend so the text stays readable where a line
        // happens to run through it. Monospace, so the widest entry's
        // character count is enough to size it.
        let count = self.series.len() as f64;
        let widest = self
            .series
            .iter()
            .map(|item| item.label.chars().count() + item.value.chars().count() + 1)
            .max()
            .unwrap_or(0) as f64;
        // font_size is in points; a monospace advance is about 0.6 em.
        let text_width = widest * self.draw_text.text_style.font_size as f64 * (4.0 / 3.0) * 0.6;
        let backing = if one_row {
            Rect { pos: plot.pos, size: dvec2(item_width * count + 8.0, 16.0) }
        } else {
            Rect {
                pos: plot.pos,
                size: dvec2((text_width + 20.0).min(plot.size.x), count * 13.0 + 6.0),
            }
        };
        self.draw_bg.color = with_alpha(self.color_bg, 0.82);
        self.draw_bg.draw_abs(cx, backing);
        for (index, item) in self.series.iter().enumerate() {
            let origin = if one_row {
                dvec2(plot.pos.x + 4.0 + index as f64 * item_width, plot.pos.y + 3.0)
            } else {
                dvec2(plot.pos.x + 4.0, plot.pos.y + 3.0 + index as f64 * 13.0)
            };
            self.draw_grid.color = item.color;
            self.draw_grid.draw_abs(cx, Rect { pos: origin + dvec2(0.0, 3.0), size: dvec2(7.0, 7.0) });
            self.draw_text.color = item.color;
            self.draw_text.draw_abs(
                cx,
                origin + dvec2(11.0, 0.0),
                &format!("{} {}", item.label, item.value),
            );
        }
    }
}

impl Widget for AggregateGraph {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_bg.color = self.color_bg;
        self.draw_bg.draw_abs(cx, rect);
        if self.series.is_empty() || rect.size.x < 60.0 || rect.size.y < 40.0 {
            return DrawStep::done();
        }

        // Right gutter for the axis labels; the legend floats over the plot.
        let gutter = if rect.size.x > 220.0 { 30.0 } else { 0.0 };
        let plot = Rect {
            pos: rect.pos + dvec2(6.0, 6.0),
            size: dvec2((rect.size.x - 12.0 - gutter).max(10.0), (rect.size.y - 12.0).max(10.0)),
        };
        let py = |value: f64| plot.pos.y + (1.0 - value.clamp(0.0, 100.0) / 100.0) * plot.size.y;

        // A fixed 0/25/50/75/100 grid: every series is already a percentage of
        // its own ceiling, so the axis never needs to rescale and the lines
        // stay comparable frame to frame.
        self.draw_grid.color = self.color_grid;
        self.draw_text.color = self.color_text;
        for step in 0..=4 {
            let value = step as f64 * 25.0;
            let y = py(value);
            self.draw_grid.draw_abs(cx, Rect { pos: dvec2(plot.pos.x, y), size: dvec2(plot.size.x, 1.0) });
            if gutter > 0.0 {
                self.draw_text.draw_abs(
                    cx,
                    dvec2(plot.pos.x + plot.size.x + 5.0, y - 5.0),
                    &format!("{value:.0}"),
                );
            }
        }

        self.draw_seg.mode = 0.0;
        self.draw_seg.thickness = self.line_width as f32;
        let margin = self.line_width + 2.0;
        for index in 0..self.series.len() {
            let count = self.series[index].points.len();
            if count < 2 {
                continue;
            }
            self.draw_seg.color = self.series[index].color;
            let dx = plot.size.x / (count - 1) as f64;
            for point in 0..count - 1 {
                let x0 = plot.pos.x + point as f64 * dx;
                let x1 = x0 + dx;
                let y0 = py(self.series[index].points[point]);
                let y1 = py(self.series[index].points[point + 1]);
                let quad = Rect {
                    pos: dvec2(x0 - margin, y0.min(y1) - margin),
                    size: dvec2(dx + 2.0 * margin, (y0 - y1).abs() + 2.0 * margin),
                };
                self.draw_seg.seg_a = Vec2f { x: margin as f32, y: (y0 - quad.pos.y) as f32 };
                self.draw_seg.seg_b = Vec2f { x: (x1 - quad.pos.x) as f32, y: (y1 - quad.pos.y) as f32 };
                self.draw_seg.draw_abs(cx, quad);
            }
        }

        self.draw_legend(cx, plot);
        DrawStep::done()
    }
}

// ---- ProcessTable ----

/// Column order. `PPID` stays visible in tree mode on purpose: it is the proof
/// that the indentation came from real kernel parentage.
const COLUMNS: [&str; 8] = ["PID", "PPID", "PROGRAM", "USER", "ST", "THR", "MEM", "CPU%"];
const COL_PID: usize = 0;
const COL_PPID: usize = 1;
const COL_NAME: usize = 2;
const COL_USER: usize = 3;
const COL_STATE: usize = 4;
const COL_THREADS: usize = 5;
const COL_MEM: usize = 6;
const COL_CPU: usize = 7;
const COL_WIDTHS: [f64; 8] = [70.0, 70.0, 400.0, 110.0, 40.0, 55.0, 145.0, 85.0];
/// PROGRAM width once the window is too narrow for the full table.
const COMPACT_NAME_WIDTH: f64 = 220.0;
/// Which data columns the table shows, wide and narrow. In a narrow window the
/// columns that matter (who, how much memory, how much CPU) have to stay on
/// screen, so the merely informative ones go rather than scroll away.
const FULL_COLS: [usize; 8] = [COL_PID, COL_PPID, COL_NAME, COL_USER, COL_STATE, COL_THREADS, COL_MEM, COL_CPU];
const COMPACT_COLS: [usize; 5] = [COL_PID, COL_NAME, COL_STATE, COL_MEM, COL_CPU];

/// A drawn row: which process, and where it sits in the tree.
#[derive(Clone, Copy, Debug)]
struct Row {
    /// Index into `ProcessTable::processes`.
    index: usize,
    depth: usize,
    children: usize,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ProcessTable {
    #[deref]
    view: View,
    #[rust]
    processes: Vec<ProcInfo>,
    #[rust]
    rows: Vec<Row>,
    #[rust]
    filter: String,
    #[rust(COL_CPU)]
    sort_column: usize,
    #[rust]
    ascending: bool,
    #[rust(true)]
    tree_mode: bool,
    /// pids whose subtree is folded away.
    #[rust]
    collapsed: HashSet<u32>,
    #[rust]
    selected_pid: Option<u32>,
    /// A process that has just been sent SIGTERM: pressing kill again inside
    /// [`FORCE_WINDOW`] escalates to SIGKILL.
    #[rust]
    force_armed: Option<Armed>,
    #[rust]
    notice: Option<String>,
    #[rust]
    memory_total: u64,
    /// Narrow window: give the PROGRAM column less room so the numbers on the
    /// right stay on screen instead of needing a horizontal scroll.
    #[rust]
    compact: bool,
    /// Last colour pushed to the status row, so the per-sample update can skip
    /// the script eval when nothing changed.
    #[rust]
    row_color: Vec4f,
    /// (killable, force-armed) last pushed to the kill button; `None` until
    /// the first update, so the initial state is always written once.
    #[rust]
    kill_button_state: Option<KillButtonState>,
    #[rust]
    initialized: bool,
    #[live]
    accent_color: Vec4f,
    #[live]
    warning_color: Vec4f,
    #[live]
    muted_color: Vec4f,
}

#[derive(Clone, Copy, Debug)]
struct Armed {
    pid: u32,
    since: Instant,
}

/// How long after a SIGTERM the next kill press escalates to SIGKILL.
const FORCE_WINDOW: Duration = Duration::from_secs(5);

/// (can this be killed, is the force escalation armed).
type KillButtonState = (bool, bool);

/// Processes the kill button refuses: the kernel, init, and task itself.
fn is_protected(pid: u32) -> bool {
    pid == 0 || pid == 1 || pid == std::process::id()
}

impl ProcessTable {
    pub fn set_processes(&mut self, cx: &mut Cx, processes: Vec<ProcInfo>, memory_total: u64) {
        self.processes = processes;
        self.memory_total = memory_total;
        // A folded pid that exited must not keep folding a recycled one.
        if !self.collapsed.is_empty() {
            let live: HashSet<u32> = self.processes.iter().map(|process| process.pid).collect();
            self.collapsed.retain(|pid| live.contains(pid));
        }
        self.rebuild(cx);
    }

    pub fn set_compact(&mut self, cx: &mut Cx, compact: bool) {
        if self.compact == compact && self.initialized {
            return;
        }
        self.compact = compact;
        let grid = self.view.data_grid(cx, ids!(process_grid));
        self.configure_columns(cx, &grid);
        grid.redraw(cx);
    }

    /// The data columns on screen right now.
    fn columns(&self) -> &'static [usize] {
        if self.compact {
            &COMPACT_COLS
        } else {
            &FULL_COLS
        }
    }

    fn column_width(&self, data_col: usize) -> f64 {
        if self.compact && data_col == COL_NAME {
            COMPACT_NAME_WIDTH
        } else {
            COL_WIDTHS[data_col]
        }
    }

    fn configure_columns(&self, cx: &mut Cx, grid: &DataGridRef) {
        let columns = self.columns();
        grid.set_col_labels(columns.iter().map(|&col| COLUMNS[col].to_string()).collect());
        grid.set_grid_size(self.rows.len(), columns.len());
        for (display, &data_col) in columns.iter().enumerate() {
            grid.set_col_width(display, self.column_width(data_col));
        }
        // The sort column may not be on screen in the narrow table; then the
        // header simply carries no arrow, while the order still holds.
        grid.set_sort_indicator(
            columns
                .iter()
                .position(|&col| col == self.sort_column)
                .map(|display| (display, self.ascending)),
        );
        let _ = cx;
    }

    /// Filter → sort → (tree order + fold) → grid rows.
    fn rebuild(&mut self, cx: &mut Cx) {
        let needle = self.filter.trim().to_ascii_lowercase();
        let mut kept: Vec<usize> = if needle.is_empty() {
            (0..self.processes.len()).collect()
        } else {
            let matches: Vec<usize> = (0..self.processes.len())
                .filter(|&index| {
                    let process = &self.processes[index];
                    process.name.to_ascii_lowercase().contains(&needle)
                        || process.cmdline.to_ascii_lowercase().contains(&needle)
                        || process.pid.to_string() == needle
                })
                .collect();
            if self.tree_mode {
                // Keep the ancestors of every hit so the tree stays connected.
                self.with_ancestors(matches)
            } else {
                matches
            }
        };

        let column = self.sort_column;
        let ascending = self.ascending;
        let processes = &self.processes;
        kept.sort_by(|&a, &b| {
            let (a, b) = (&processes[a], &processes[b]);
            let order = match column {
                COL_PID => a.pid.cmp(&b.pid),
                COL_PPID => a.ppid.cmp(&b.ppid),
                COL_NAME => a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()),
                COL_USER => a.user.cmp(&b.user),
                COL_STATE => a.state.as_str().cmp(b.state.as_str()),
                COL_THREADS => a.threads.cmp(&b.threads),
                COL_MEM => a.mem_rss.cmp(&b.mem_rss),
                _ => a.cpu_pct.partial_cmp(&b.cpu_pct).unwrap_or(Ordering::Equal),
            };
            let order = if ascending { order } else { order.reverse() };
            order.then_with(|| a.pid.cmp(&b.pid))
        });

        self.rows = if self.tree_mode {
            let pairs: Vec<(u32, u32)> =
                kept.iter().map(|&index| (processes[index].pid, processes[index].ppid)).collect();
            let ordered = crate::backend::tree_order(&pairs);
            let mut rows = Vec::with_capacity(ordered.len());
            // Skip everything deeper than a folded node.
            let mut fold_depth: Option<usize> = None;
            for row in ordered {
                match fold_depth {
                    Some(depth) if row.depth > depth => continue,
                    _ => fold_depth = None,
                }
                let index = kept[row.index];
                if row.children > 0 && self.collapsed.contains(&processes[index].pid) {
                    fold_depth = Some(row.depth);
                }
                rows.push(Row { index, depth: row.depth, children: row.children });
            }
            rows
        } else {
            kept.iter().map(|&index| Row { index, depth: 0, children: 0 }).collect()
        };

        let grid = self.view.data_grid(cx, ids!(process_grid));
        grid.set_grid_size(self.rows.len(), self.columns().len());
        if let Some(pid) = self.selected_pid {
            match self.row_of_pid(pid) {
                // The selection follows the *process*, not the row index, so a
                // refresh or a re-sort keeps the same one highlighted.
                Some(row) => grid.set_selection(cx, Some(row_selection(row))),
                None => {
                    // The selected process exited or was filtered away.
                    self.selected_pid = None;
                    self.force_armed = None;
                    grid.set_selection(cx, None);
                }
            }
        }
        self.update_status(cx);
        grid.redraw(cx);
    }

    /// Every index in `seed`, plus the chain of parents above each one.
    fn with_ancestors(&self, seed: Vec<usize>) -> Vec<usize> {
        let mut index_of_pid: HashMap<u32, usize> = HashMap::with_capacity(self.processes.len());
        for (index, process) in self.processes.iter().enumerate() {
            index_of_pid.entry(process.pid).or_insert(index);
        }
        let mut wanted: HashSet<u32> = HashSet::with_capacity(seed.len() * 2);
        for &index in &seed {
            let mut pid = self.processes[index].pid;
            // The hop cap is a cycle guard; `wanted.insert` already stops on a
            // parent we have walked through before.
            let mut hops = 0;
            while wanted.insert(pid) && hops < 64 {
                let Some(&at) = index_of_pid.get(&pid) else { break };
                let parent = self.processes[at].ppid;
                if parent == 0 || parent == pid {
                    break;
                }
                pid = parent;
                hops += 1;
            }
        }
        (0..self.processes.len())
            .filter(|&index| wanted.contains(&self.processes[index].pid))
            .collect()
    }

    fn row_of_pid(&self, pid: u32) -> Option<usize> {
        self.rows.iter().position(|row| self.processes[row.index].pid == pid)
    }

    fn selected_row(&self) -> Option<usize> {
        self.selected_pid.and_then(|pid| self.row_of_pid(pid))
    }

    fn update_status(&mut self, cx: &mut Cx) {
        let direction = if self.ascending { "asc" } else { "desc" };
        let column = COLUMNS.get(self.sort_column).copied().unwrap_or("CPU%");
        let mode = if self.tree_mode { "TREE" } else { "FLAT" };
        let selection = self.selected_pid.map(|pid| format!(" · PID {pid}")).unwrap_or_default();
        // The key hints are the first thing to go in a narrow window: the
        // buttons next to this line already say what they do.
        let hints = if self.compact { "" } else { " · T tree · SPACE fold · K kill" };
        let status = format!(
            "{mode} · {}/{} · {column} {direction}{selection}{hints}",
            self.rows.len(),
            self.processes.len()
        );

        // In a narrow window the toolbar has no room left beside the filter
        // box, so the status moves down to the full-width row under it — where
        // a kill notice, when there is one, still wins.
        let armed = self.force_is_armed();
        let (toolbar_text, row_text, row_color) = match (self.compact, &self.notice) {
            (true, Some(notice)) => (String::new(), notice.clone(), self.warning_or_accent(armed)),
            (true, None) => (String::new(), status, self.muted_color),
            (false, notice) => (
                status,
                notice.clone().unwrap_or_default(),
                self.warning_or_accent(armed),
            ),
        };
        self.view.label(cx, ids!(process_status)).set_text(cx, &toolbar_text);
        let mut label = self.view.label(cx, ids!(confirm_label));
        // This runs on every sample — ten times a second at the fastest
        // refresh — and each `script_apply_eval!` allocates script objects, so
        // only re-tint when the colour actually changed.
        if self.row_color != row_color {
            self.row_color = row_color;
            script_apply_eval!(cx, label, {draw_text +: {color: #(row_color)}});
        }
        label.set_text(cx, &row_text);

        // The button says which of the two things the next press will do, and
        // goes flat when there is nothing it may signal. Both setters kick the
        // animator, so — like the tint above — they only run on a change.
        let killable = self.selected_pid.is_some_and(|pid| !is_protected(pid));
        if self.kill_button_state != Some((killable, armed)) {
            self.kill_button_state = Some((killable, armed));
            let button = self.view.button(cx, ids!(kill_button));
            button.set_text(cx, if armed { "KILL — FORCE" } else { "KILL" });
            button.set_enabled(cx, killable);
        }
    }

    fn warning_or_accent(&self, armed: bool) -> Vec4f {
        if armed {
            self.warning_color
        } else {
            self.accent_color
        }
    }

    fn force_is_armed(&self) -> bool {
        match (self.force_armed, self.selected_pid) {
            (Some(armed), Some(pid)) => armed.pid == pid && armed.since.elapsed() <= FORCE_WINDOW,
            _ => false,
        }
    }

    fn toggle_tree(&mut self, cx: &mut Cx) {
        self.tree_mode = !self.tree_mode;
        self.notice = None;
        self.view
            .button(cx, ids!(tree_toggle))
            .set_text(cx, if self.tree_mode { "TREE" } else { "FLAT" });
        self.rebuild(cx);
    }

    /// Fold/unfold the subtree under `row`, if it has one.
    fn toggle_fold(&mut self, cx: &mut Cx, row: usize) {
        if !self.tree_mode {
            return;
        }
        let Some(entry) = self.rows.get(row).copied() else { return };
        if entry.children == 0 {
            return;
        }
        let pid = self.processes[entry.index].pid;
        if !self.collapsed.remove(&pid) {
            self.collapsed.insert(pid);
        }
        self.rebuild(cx);
    }

    /// Kill the selected process. The first press asks politely (SIGTERM); a
    /// second press within [`FORCE_WINDOW`], or a shift-click, forces it
    /// (SIGKILL). No modal — the status row carries the escalation offer.
    fn kill_selected(&mut self, cx: &mut Cx, force_requested: bool) {
        let Some(pid) = self.selected_pid else {
            self.notice = Some("select a process row first".to_string());
            self.update_status(cx);
            return;
        };
        if is_protected(pid) {
            self.notice = Some(format!("PID {pid} is protected and will not be signalled"));
            self.update_status(cx);
            return;
        }
        let force = force_requested
            || self
                .force_armed
                .is_some_and(|armed| armed.pid == pid && armed.since.elapsed() <= FORCE_WINDOW);
        self.notice = Some(match terminate(pid, force) {
            Ok(()) => {
                let signal = if force { "SIGKILL" } else { "SIGTERM" };
                // Killing something is worth a line in the log.
                log!("task: {signal} -> pid {pid}");
                if force {
                    self.force_armed = None;
                    format!("SIGKILL sent to PID {pid}")
                } else {
                    self.force_armed = Some(Armed { pid, since: Instant::now() });
                    format!("SIGTERM sent to PID {pid}  ·  kill again within 5 s to force (SIGKILL)")
                }
            }
            Err(error) => {
                self.force_armed = None;
                format!("could not signal PID {pid}: {error}")
            }
        });
        self.update_status(cx);
    }

    pub fn apply_theme(&mut self, cx: &mut Cx, theme: Theme) {
        self.accent_color = theme.accent;
        self.warning_color = theme.red;
        self.muted_color = theme.muted;
        let background = theme.background;
        let foreground = theme.foreground;
        let dark = theme.surface;
        let muted = theme.muted;
        let accent = theme.accent;
        let panel = theme.panel;
        let selection = with_alpha(theme.accent, 0.16);
        let selection_guide = with_alpha(theme.accent, 0.4);

        let mut grid = self.view.data_grid(cx, ids!(process_grid));
        script_apply_eval!(cx, grid, {
            color_bg: #(background)
            color_cell: #(background)
            color_cell_alt: #(panel)
            color_text: #(foreground)
            color_header: #(dark)
            color_header_active: #(panel)
            color_header_text: #(accent)
            color_selection: #(selection)
            color_selection_border: #(accent)
            color_drag_marker: #(accent)
            color_resize_guide: #(selection_guide)
            draw_cell +: {border_color: #(panel)}
            draw_text +: {color: #(foreground)}
            draw_text_bold +: {color: #(foreground)}
        });
        // No draw_bg here: makepad_wm_theme::apply already flattened every text-field
        // well, and re-styling it locally would put the gradient back.
        let mut confirm_row = self.view.view(cx, ids!(confirm_row));
        script_apply_eval!(cx, confirm_row, {draw_bg +: {color: #(dark) border_color: #(panel)}});
        let mut filter_label = self.view.label(cx, ids!(filter_label));
        script_apply_eval!(cx, filter_label, {draw_text +: {color: #(accent)}});
        let mut status = self.view.label(cx, ids!(process_status));
        script_apply_eval!(cx, status, {draw_text +: {color: #(muted)}});
    }

    /// Text + style for one cell of one row.
    fn cell(&self, row: &Row, col: usize) -> (String, CellStyle) {
        let process = &self.processes[row.index];
        let right = CellStyle { align: 1.0, ..CellStyle::default() };
        match col {
            COL_PID => (process.pid.to_string(), CellStyle { color: Some(self.muted_color), ..right }),
            COL_PPID => (process.ppid.to_string(), CellStyle { color: Some(self.muted_color), ..right }),
            COL_NAME => {
                // ▲/▼ and not ▶/▼: the code font has the up/down triangles
                // (the data grid's own sort indicator uses them) but no
                // right-pointing one — that renders as .notdef.
                let marker = if row.children == 0 {
                    "   "
                } else if self.collapsed.contains(&process.pid) {
                    " ▲ "
                } else {
                    " ▼ "
                };
                let indent = if self.tree_mode { "  ".repeat(row.depth.min(24)) } else { String::new() };
                let prefix = if self.tree_mode { marker } else { "" };
                (
                    format!("{indent}{prefix}{}", process.name),
                    CellStyle { bold: true, ..CellStyle::default() },
                )
            }
            COL_USER => (process.user.clone(), CellStyle::default()),
            COL_STATE => (
                process.state.as_str().to_string(),
                CellStyle { align: 0.5, color: Some(self.muted_color), ..CellStyle::default() },
            ),
            COL_THREADS => (process.threads.to_string(), CellStyle { color: Some(self.muted_color), ..right }),
            COL_MEM => {
                let percent = if self.memory_total > 0 {
                    process.mem_rss as f64 / self.memory_total as f64 * 100.0
                } else {
                    0.0
                };
                (format!("{}  {percent:>4.1}%", format_bytes(process.mem_rss)), right)
            }
            _ => {
                let color = if process.cpu_pct >= 80.0 { self.warning_color } else { self.accent_color };
                (
                    format!("{:>6.1}", process.cpu_pct),
                    CellStyle { color: Some(color), bold: true, ..right },
                )
            }
        }
    }
}

impl Widget for ProcessTable {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut grid) = step.as_data_grid().borrow_mut() {
                let columns = self.columns();
                if !self.initialized {
                    self.initialized = true;
                    grid.set_col_labels(columns.iter().map(|&col| COLUMNS[col].to_string()).collect());
                    for (display, &data_col) in columns.iter().enumerate() {
                        grid.set_col_width(display, self.column_width(data_col));
                    }
                    grid.set_sort_indicator(
                        columns
                            .iter()
                            .position(|&col| col == self.sort_column)
                            .map(|display| (display, self.ascending)),
                    );
                }
                grid.set_grid_size(self.rows.len(), columns.len());
                while let Some(cell) = grid.next_cell(cx) {
                    let Some(row) = self.rows.get(cell.row).copied() else { continue };
                    let Some(&data_col) = columns.get(cell.col) else { continue };
                    let (text, style) = self.cell(&row, data_col);
                    grid.cell_text_styled(cx, &cell, &text, style);
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);

        if let Event::Actions(actions) = event {
            if self.view.button(cx, ids!(tree_toggle)).clicked(actions) {
                self.toggle_tree(cx);
            }
            // Shift-click skips straight to SIGKILL.
            if let Some(modifiers) = self.view.button(cx, ids!(kill_button)).clicked_modifiers(actions) {
                self.kill_selected(cx, modifiers.shift);
            }
            if let Some(filter) = self.view.text_input(cx, ids!(filter_input)).changed(actions) {
                self.filter = filter;
                self.notice = None;
                self.rebuild(cx);
            }
            let grid = self.view.data_grid(cx, ids!(process_grid));
            for action in grid.actions(actions) {
                match action {
                    DataGridAction::HeaderClicked { col, .. } => {
                        // `col` indexes the columns on screen, which is a
                        // subset in the narrow table.
                        let Some(&data_col) = self.columns().get(col) else { continue };
                        if self.sort_column == data_col {
                            self.ascending = !self.ascending;
                        } else {
                            self.sort_column = data_col;
                            // Numbers read best biggest-first, names A-Z.
                            self.ascending = !matches!(data_col, COL_MEM | COL_CPU | COL_THREADS);
                        }
                        let grid = self.view.data_grid(cx, ids!(process_grid));
                        self.configure_columns(cx, &grid);
                        self.rebuild(cx);
                    }
                    DataGridAction::SelectionChanged { selection } => {
                        let row = selection.map(|selection| selection.head.0);
                        let pid = row
                            .and_then(|row| self.rows.get(row))
                            .map(|row| self.processes[row.index].pid);
                        // Only a *different* process disarms the force offer:
                        // the grid re-emits this action for things like a
                        // drag-scroll, and losing the arm to that would make a
                        // second kill press silently repeat SIGTERM.
                        if pid != self.selected_pid {
                            self.selected_pid = pid;
                            self.force_armed = None;
                        }
                        // A click lands on one cell; the selection a process
                        // manager wants is the whole line.
                        if let Some(row) = row {
                            let grid = self.view.data_grid(cx, ids!(process_grid));
                            grid.set_selection(cx, Some(row_selection(row)));
                        }
                        self.update_status(cx);
                    }
                    DataGridAction::CellDoubleClicked { row, .. } => self.toggle_fold(cx, row),
                    _ => {}
                }
            }
        }

        if let Event::KeyDown(key) = event {
            // The filter box owns every keystroke while it has focus.
            if self.view.text_input(cx, ids!(filter_input)).key_focus(cx)
                || key.modifiers.logo
                || key.modifiers.control
            {
                return;
            }
            match key.key_code {
                // Delete/Backspace is the shortcut the kill button advertises;
                // K stays as the btop-style one. Shift forces.
                KeyCode::KeyK | KeyCode::Delete | KeyCode::Backspace => {
                    self.kill_selected(cx, key.modifiers.shift)
                }
                KeyCode::KeyT => self.toggle_tree(cx),
                KeyCode::Space => {
                    if let Some(row) = self.selected_row() {
                        self.toggle_fold(cx, row);
                    }
                }
                KeyCode::Escape => {
                    if self.force_armed.take().is_some() {
                        self.notice = Some("force cancelled".to_string());
                        self.update_status(cx);
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

fn with_alpha(mut value: Vec4f, alpha: f32) -> Vec4f {
    value.w = alpha;
    value
}
