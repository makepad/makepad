//! mptask — the task manager / activity monitor of the mp* app family.
//!
//! btop's layout: per-core CPU bars beside a 60-second load graph, a memory
//! panel, network up/down graphs, and a process table that switches between a
//! flat sortable list and a real parent/child tree.
//!
//! All numbers come from [`backend`], which is one trait with a native
//! implementation per OS — never `ps`/`top` output.

pub use makepad_widgets;

use makepad_widgets::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

mod backend;
mod sampler;
mod widgets;

use backend::Snapshot;
use widgets::{AggregateGraph, GraphSeries, MeterBars, MeterRow, ProcessTable};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Panel = RectView{
        width: Fill
        height: Fill
        flow: Down
        padding: 8
        spacing: 5
        draw_bg +: {
            color: #x1a1b26
            border_color: #x3b4261
            border_size: 1.0
        }
    }

    let PanelTitle = Label{
        width: Fill
        height: 18
        draw_text +: {
            color: #x7aa2f7
            text_style: theme.font_code{font_size: 9.5}
        }
    }

    let MetricText = Label{
        draw_text +: {
            color: #xa9b1d6
            text_style: theme.font_code{font_size: 8.5}
        }
    }

    // Line only: the series line on the panel well, muted gridlines and the
    // dashed last-value rule. `color_fill` is fully transparent, which makes
    // TrendChart's area pass draw nothing — no gradient, no glow.
    let TelemetryChart = TrendChart{
        width: Fill
        height: Fill
        color_bg: #x16161e
        color_grid: #x41486859
        color_line: #x7aa2f7
        color_fill: #x00000000
        color_up: #x9ece6a
        color_down: #xf7768e
        color_text: #x565f89
        color_accent: #xe0af68
        line_width: 1.5
        draw_text +: {text_style: theme.font_code{font_size: 7.0}}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "mptask"
                window.inner_size: vec2(1400 900)
                body +: {
                    app_bg := RectView{
                        width: Fill
                        height: Fill
                        flow: Down
                        padding: 8
                        spacing: 8
                        draw_bg +: {color: #x1a1b26}

                        top_row := View{
                            width: Fill
                            height: 330
                            flow: Right
                            spacing: 8

                            // The one panel that survives every breakpoint.
                            aggregate_panel := Panel{
                                width: Fill
                                aggregate_title := PanelTitle{text: "SYSTEM"}
                                aggregate_graph := AggregateGraph{}
                            }

                            cpu_panel := Panel{
                                width: 420
                                cpu_title := PanelTitle{text: "CPU  --.-%"}
                                cpu_cores := MeterBars{height: 150 columns: 2}
                                cpu_chart := TelemetryChart{}
                            }

                            side_column := View{
                                width: 440
                                height: Fill
                                flow: Down
                                spacing: 8

                                memory_panel := Panel{
                                    height: 176
                                    memory_title := PanelTitle{text: "MEMORY"}
                                    View{
                                        width: Fill
                                        height: Fill
                                        flow: Right
                                        spacing: 8
                                        memory_bars := MeterBars{
                                            width: 232
                                            height: Fill
                                            columns: 1
                                            gradient: false
                                            label_width: 48.0
                                            value_width: 66.0
                                            bar_color: #x9ece6a
                                        }
                                        memory_chart := TelemetryChart{
                                            color_line: #x9ece6a
                                            color_accent: #x9ece6a
                                        }
                                    }
                                }

                                network_panel := Panel{
                                    height: Fill
                                    network_title := PanelTitle{text: "NETWORK"}
                                    View{
                                        width: Fill
                                        height: 18
                                        flow: Right
                                        spacing: 18
                                        network_down := MetricText{text: "DOWN 0 B/s"}
                                        network_up := MetricText{text: "UP 0 B/s"}
                                    }
                                    View{
                                        width: Fill
                                        height: Fill
                                        flow: Right
                                        spacing: 6
                                        down_chart := TelemetryChart{
                                            color_line: #x7dcfff
                                            color_accent: #x7dcfff
                                        }
                                        up_chart := TelemetryChart{
                                            color_line: #xbb9af7
                                            color_accent: #xbb9af7
                                        }
                                    }
                                }
                            }
                        }

                        process_panel := Panel{
                            height: Fill
                            process_header := View{
                                width: Fill
                                height: 22
                                flow: Right
                                spacing: 10
                                align: Align{y: 0.5}
                                process_title := PanelTitle{width: Fill text: "PROCESSES"}
                                refresh_label := MetricText{text: "REFRESH"}
                                refresh_select := DropDown{
                                    width: 86
                                    height: 22
                                    labels: ["0.1 s" "0.2 s" "0.5 s" "1 s" "2 s" "5 s" "10 s"]
                                    selected_item: 0
                                }
                            }
                            process_table := ProcessTable{}
                        }
                    }
                }
            }
        }
    }
}

// ---- theme ----

/// The palette the whole app paints with. Sourced from the mp* WM theme
/// (`MPWM_THEME_SPLASH`) through `mp_theme::current()`, so mptask matches
/// mpterm/mpfiles/mpwm; the fallback is Tokyo Night.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub accent: Vec4f,
    /// Window and cell background.
    pub background: Vec4f,
    /// Chart/input wells — one step darker than the background.
    pub surface: Vec4f,
    /// Zebra rows, borders, meter tracks — one step lighter.
    pub panel: Vec4f,
    pub foreground: Vec4f,
    pub muted: Vec4f,
    pub red: Vec4f,
    pub green: Vec4f,
    pub yellow: Vec4f,
    pub blue: Vec4f,
    pub cyan: Vec4f,
    pub magenta: Vec4f,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: color("#7aa2f7"),
            background: color("#1a1b26"),
            surface: color("#16161e"),
            panel: color("#24283b"),
            foreground: color("#a9b1d6"),
            muted: color("#565f89"),
            red: color("#f7768e"),
            green: color("#9ece6a"),
            yellow: color("#e0af68"),
            blue: color("#7aa2f7"),
            cyan: color("#7dcfff"),
            magenta: color("#bb9af7"),
        }
    }
}

impl Theme {
    /// The WM palette if one is exported, else the built-in fallback. Uses the
    /// same scanner `mp_theme::apply` retints `mod.theme` with, so the stock
    /// widgets and mptask's own drawing can never disagree.
    fn from_environment() -> Self {
        let fallback = Self::default();
        let Some(palette) = mp_theme::current() else { return fallback };
        let pick = |key: &str, default: Vec4f| palette.get(key).and_then(parse_color).unwrap_or(default);
        // Imported omarchy themes carry their hues as the terminal palette;
        // take those when a theme does not name the colours directly.
        let hue = |key: &str, term: &str, default: Vec4f| {
            palette
                .get(key)
                .or_else(|| palette.get(term))
                .and_then(parse_color)
                .unwrap_or(default)
        };
        Self {
            accent: pick("accent", fallback.accent),
            background: pick("background", fallback.background),
            surface: pick("darker_background", fallback.surface),
            panel: pick("lighter_background", fallback.panel),
            foreground: pick("foreground", fallback.foreground),
            muted: pick("dark_foreground", fallback.muted),
            red: hue("red", "term.color1", fallback.red),
            green: hue("green", "term.color2", fallback.green),
            yellow: hue("yellow", "term.color3", fallback.yellow),
            blue: hue("blue", "term.color4", fallback.blue),
            cyan: hue("cyan", "term.color6", fallback.cyan),
            magenta: hue("magenta", "term.color5", fallback.magenta),
        }
    }
}

fn color(value: &str) -> Vec4f {
    parse_color(value).unwrap_or(vec4(1.0, 0.0, 1.0, 1.0))
}

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`.
fn parse_color(value: &str) -> Option<Vec4f> {
    let digits = value.trim().strip_prefix('#').unwrap_or(value.trim());
    let rgba = match digits.len() {
        3 | 4 => {
            let mut expanded = String::with_capacity(8);
            for digit in digits.chars() {
                expanded.push(digit);
                expanded.push(digit);
            }
            if digits.len() == 3 {
                expanded.push_str("ff");
            }
            u32::from_str_radix(&expanded, 16).ok()?
        }
        6 => u32::from_str_radix(digits, 16).ok()?.checked_shl(8)? | 0xff,
        8 => u32::from_str_radix(digits, 16).ok()?,
        _ => return None,
    };
    Some(Vec4f::from_u32(rgba))
}

fn with_alpha(mut value: Vec4f, alpha: f32) -> Vec4f {
    value.w = alpha;
    value
}

// ---- app ----

/// Samples the graphs keep. The window is a sample count, not a wall-clock
/// span, so raising the refresh rate makes the graphs scroll faster instead of
/// squashing the same minute into fewer pixels.
const HISTORY: usize = 60;

/// The refresh-rate picker, in the order the drop-down lists them.
const REFRESH_CHOICES_MS: [u64; 7] = [100, 200, 500, 1000, 2000, 5000, 10_000];
/// Index of the default (0.1 s) — must match `selected_item` in the DSL.
const DEFAULT_REFRESH: usize = 0;

/// Height of the metrics band when the process table is on screen. Matches
/// `top_row`'s height in the DSL.
const TOP_ROW_HEIGHT: f64 = 330.0;
/// The metrics band never shrinks past this, or the graph stops reading.
const MIN_TOP_ROW_HEIGHT: f64 = 180.0;

/// How the window is laid out at the current size. One place, three states.
///
/// The thresholds are in layout points and are compared against the window's
/// *inner* size, so the app behaves the same whether it is a free window or an
/// mpwm tile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Density {
    /// Aggregate graph + per-metric panels + the process table.
    #[default]
    Full,
    /// Aggregate graph + the process table.
    Medium,
    /// The aggregate graph alone, filling the window.
    Small,
}

impl Density {
    /// Full needs room for the three top panels side by side (the two detail
    /// columns are 420 + 440 wide) *and* a table worth reading underneath.
    /// Medium still fits a table. Below that only the graph reads at all.
    /// The table's own chrome (title, toolbar, status row, column header) is
    /// about 110 pt before a single process row, and the metrics band never
    /// goes below 180 pt, so under ~460 pt tall Medium would be a header with
    /// nothing under it — better one graph shown properly than two clipped
    /// halves. 620 pt wide is where the narrow column set stops fitting.
    pub fn for_size(size: Vec2d) -> Self {
        if size.x < 620.0 || size.y < 460.0 {
            Density::Small
        } else if size.x < 1180.0 || size.y < 780.0 {
            Density::Medium
        } else {
            Density::Full
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    snapshot_rx: Option<Receiver<Snapshot>>,
    #[rust]
    sampler_started: bool,
    /// Shared with the sampler thread: the tick period in milliseconds. The
    /// thread re-reads it between sleeps, so a change takes effect at once
    /// even when it was mid-way through a 10 second wait.
    #[rust]
    interval_ms: Arc<AtomicU64>,
    #[rust]
    theme: Theme,
    #[rust]
    density: Density,
    #[rust]
    density_applied: bool,
    #[rust]
    cpu_history: Vec<f64>,
    #[rust]
    memory_history: Vec<f64>,
    #[rust]
    down_history: Vec<f64>,
    #[rust]
    up_history: Vec<f64>,
}

impl App {
    fn start_sampler(&mut self) {
        if self.sampler_started {
            return;
        }
        self.sampler_started = true;
        self.interval_ms.store(REFRESH_CHOICES_MS[DEFAULT_REFRESH], Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.snapshot_rx = Some(rx);
        sampler::spawn(tx, self.interval_ms.clone());
    }

    /// Drain to the newest snapshot: if the UI was busy we want the latest
    /// reading, not a backlog replayed one frame at a time.
    fn drain_snapshots(&mut self, cx: &mut Cx) {
        let mut newest = None;
        if let Some(rx) = &self.snapshot_rx {
            while let Ok(snapshot) = rx.try_recv() {
                newest = Some(snapshot);
            }
        }
        if let Some(snapshot) = newest {
            self.apply_snapshot(cx, snapshot);
        }
    }

    fn apply_snapshot(&mut self, cx: &mut Cx, snapshot: Snapshot) {
        let cores = snapshot.cpu_cores.len();
        self.ui.label(cx, ids!(cpu_title)).set_text(
            cx,
            &format!(
                "CPU  {:>5.1}%   ·  {cores} CORES  ·  LOAD {:.2} {:.2} {:.2}",
                snapshot.cpu_total, snapshot.load_avg[0], snapshot.load_avg[1], snapshot.load_avg[2]
            ),
        );

        // Keep the bars readable: more cores means more columns, not thinner rows.
        let columns = if cores > 24 {
            4
        } else if cores > 8 {
            2
        } else {
            1
        };
        let core_rows = snapshot
            .cpu_cores
            .iter()
            .enumerate()
            .map(|(index, percent)| MeterRow {
                label: format!("CPU{index}"),
                value: format!("{percent:>5.1}%"),
                fraction: percent / 100.0,
            })
            .collect();
        if let Some(mut meter) = self.ui.widget(cx, ids!(cpu_cores)).borrow_mut::<MeterBars>() {
            meter.set_rows(cx, core_rows, columns);
        }

        let memory = snapshot.mem;
        let total = memory.total.max(1) as f64;
        self.ui.label(cx, ids!(memory_title)).set_text(
            cx,
            &format!(
                "MEMORY  {} / {}  ·  SWAP {} / {}",
                format_bytes(memory.used),
                format_bytes(memory.total),
                format_bytes(memory.swap_used),
                format_bytes(memory.swap_total)
            ),
        );
        let memory_rows = [
            ("TOTAL", memory.total),
            ("USED", memory.used),
            ("AVAIL", memory.available),
            ("CACHE", memory.cache),
            ("FREE", memory.free),
        ]
        .into_iter()
        .map(|(label, bytes)| MeterRow {
            label: label.to_string(),
            value: format_bytes(bytes),
            fraction: bytes as f64 / total,
        })
        .collect();
        if let Some(mut meter) = self.ui.widget(cx, ids!(memory_bars)).borrow_mut::<MeterBars>() {
            meter.set_rows(cx, memory_rows, 1);
        }

        self.ui
            .label(cx, ids!(network_down))
            .set_text(cx, &format!("DOWN {:>10}/s", format_bytes(snapshot.net.rx_per_second as u64)));
        self.ui
            .label(cx, ids!(network_up))
            .set_text(cx, &format!("UP {:>10}/s", format_bytes(snapshot.net.tx_per_second as u64)));
        self.ui.label(cx, ids!(network_title)).set_text(
            cx,
            &format!(
                "NETWORK  ·  {} in  ·  {} out",
                format_bytes(snapshot.net.rx_total),
                format_bytes(snapshot.net.tx_total)
            ),
        );

        push_history(&mut self.cpu_history, snapshot.cpu_total);
        push_history(&mut self.memory_history, memory.used as f64 / total * 100.0);
        // Graphs are in KiB/s: a byte-per-second axis is unreadable on a LAN.
        push_history(&mut self.down_history, snapshot.net.rx_per_second / 1024.0);
        push_history(&mut self.up_history, snapshot.net.tx_per_second / 1024.0);
        self.ui.trend_chart(cx, ids!(cpu_chart)).set_series(cx, &self.cpu_history);
        self.ui.trend_chart(cx, ids!(memory_chart)).set_series(cx, &self.memory_history);
        self.ui.trend_chart(cx, ids!(down_chart)).set_series(cx, &self.down_history);
        self.ui.trend_chart(cx, ids!(up_chart)).set_series(cx, &self.up_history);
        self.update_aggregate(cx, &snapshot);

        self.ui.label(cx, ids!(process_title)).set_text(
            cx,
            &format!(
                "PROCESSES  ·  backend {}  ·  up {}",
                snapshot.backend,
                format_uptime(snapshot.uptime_seconds)
            ),
        );
        if let Some(mut table) = self.ui.widget(cx, ids!(process_table)).borrow_mut::<ProcessTable>() {
            table.set_processes(cx, snapshot.processes, memory.total);
        }
        self.ui.redraw(cx);
    }

    /// Put every metric on the one always-visible graph.
    ///
    /// CPU and memory are already percentages. The two network rates share one
    /// scale — the largest rate seen in the window — so up and down stay
    /// comparable with each other and the legend carries the real figure.
    fn update_aggregate(&mut self, cx: &mut Cx, snapshot: &Snapshot) {
        let peak = self
            .down_history
            .iter()
            .chain(self.up_history.iter())
            .fold(0.0f64, |peak, value| peak.max(*value));
        let scale = |history: &Vec<f64>| {
            if peak <= 0.0 {
                vec![0.0; history.len()]
            } else {
                history.iter().map(|value| value / peak * 100.0).collect()
            }
        };
        let series = vec![
            GraphSeries {
                label: "CPU".to_string(),
                value: format!("{:.1}%", snapshot.cpu_total),
                color: self.theme.blue,
                points: self.cpu_history.clone(),
            },
            GraphSeries {
                label: "MEM".to_string(),
                value: format!("{:.1}%", self.memory_history.last().copied().unwrap_or(0.0)),
                color: self.theme.green,
                points: self.memory_history.clone(),
            },
            GraphSeries {
                label: "NET DN".to_string(),
                value: format!("{}/s", format_bytes(snapshot.net.rx_per_second as u64)),
                color: self.theme.cyan,
                points: scale(&self.down_history),
            },
            GraphSeries {
                label: "NET UP".to_string(),
                value: format!("{}/s", format_bytes(snapshot.net.tx_per_second as u64)),
                color: self.theme.magenta,
                points: scale(&self.up_history),
            },
        ];
        if let Some(mut graph) = self.ui.widget(cx, ids!(aggregate_graph)).borrow_mut::<AggregateGraph>() {
            graph.set_series(cx, series);
        }
        self.ui.label(cx, ids!(aggregate_title)).set_text(
            cx,
            &format!(
                "SYSTEM  ·  {} samples  ·  net axis peak {}/s",
                self.cpu_history.len(),
                format_bytes((peak * 1024.0) as u64)
            ),
        );
    }

    /// Fold the layout down as the window shrinks. Panels are hidden whole —
    /// never clipped in half — so nothing ever needs a scrollbar for chrome.
    fn apply_layout(&mut self, cx: &mut Cx, size: Vec2d) {
        let density = Density::for_size(size);
        let changed = !self.density_applied || self.density != density;
        self.density = density;
        self.density_applied = true;
        let details = density == Density::Full;
        let table = density != Density::Small;
        if changed {
            self.ui.view(cx, ids!(cpu_panel)).set_visible(cx, details);
            self.ui.view(cx, ids!(side_column)).set_visible(cx, details);
            self.ui.view(cx, ids!(process_panel)).set_visible(cx, table);
            if let Some(mut table) = self.ui.widget(cx, ids!(process_table)).borrow_mut::<ProcessTable>() {
                table.set_compact(cx, !details);
            }
            log!("mptask: layout {density:?} at {:.0}x{:.0}", size.x, size.y);
        }
        // With the table gone the graph takes the whole window; with it there
        // the metrics band keeps at most 45% so the table always has rows to
        // show. The walk is set on the widget directly — `Fill` is a DSL name
        // and does not resolve inside a `script_apply_eval!` body.
        if let Some(mut top_row) = self.ui.widget(cx, ids!(top_row)).borrow_mut::<View>() {
            top_row.walk.height = if table {
                Size::Fixed(TOP_ROW_HEIGHT.min(size.y * 0.45).max(MIN_TOP_ROW_HEIGHT))
            } else {
                Size::fill()
            };
        }
        self.ui.redraw(cx);
    }

    fn layout_from_window(&mut self, cx: &mut Cx) {
        let size = self.ui.window(cx, ids!(main_window)).get_inner_size(cx);
        // Before the first draw the window reports nothing; the geometry event
        // that follows carries the real size.
        if size.x > 1.0 && size.y > 1.0 {
            self.apply_layout(cx, size);
        }
    }

    fn apply_theme(&mut self, cx: &mut Cx) {
        let theme = self.theme;
        let background = theme.background;
        let foreground = theme.foreground;
        let surface = theme.surface;
        let border = theme.panel;
        let accent = theme.accent;
        let muted = theme.muted;

        let mut app_bg = self.ui.view(cx, ids!(app_bg));
        script_apply_eval!(cx, app_bg, {draw_bg +: {color: #(background)}});
        for path in [ids!(cpu_panel), ids!(memory_panel), ids!(network_panel), ids!(process_panel)] {
            let mut view = self.ui.view(cx, path);
            script_apply_eval!(cx, view, {draw_bg +: {color: #(background) border_color: #(border)}});
        }
        for path in [ids!(cpu_title), ids!(memory_title), ids!(network_title), ids!(process_title)] {
            let mut label = self.ui.label(cx, path);
            script_apply_eval!(cx, label, {draw_text +: {color: #(accent)}});
        }
        for path in [ids!(network_down), ids!(network_up)] {
            let mut label = self.ui.label(cx, path);
            script_apply_eval!(cx, label, {draw_text +: {color: #(foreground)}});
        }
        self.apply_chart_theme(cx, ids!(cpu_chart), theme.blue);
        self.apply_chart_theme(cx, ids!(memory_chart), theme.green);
        self.apply_chart_theme(cx, ids!(down_chart), theme.cyan);
        self.apply_chart_theme(cx, ids!(up_chart), theme.magenta);

        for (path, bar_color) in [(ids!(cpu_cores), theme.accent), (ids!(memory_bars), theme.green)] {
            let mut meter = self.ui.widget(cx, path);
            let warn = theme.yellow;
            let crit = theme.red;
            script_apply_eval!(cx, meter, {
                bar_color: #(bar_color)
                warn_color: #(warn)
                crit_color: #(crit)
                track_color: #(surface)
                text_color: #(foreground)
                muted_color: #(muted)
                draw_text +: {color: #(foreground)}
            });
        }
        if let Some(mut table) = self.ui.widget(cx, ids!(process_table)).borrow_mut::<ProcessTable>() {
            table.apply_theme(cx, theme);
        }
        self.ui.redraw(cx);
    }

    fn apply_chart_theme(&self, cx: &mut Cx, path: &[LiveId], series: Vec4f) {
        let mut chart = self.ui.trend_chart(cx, path);
        let background = self.theme.surface;
        let grid = with_alpha(self.theme.muted, 0.35);
        let text = self.theme.muted;
        // Transparent: TrendChart's area pass paints nothing, leaving the bare
        // line the reference asks for.
        let fill = with_alpha(series, 0.0);
        script_apply_eval!(cx, chart, {
            color_bg: #(background)
            color_grid: #(grid)
            color_line: #(series)
            color_fill: #(fill)
            color_text: #(text)
            color_accent: #(series)
        });
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(choice) = self.ui.drop_down(cx, ids!(refresh_select)).changed(actions) {
            let millis = REFRESH_CHOICES_MS.get(choice).copied().unwrap_or(1000);
            self.interval_ms.store(millis, Ordering::Relaxed);
            log!("mptask: refresh interval now {millis} ms");
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        // The mp* WM palette retints the stock widgets before anything is built.
        mp_theme::apply(vm);
        // MeterBars/ProcessTable must exist in mod.widgets before the UI below
        // does `use mod.widgets.*`.
        crate::widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Startup = event {
            self.theme = Theme::from_environment();
            self.apply_theme(cx);
            // `--size WxH` lets a test drive the breakpoints without a WM.
            if let Some(size) = size_from_args() {
                self.ui.window(cx, ids!(main_window)).resize(cx, size);
            }
            self.layout_from_window(cx);
            self.start_sampler();
        }
        // The window is often an mpwm tile, so the layout follows its size
        // rather than assuming a desktop-sized window.
        if let Event::WindowGeomChange(geom) = event {
            self.apply_layout(cx, geom.new_geom.inner_size);
        }
        if let Event::Signal = event {
            self.drain_snapshots(cx);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

/// `--size 520x360` — resize at startup so the breakpoints can be driven from
/// a script (there is no resize verb on the remote surface).
fn size_from_args() -> Option<Vec2d> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        let value = match arg.strip_prefix("--size=") {
            Some(value) => value.to_string(),
            None if arg == "--size" => args.next()?,
            None => continue,
        };
        let (width, height) = value.split_once(['x', 'X'])?;
        return Some(dvec2(width.trim().parse().ok()?, height.trim().parse().ok()?));
    }
    None
}

fn push_history(history: &mut Vec<f64>, value: f64) {
    history.push(value);
    if history.len() > HISTORY {
        history.remove(0);
    }
}

/// Binary units, because that is what a process manager's RSS is measured in.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;
    if days > 0 {
        format!("{days}d {hours:02}h {minutes:02}m")
    } else {
        format!("{hours:02}h {minutes:02}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_units_are_compact() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.0 MiB");
        assert_eq!(format_bytes(64 * 1024 * 1024 * 1024), "64.0 GiB");
    }

    #[test]
    fn uptime_reads_as_days_hours_minutes() {
        assert_eq!(format_uptime(0), "00h 00m");
        assert_eq!(format_uptime(3 * 3600 + 25 * 60), "03h 25m");
        assert_eq!(format_uptime(2 * 86400 + 3600 + 60), "2d 01h 01m");
    }

    #[test]
    fn history_is_capped_at_sixty_seconds() {
        let mut history = Vec::new();
        for value in 0..75 {
            push_history(&mut history, value as f64);
        }
        assert_eq!(history.len(), HISTORY);
        assert_eq!(history[0], 15.0);
        assert_eq!(history[HISTORY - 1], 74.0);
    }

    #[test]
    fn colors_parse_in_every_hex_length() {
        assert_eq!(parse_color("#fff"), parse_color("#ffffff"));
        assert_eq!(parse_color("#7aa2f7"), parse_color("7aa2f7"));
        assert_eq!(parse_color("#00000000").map(|c| c.w), Some(0.0));
        assert!(parse_color("#zz").is_none());
    }

    #[test]
    fn theme_falls_back_when_no_wm_palette_is_exported() {
        // Nothing may panic when MPWM_THEME_SPLASH is unset or bogus.
        let theme = Theme::from_environment();
        assert!(theme.accent.w > 0.0);
    }
}
