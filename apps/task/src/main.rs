//! task — the task manager / activity monitor of the Makepad app family.
//!
//! One compact toolbar, a shallow history band with a scrub cursor, a strip
//! of system traces, pinned processes, the process table and an inspector.
//! Everything is recorded: the band can be dragged back to any retained
//! moment and every view — tiles, table, inspector — then shows that same
//! sample while the worker keeps recording. History survives restarts in a
//! bounded journal in the user's application-data directory.
//!
//! All numbers come from [`backend`], one trait with a native
//! implementation per OS — never `ps`/`top` output.

pub use makepad_widgets;

use makepad_widgets::*;
use std::collections::HashMap;

mod backend;
mod clock;
mod columns;
mod history;
mod metrics;
mod model;
mod persist;
mod sampler;
mod supp;
mod widgets;

use backend::ProcKey;
use model::{Model, RANGES_MS};
use sampler::{Command, Message, Worker};
use widgets::{HistoryBand, InspectorBody, InspectorTab, ProcessTable, SeriesLegend, INSPECTOR_TABS};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // One control height (26) and one radius for every bar control; colours
    // come from the app theme at runtime (App::style_toggle / apply_theme).
    let ToolButton = ButtonFlat{
        width: Fit
        height: 26
        padding: Inset{left: 10 right: 10 top: 0 bottom: 0}
        draw_text +: {text_style: theme.font_regular{font_size: 10.5}}
    }

    let IconButton = ButtonFlat{
        width: 26
        height: 26
        padding: 0
        align: Align{x: 0.5 y: 0.5}
        draw_text +: {text_style: theme.font_icons{font_size: 9.0}}
    }

    let ToolDrop = DropDown{
        width: 76
        height: 26
        padding: Inset{left: 10 right: 20 top: 0 bottom: 0}
        draw_text +: {text_style: theme.font_regular{font_size: 10.5}}
    }

    let Segments = SegmentedControl{
        segment_height: 26
        segment_padding: 12
        draw_text +: {text_style: theme.font_regular{font_size: 10.5}}
        draw_text_selected +: {text_style: theme.font_bold{font_size: 10.5}}
    }

    let Rule = View{width: Fill height: 1 show_bg: true draw_bg +: {color: #x2a2b2f}}
    let ToolGap = View{width: 1 height: 18 show_bg: true margin: Inset{left: 4 right: 4} draw_bg +: {color: #x2a2b2f}}

    let TabButton = ButtonFlat{
        width: Fit
        height: 30
        padding: Inset{left: 12 right: 12 top: 0 bottom: 0}
        draw_text +: {text_style: theme.font_regular{font_size: 10.5}}
    }

    let SmallText = Label{
        padding: 0
        draw_text +: {color: #x9b9ea6 text_style: theme.font_regular{font_size: 10.0}}
    }

    let MonoText = Label{
        padding: 0
        draw_text +: {color: #xe3e4e6 text_style: theme.font_code{font_size: 10.5}}
    }

    // A quiet 1 px divider with a wider grab strip; hover and drag light it.
    let QuietSplitter = Splitter{
        size: 7.0
        draw_bg +: {
            color_bg: uniform(#x1b1c1f)
            color: uniform(#x2a2b2f)
            color_hover: uniform(#x4f9dff)
            color_drag: uniform(#x4f9dff)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.clear(self.color_bg)
                let emphasis = max(self.hover, self.drag)
                let thickness = mix(1.0, 2.0, emphasis)
                if self.is_vertical > 0.5 {
                    sdf.rect((self.rect_size.x - thickness) * 0.5, 0.0, thickness, self.rect_size.y)
                }
                else {
                    sdf.rect(0.0, (self.rect_size.y - thickness) * 0.5, self.rect_size.x, thickness)
                }
                return sdf.fill(mix(self.color, mix(self.color_hover, self.color_drag, self.drag), emphasis * 0.7))
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Task Manager"
                window.inner_size: vec2(1400 900)
                // Overlay: the menu layer after the app covers the whole window
                // from its corner, where menus are placed from.
                body +: {
                    flow: Overlay
                    app_bg := RectView{
                        width: Fill
                        height: Fill
                        flow: Down
                        padding: 0
                        spacing: 0
                        draw_bg +: {color: theme.color_bg_app}

                        // What to show: scope, list/tree, order and sampling on
                        // the left; search and the inspector on the right.
                        toolbar := RectView{
                            width: Fill
                            height: Fit
                            flow: Down
                            draw_bg +: {color: theme.color_bg_app}
                            tool_row := View{
                                width: Fill
                                height: 44
                                flow: Right
                                spacing: 8
                                align: Align{y: 0.5}
                                padding: Inset{left: 12 right: 12}
                                scope_seg := Segments{options: ["Processes" "Apps"]}
                                view_seg := Segments{options: ["List" "Tree"]}
                                freeze_toggle := ToolButton{text: "Freeze order"}
                                // DropDown has no visibility of its own: a View
                                // around it is what hides it.
                                refresh_wrap := View{
                                    width: Fit height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
                                    refresh_label := SmallText{text: "Every"}
                                    refresh_select := ToolDrop{
                                        labels: ["0.1 s" "0.2 s" "0.5 s" "1 s" "2 s" "5 s" "10 s"]
                                        selected_item: 0
                                    }
                                }
                                // How much history the graphs show: separate
                                // from how often a sample is taken.
                                history_wrap := View{
                                    width: Fit height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
                                    history_label := SmallText{text: "History"}
                                    range_select := ToolDrop{
                                        width: 70
                                        labels: ["60 s" "15 m" "1 h" "24 h"]
                                        selected_item: 1
                                    }
                                }
                                View{width: Fill height: 1}
                                filter_input := TextInput{
                                    width: 260
                                    height: 26
                                    empty_text: "Search processes"
                                    padding: Inset{left: 10 right: 10 top: 4 bottom: 4}
                                    draw_text +: {text_style: theme.font_regular{font_size: 10.5}}
                                }
                                inspector_toggle := ToolButton{text: "Inspector"}
                            }
                            // Narrow windows: what did not fit the first row.
                            tool_row_more := View{
                                visible: false
                                width: Fill
                                height: 38
                                flow: Right
                                spacing: 8
                                align: Align{y: 0.5}
                                padding: Inset{left: 12 right: 12 bottom: 6}
                                freeze_toggle_more := ToolButton{text: "Freeze order"}
                                refresh_label_more := SmallText{text: "Every"}
                                refresh_wrap_more := View{
                                    width: Fit height: Fit
                                    refresh_select_more := ToolDrop{
                                        labels: ["0.1 s" "0.2 s" "0.5 s" "1 s" "2 s" "5 s" "10 s"]
                                        selected_item: 0
                                    }
                                }
                                history_label_more := SmallText{text: "History"}
                                history_wrap_more := View{
                                    width: Fit height: Fit
                                    range_select_more := ToolDrop{
                                        width: 70
                                        labels: ["60 s" "15 m" "1 h" "24 h"]
                                        selected_item: 1
                                    }
                                }
                                View{width: Fill height: 1}
                                inspector_toggle_more := ToolButton{text: "Inspector"}
                            }
                        }
                        rule_toolbar := Rule{}
                        // Series chips on the left; stepping through recorded
                        // samples, the shown time and the one Live control on
                        // the right, on the same line.
                        legend_row := View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 8
                            align: Align{y: 0.0}
                            padding: Inset{left: 12 right: 12 top: 6 bottom: 4}
                            series_legend := SeriesLegend{}
                            history_controls := View{
                                width: Fit
                                height: 26
                                flow: Right
                                spacing: 4
                                align: Align{y: 0.5}
                                step_back := IconButton{text: "\u{f053}"}
                                time_label := MonoText{
                                    padding: Inset{left: 4 right: 4}
                                    text: "Live"
                                }
                                step_forward := IconButton{text: "\u{f054}"}
                                live_button := ToolButton{text: "Live"}
                            }
                        }
                        history_band := HistoryBand{}
                        rule_band := Rule{}
                        // Plain Views own the visibility: the custom strips
                        // have no `visible` property of their own.
                        metric_wrap := View{width: Fill height: Fit metric_strip := MetricStrip{}}
                        rule_metrics := Rule{}
                        pinned_wrap := View{visible: false width: Fill height: Fit pinned_strip := PinnedStrip{}}
                        rule_pinned := Rule{visible: false}
                        body_split := QuietSplitter{
                            width: Fill
                            height: Fill
                            axis: SplitterAxis.Vertical
                            align: SplitterAlign.FromB(250.0)
                            min_horizontal: 80.0
                            max_horizontal: 80.0
                            a: View{width: Fill height: Fill process_table := ProcessTable{}}
                            b: View{
                                width: Fill
                                height: Fill
                                flow: Down
                                // Tabs on the raised bar; the active one takes
                                // the content colour so it reads as part of the
                                // panel below. End process stands apart.
                                inspector_tabs := RectView{
                                    width: Fill
                                    height: 36
                                    flow: Right
                                    spacing: 2
                                    align: Align{y: 1.0}
                                    padding: Inset{left: 8 right: 10 top: 0 bottom: 0}
                                    draw_bg +: {color: theme.color_bg_app}
                                    // The eight tabs scroll sideways when the
                                    // panel is narrower than they are (a thin
                                    // bar shows it); the actions stay put.
                                    tab_scroll := ScrollXView{
                                        width: Fill
                                        height: 36
                                        flow: Right
                                        spacing: 2
                                        align: Align{y: 1.0}
                                        scroll_bars +: {scroll_bar_x +: {bar_size: 3.0}}
                                        tab_info := TabButton{text: "Info"}
                                        tab_activity := TabButton{text: "Activity"}
                                        tab_history := TabButton{text: "History"}
                                        tab_threads := TabButton{text: "Threads"}
                                        tab_memory := TabButton{text: "Memory"}
                                        tab_files := TabButton{text: "Files"}
                                        tab_ports := TabButton{text: "Ports"}
                                        tab_libraries := TabButton{text: "Libraries"}
                                    }
                                    View{width: 1 height: 20 margin: Inset{left: 6 right: 6 bottom: 8} show_bg: true draw_bg +: {color: #x2a2b2f}}
                                    tab_actions := View{
                                        width: Fit height: 36 flow: Right spacing: 8 align: Align{y: 0.5}
                                        kill_button := ToolButton{text: "End process"}
                                        close_inspector := IconButton{text: "\u{f00d}"}
                                    }
                                }
                                inspector_body := InspectorBody{}
                            }
                        }
                        rule_status := Rule{}
                        status_bar := RectView{
                            width: Fill
                            height: 26
                            flow: Right
                            spacing: 12
                            align: Align{y: 0.5}
                            padding: Inset{left: 12 right: 12}
                            draw_bg +: {color: theme.color_bg_app}
                            status_left := SmallText{width: Fill text: "Waiting for the first sample"}
                            status_right := SmallText{text: ""}
                        }
                    }
                    // Every menu the app raises (the column chooser) draws here,
                    // over everything.
                    menus := MenuLayer{}
                }
            }
        }
    }
}

// ---- theme ----

/// The palette the whole app paints with. Sourced from the Makepad WM theme
/// (`MAKEPAD_WM_THEME_SPLASH`) through `makepad_wm_theme::current()`, so task matches
/// terminal/files/wm; the fallback is a neutral charcoal.
///
/// The app draws with three surface roles derived from the palette's
/// background and foreground, so any WM theme gets the same structure:
/// `background` for rows and content, [`Theme::raised`] for bars, headers
/// and summaries, [`Theme::well`] for plots and grouped sections. Colour is
/// kept for data and the accent.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub accent: Vec4f,
    /// Window, rows and content.
    pub background: Vec4f,
    pub foreground: Vec4f,
    /// Secondary text.
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
            accent: color("#4f9dff"),
            background: color("#1b1c1f"),
            foreground: color("#e3e4e6"),
            muted: color("#9b9ea6"),
            red: color("#f0657b"),
            green: color("#5fd3a6"),
            yellow: color("#f0b64b"),
            blue: color("#5aa9ff"),
            cyan: color("#56c2e6"),
            magenta: color("#b48cf2"),
        }
    }
}

impl Theme {
    /// Bars, headers and summaries: a step off the background.
    pub fn raised(&self) -> Vec4f {
        mix(self.background, self.foreground, 0.035)
    }

    /// Plots and grouped sections: a further step.
    pub fn well(&self) -> Vec4f {
        mix(self.background, self.foreground, 0.065)
    }

    /// Tiles that must stand out on a well (the kind glyph).
    pub fn well_strong(&self) -> Vec4f {
        mix(self.background, self.foreground, 0.13)
    }

    /// Quiet 1 px separators.
    pub fn rule(&self) -> Vec4f {
        with_alpha(self.foreground, 0.09)
    }

    pub fn secondary(&self) -> Vec4f {
        self.muted
    }

    /// Notes, axis labels and other text that should recede.
    pub fn tertiary(&self) -> Vec4f {
        with_alpha(self.muted, 0.72)
    }

    /// Secondary text a step closer to the foreground: a palette's dim
    /// foreground is meant for large type and goes faint at 10 pt.
    fn legible(mut self) -> Self {
        self.muted = mix(self.muted, self.foreground, 0.3);
        self
    }

    /// The WM palette if one is exported, else the built-in fallback. Uses the
    /// same scanner `makepad_wm_theme::apply` retints `mod.theme` with, so the stock
    /// widgets and task's own drawing can never disagree.
    #[cfg(test)]
    fn from_environment() -> Self { Self::from_palette(makepad_wm_theme::current()) }
    fn from_palette(palette: Option<makepad_wm_theme::Palette>) -> Self {
        let fallback = Self::default();
        let Some(palette) = palette else { return fallback };
        let pick = |key: &str, default: Vec4f| palette.get(key).and_then(parse_color).unwrap_or(default);
        // Imported omarchy themes carry their hues as the terminal palette;
        // take those when a theme does not name the colours directly.
        let light = pick("background", fallback.background).x > 0.5;
        let hue = |key: &str, term: &str, default: Vec4f| {
            let default = if light {
                color(match key {
                    "red" => "#d12b3a", "green" => "#39851e", "yellow" => "#986b00",
                    "blue" => "#246bce", "cyan" => "#008398", "magenta" => "#8951bd",
                    _ => "#246bce",
                })
            } else { default };
            palette
                .get(key)
                .or_else(|| palette.get(term))
                .and_then(parse_color)
                .unwrap_or(default)
        };
        Self {
            accent: pick("accent", fallback.accent),
            background: pick("background", fallback.background),
            foreground: pick("foreground", fallback.foreground),
            muted: pick("dark_foreground", fallback.muted),
            red: hue("red", "term.color1", fallback.red),
            green: hue("green", "term.color2", fallback.green),
            yellow: hue("yellow", "term.color3", fallback.yellow),
            blue: hue("blue", "term.color4", fallback.blue),
            cyan: hue("cyan", "term.color6", fallback.cyan),
            magenta: hue("magenta", "term.color5", fallback.magenta),
        }
        .legible()
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

pub fn with_alpha(mut value: Vec4f, alpha: f32) -> Vec4f {
    value.w = alpha;
    value
}

pub fn mix(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    vec4(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t, a.w + (b.w - a.w) * t)
}

// ---- app ----

/// Sample-collection costs the status line averages. The window is a sample
/// count, not a wall-clock span: the last six seconds at the default
/// 0.1 s interval, or the last minute at 1 s.
const HISTORY: usize = 60;

/// The refresh-rate picker, in the order the drop-down lists them.
const REFRESH_CHOICES_MS: [u64; 7] = [100, 200, 500, 1000, 2000, 5000, 10_000];
/// Index of the default (0.1 s) — must match `selected_item` in the DSL.
const DEFAULT_REFRESH: usize = 0;

/// How long after a SIGTERM the next End press escalates to SIGKILL.
const FORCE_WINDOW_SECS: f64 = 5.0;

/// How the window is laid out at the current size.
///
/// The thresholds are in layout points and are compared against the window's
/// *inner* size, so the app behaves the same whether it is a free window or an
/// wm tile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Density {
    /// Band, metric strip, pins, table and inspector.
    #[default]
    Full,
    /// The same with a shallower band and fewer columns and tiles.
    Medium,
    /// The history band alone, filling the window.
    Small,
    /// Phone portrait or a short landscape tile: band and table.
    Phone,
}

impl Density {
    pub fn for_size(size: Vec2d) -> Self {
        if size.y < 260.0 {
            Density::Small
        } else if size.x < 620.0 || size.y < 460.0 {
            Density::Phone
        } else if size.x < 1180.0 || size.y < 780.0 {
            Density::Medium
        } else {
            Density::Full
        }
    }
}

/// The warm-pool dormancy state machine (see `makepad_wm_api::warm_start` /
/// `WmEvent::Adopted`). wm pre-spawns hidden warm instances of this app
/// (`MAKEPAD_WM_WARM_START=1`); a cached task manager must not burn CPU sampling
/// in the background before anyone is looking at it. A warm instance starts
/// `Dormant` — no sampler thread, no journal, empty graphs — and wakes
/// exactly once: either wm adopts it into a real tile (`WmEvent::Adopted`
/// on the studio `Custom` channel), or, defensively, a human touches the
/// window directly (a key or pointer/touch event, in case an `Adopted`
/// message is ever lost). A non-warm instance is never dormant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dormancy {
    /// Not a warm-pool instance: sampling starts immediately.
    #[default]
    Active,
    /// A warm-pool instance, still idling.
    Dormant,
    /// A warm-pool instance that has woken up.
    Woken,
}

impl Dormancy {
    /// `warm` is `makepad_wm_api::warm_start()`, read once at startup.
    pub fn start(warm: bool) -> Self {
        if warm { Dormancy::Dormant } else { Dormancy::Active }
    }

    pub fn is_dormant(&self) -> bool {
        *self == Dormancy::Dormant
    }

    /// Transition `Dormant` -> `Woken`. Returns `true` the one time this
    /// actually wakes it (the caller should start the sampler then); `false`
    /// when it was already active or already woken, so Adopted arriving
    /// after an input wake (or twice) never restarts anything.
    pub fn wake(&mut self) -> bool {
        if *self == Dormancy::Dormant {
            *self = Dormancy::Woken;
            true
        } else {
            false
        }
    }
}

/// A raw input event a human — not the WM protocol — could only have sent:
/// the defensive wake path for a lost `WmEvent::Adopted`.
fn is_wake_input(event: &Event) -> bool {
    matches!(
        event,
        Event::KeyDown(_) | Event::MouseDown(_) | Event::TouchUpdate(_)
    )
}

impl Default for Model {
    fn default() -> Self {
        Model::new(Theme::default(), REFRESH_CHOICES_MS[DEFAULT_REFRESH])
    }
}

/// A process that was just sent SIGTERM: End again inside
/// [`FORCE_WINDOW_SECS`] escalates to SIGKILL.
#[derive(Clone, Copy, Debug)]
struct Armed {
    key: ProcKey,
    since: f64,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    model: Model,
    #[rust]
    worker: Option<Worker>,
    /// Warm-pool dormancy — see `Dormancy`.
    #[rust]
    dormancy: Dormancy,
    #[rust]
    density: Density,
    #[rust]
    density_applied: bool,
    #[rust]
    layout_size: Vec2d,
    #[rust]
    budget_set: bool,
    /// Per-sample collection cost (ms), the last [`HISTORY`] samples.
    #[rust]
    sample_costs: Vec<f64>,
    #[rust]
    armed: Option<Armed>,
    /// Control looks last written, so a script eval runs only on change.
    #[rust]
    styled: HashMap<LiveId, Look>,
    /// (apps only, tree) last pushed to the segmented controls.
    #[rust]
    segments: Option<(bool, bool)>,
    #[rust]
    visible: HashMap<LiveId, bool>,
    #[rust]
    kill_state: Option<(bool, bool)>,
    /// The inspector fold last applied to the body splitter.
    #[rust]
    inspector_applied: Option<SplitterCollapse>,
    /// Drives the live graphs between samples — see `animate`.
    #[rust]
    motion: Motion,
}

/// The live edge of the graphs moves with the UI's monotonic clock, anchored
/// to the samples' own timestamps, so every trace glides left instead of
/// stepping once per sample. Only translation happens per frame: the folded
/// geometry is cached by store generation in each widget.
#[derive(Default)]
struct Motion {
    next_frame: NextFrame,
    requested: bool,
    /// Sample time minus UI clock (ms) — smoothed, snapped on a jump.
    offset_ms: Option<f64>,
    /// Live edge at the last redraw of the band, the strips and the table.
    band_end: f64,
    strips_end: f64,
    table_end: f64,
    inspector_end: f64,
}

impl Motion {
    /// A sample arrived: its timestamp says where "now" is on the data's
    /// clock. Small disagreements are eased in so the edge never lurches.
    fn anchor(&mut self, sample_ms: f64, clock_ms: f64) {
        let offset = sample_ms - clock_ms;
        self.offset_ms = Some(match self.offset_ms {
            Some(previous) if (offset - previous).abs() < 1000.0 => previous + (offset - previous) * 0.2,
            _ => offset,
        });
    }
}

impl App {
    /// Starts the sampler worker, unless a warm-pool instance is still
    /// dormant — a cached task manager must not sample (or touch its journal)
    /// while nobody is looking at it.
    fn start_sampler(&mut self, cx: &mut Cx) {
        if self.worker.is_some() || self.dormancy.is_dormant() {
            return;
        }
        match Worker::spawn(&cx.thread_spawner(), self.model.interval_ms) {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => log!("task: could not start sampler worker: {error}"),
        }
    }

    /// Wakes a dormant warm instance: `WmEvent::Adopted`, or defensively the
    /// first real key/pointer input in case that message was lost.
    fn wake(&mut self, cx: &mut Cx) {
        if self.dormancy.wake() {
            log!("task: warm instance woken, starting sampler");
            self.start_sampler(cx);
        }
    }

    fn send(&mut self, command: Command) {
        if let Some(worker) = &mut self.worker {
            if !worker.send(command) {
                self.model.notice = Some("the sampler is busy; try again".to_string());
                self.model.requests.redraw = true;
            }
        }
    }

    /// Take what the worker published. Never waits.
    fn poll_worker(&mut self, cx: &mut Cx) {
        let Some(worker) = &mut self.worker else { return };
        let messages = worker.poll();
        if messages.is_empty() {
            return;
        }
        let model = &mut self.model;
        let now = backend::now_ms();
        for message in messages {
            match message {
                Message::Sample(sample) => {
                    self.motion.anchor(sample.time_ms as f64, cx.seconds_since_app_start() * 1000.0);
                    model.remember_pins(&sample);
                    if !self.budget_set {
                        self.budget_set = true;
                        model.store.set_budget_from_ram(sample.system.mem_total);
                        model.supp.set_budget(supp::budget_for_ram(sample.system.mem_total));
                    }
                    push_history(&mut self.sample_costs, sample.cost_us as f64 / 1000.0);
                    model.store.push(sample);
                    model.store.thin(now, false);
                    if model.live {
                        if let Some(key) = model.selected {
                            if let Some(meta) = model.store.latest().and_then(|s| s.process(key)).map(|r| r.meta.clone()) {
                                model.selected_meta = Some(meta);
                            }
                        }
                    }
                    model.changed();
                }
                Message::Loaded(batch) => {
                    for sample in &batch {
                        model.remember_pins(sample);
                    }
                    model.store.ingest_loaded(batch, now);
                    model.changed();
                }
                Message::Pins(pins) => {
                    for pin in pins {
                        if !model.is_pinned(pin.key) {
                            model.pins.push(pin);
                        }
                    }
                    model.changed();
                }
                Message::Columns(text) => {
                    if model.columns.parse(&text) {
                        model.changed();
                    }
                }
                Message::Detail(detail) => {
                    model.details.push(detail, now);
                    model.requests.redraw = true;
                }
                // Not a `changed()`: the process table's order does not
                // depend on it; the inspector's caches key on the store's
                // generation.
                Message::Supp(events) => {
                    model.supp.apply(events, now);
                    model.requests.redraw = true;
                }
                Message::Terminated { key, force, result } => {
                    let pid = key.pid;
                    model.notice = Some(match result {
                        Ok(()) if force || cfg!(windows) => {
                            self.armed = None;
                            if cfg!(windows) { format!("PID {pid} terminated") } else { format!("SIGKILL sent to PID {pid}") }
                        }
                        Ok(()) => {
                            self.armed = Some(Armed { key, since: Cx::monotonic_now() });
                            format!("SIGTERM sent to PID {pid} · End again within 5 s to force (SIGKILL)")
                        }
                        Err(error) => {
                            self.armed = None;
                            format!("could not signal PID {pid}: {error}")
                        }
                    });
                    model.requests.redraw = true;
                }
                Message::Status(status) => {
                    log!("task: {status}");
                    model.journal_status = status;
                    model.requests.redraw = true;
                }
            }
        }
        let _ = cx;
    }

    /// End the selected process: SIGTERM first, SIGKILL on a second press
    /// within [`FORCE_WINDOW_SECS`] or with shift. The worker re-verifies the
    /// identity right before signalling.
    fn terminate(&mut self, force_requested: bool) {
        if let Some(reason) = self.model.terminate_refusal() {
            self.model.notice = Some(reason);
            return;
        }
        let Some(key) = self.model.selected else { return };
        let force = force_requested
            || self.armed.is_some_and(|armed| armed.key == key && Cx::monotonic_now() - armed.since <= FORCE_WINDOW_SECS);
        self.model.notice = Some(format!("signalling PID {}…", key.pid));
        self.send(Command::Terminate { key, force });
    }

    fn force_armed(&self) -> bool {
        match (self.armed, self.model.selected) {
            (Some(armed), Some(key)) => armed.key == key && Cx::monotonic_now() - armed.since <= FORCE_WINDOW_SECS,
            _ => false,
        }
    }

    /// Act on what widgets asked for during this event.
    fn after_event(&mut self, cx: &mut Cx) {
        let requests = std::mem::take(&mut self.model.requests);
        if requests.inspect {
            let key = if self.model.inspector_open { self.model.selected } else { None };
            if self.armed.is_some_and(|armed| Some(armed.key) != self.model.selected) {
                self.armed = None;
            }
            self.send(Command::Inspect(key));
        }
        if requests.pins {
            let pins = self.model.pins.clone();
            self.send(Command::SavePins(pins));
        }
        if requests.columns {
            let text = self.model.columns.serialize();
            self.send(Command::SaveColumns(text));
        }
        if let Some(force) = requests.terminate {
            self.terminate(force);
        }
        // Selection may have opened the inspector on its own.
        self.apply_inspector(cx);
        self.request_motion(cx);
        if requests.redraw || requests.terminate.is_some() || requests.pins {
            self.update_chrome(cx);
            self.ui.redraw(cx);
        }
    }

    /// Frames run only while there is something moving: live, sampling,
    /// adopted and with data. Scrubbing or going dormant stops them.
    fn moving(&self) -> bool {
        self.model.live && self.worker.is_some() && !self.dormancy.is_dormant() && self.motion.offset_ms.is_some() && !self.model.store.is_empty()
    }

    fn request_motion(&mut self, cx: &mut Cx) {
        if !self.motion.requested && self.moving() {
            self.motion.requested = true;
            self.motion.next_frame = cx.new_next_frame();
        }
    }

    /// Advances the live edge to the presentation clock and redraws the
    /// graphs — never the rest of the window, and never a table rebuild
    /// (rows rebuild only on a model version change). The edge trails the
    /// clock by one interval, so the newest sample is reached as the next
    /// one arrives; a stalled sampler shows as the real gap it is.
    fn animate(&mut self, cx: &mut Cx) {
        if !self.moving() {
            return;
        }
        let Some(offset) = self.motion.offset_ms else { return };
        let lag = (self.model.interval_ms as f64).clamp(100.0, 2000.0);
        let target = cx.seconds_since_app_start() * 1000.0 + offset - lag;
        let previous = self.model.live_end_ms;
        // Monotonic, except after a clock jump (sleep, clock change).
        let end = if previous > 0.0 && target < previous && previous - target < 1000.0 { previous } else { target };
        self.model.live_end_ms = end;
        let width = self.layout_size.x.max(1.0);
        // Redraw once the band would move by a quarter pixel: every frame at
        // 60 s, a few times a minute at 24 h.
        let band_step = self.model.range_ms() as f64 / width * 0.25;
        if (end - self.motion.band_end).abs() >= band_step {
            self.motion.band_end = end;
            self.ui.widget(cx, ids!(history_band)).redraw(cx);
        }
        // The 60 s strips are ~100–300 px wide: 30 Hz is under a pixel a step.
        if (end - self.motion.strips_end).abs() >= 33.0 {
            self.motion.strips_end = end;
            self.ui.widget(cx, ids!(metric_strip)).redraw(cx);
            self.ui.widget(cx, ids!(pinned_strip)).redraw(cx);
        }
        // The table's sparklines are 110 px over 60 s; 15 Hz keeps its
        // redraw cost bounded with hundreds of rows.
        if (end - self.motion.table_end).abs() >= 66.0 {
            self.motion.table_end = end;
            self.ui.widget(cx, ids!(process_table)).redraw(cx);
        }
        // The inspector's graphs share the band's window (Activity's
        // sparklines a minute): redraw only while one is showing, at most
        // 20 Hz and once it would move a quarter pixel. Its rows and points
        // are cached, so this is drawing only.
        if self.model.inspector_open && self.model.selected.is_some() {
            let tab = self.ui.widget(cx, ids!(inspector_body)).borrow::<InspectorBody>().map(|body| body.tab).unwrap_or_default();
            let span = match tab {
                InspectorTab::Activity => Some(history::MINUTE_MS as f64),
                InspectorTab::History | InspectorTab::Threads | InspectorTab::Memory => Some(self.model.range_ms() as f64),
                _ => None,
            };
            if let Some(span) = span {
                let step = (span / width * 0.25).max(50.0);
                if (end - self.motion.inspector_end).abs() >= step {
                    self.motion.inspector_end = end;
                    self.ui.widget(cx, ids!(inspector_body)).redraw(cx);
                }
            }
        }
        self.request_motion(cx);
    }

    fn set_visible(&mut self, cx: &mut Cx, id: LiveId, path: &[LiveId], visible: bool) {
        if self.visible.get(&id) != Some(&visible) {
            self.visible.insert(id, visible);
            self.ui.widget(cx, path).set_visible(cx, visible);
        }
    }

    /// Give a button one of the app's looks — only when that changed.
    fn style_button(&mut self, cx: &mut Cx, id: LiveId, path: &[LiveId], look: Look) {
        if self.styled.get(&id) == Some(&look) {
            return;
        }
        self.styled.insert(id, look);
        let t = self.model.theme;
        let clear = with_alpha(t.background, 0.0);
        let flat = vec4(-1.0, -1.0, -1.0, -1.0);
        let (fill, hover, down, ink, border, border_size) = match look {
            Look::Plain => (clear, with_alpha(t.foreground, 0.07), with_alpha(t.foreground, 0.12), t.foreground, clear, 0.0),
            Look::On => (with_alpha(t.accent, 0.18), with_alpha(t.accent, 0.26), with_alpha(t.accent, 0.32), t.accent, clear, 0.0),
            Look::TabActive => (t.background, t.background, t.background, t.foreground, clear, 0.0),
            Look::TabIdle => (clear, with_alpha(t.foreground, 0.06), with_alpha(t.foreground, 0.10), t.secondary(), clear, 0.0),
            Look::Danger => (clear, with_alpha(t.red, 0.14), with_alpha(t.red, 0.22), t.red, with_alpha(t.red, 0.55), 1.0),
            Look::DangerOff => (clear, clear, clear, t.tertiary(), t.rule(), 1.0),
        };
        let muted = t.tertiary();
        let tab = matches!(look, Look::TabActive | Look::TabIdle);
        let bottom_radius = if tab { 0.0 } else { 5.0 };
        let mut button = self.ui.button(cx, path);
        script_apply_eval!(cx, button, {
            draw_bg +: {
                color: #(fill)
                color_hover: #(hover)
                color_down: #(down)
                color_focus: #(fill)
                color_2_hover: #(flat)
                color_2_down: #(flat)
                color_2_focus: #(flat)
                border_size: #(border_size)
                border_color: #(border)
                border_color_hover: #(border)
                border_color_down: #(border)
                border_color_focus: #(border)
                border_radius: 5.0
                border_radius_bl: #(bottom_radius)
                border_radius_br: #(bottom_radius)
            }
            draw_text +: {color: #(ink) color_hover: #(ink) color_down: #(ink) color_focus: #(ink) color_disabled: #(muted)}
        });
    }

    /// The history readout, control states, End button and status line.
    fn update_chrome(&mut self, cx: &mut Cx) {
        let live = self.model.live;
        let wide = self.layout_size.x >= 1180.0;
        let time_text = if live {
            match self.model.store.newest_ms() {
                Some(ms) => {
                    let time = clock::LocalTime::from_epoch_ms(ms);
                    format!("{}{}", time.hms(), if time.zoned { "" } else { " UTC" })
                }
                None => "—".to_string(),
            }
        } else {
            // The date only where there is room; HH:MM:SS otherwise.
            let time = clock::LocalTime::from_epoch_ms(self.model.cursor_ms);
            let when = if wide {
                time.date_hms()
            } else if time.zoned {
                time.hms()
            } else {
                format!("{} UTC", time.hms())
            };
            let tier = self.model.view_tier();
            if self.model.cursor_expired() {
                format!("{when} · aged out of retention")
            } else if tier == history::Tier::Raw || !wide {
                when
            } else {
                format!("{when} · {}", tier.label())
            }
        };
        self.ui.label(cx, ids!(time_label)).set_text(cx, &time_text);
        if self.styled.get(&live_id!(time_label)) != Some(&if live { Look::Plain } else { Look::On }) {
            self.styled.insert(live_id!(time_label), if live { Look::Plain } else { Look::On });
            let ink = if live { self.model.theme.secondary() } else { self.model.theme.accent };
            let mut label = self.ui.label(cx, ids!(time_label));
            script_apply_eval!(cx, label, {draw_text +: {color: #(ink)}});
        }
        self.style_button(cx, live_id!(live_button), ids!(live_button), if live { Look::On } else { Look::Plain });
        self.style_button(cx, live_id!(step_back), ids!(step_back), Look::Plain);
        self.style_button(cx, live_id!(step_forward), ids!(step_forward), Look::Plain);
        let freeze = if self.model.freeze { Look::On } else { Look::Plain };
        self.style_button(cx, live_id!(freeze_toggle), ids!(freeze_toggle), freeze);
        self.style_button(cx, live_id!(freeze_toggle_more), ids!(freeze_toggle_more), freeze);
        let inspector = if self.model.inspector_open { Look::On } else { Look::Plain };
        self.style_button(cx, live_id!(inspector_toggle), ids!(inspector_toggle), inspector);
        self.style_button(cx, live_id!(inspector_toggle_more), ids!(inspector_toggle_more), inspector);
        self.style_button(cx, live_id!(close_inspector), ids!(close_inspector), Look::Plain);
        let segments = (self.model.apps_only, self.model.tree);
        if self.segments != Some(segments) {
            self.segments = Some(segments);
            self.ui.segmented_control(cx, ids!(scope_seg)).set_selected(cx, segments.0 as usize);
            self.ui.segmented_control(cx, ids!(view_seg)).set_selected(cx, segments.1 as usize);
        }
        let tab = self.ui.widget(cx, ids!(inspector_body)).borrow::<InspectorBody>().map(|body| body.tab).unwrap_or_default();
        for (index, (id, path)) in TAB_IDS.iter().enumerate() {
            self.style_button(cx, *id, path, if INSPECTOR_TABS[index] == tab { Look::TabActive } else { Look::TabIdle });
        }

        // The End button says what the next press does, is set apart from
        // the tabs in red, and goes quiet when there is nothing it may signal.
        let killable = self.model.can_terminate();
        let armed = self.force_armed();
        if self.kill_state != Some((killable, armed)) {
            self.kill_state = Some((killable, armed));
            let button = self.ui.button(cx, ids!(kill_button));
            button.set_text(cx, if armed && !cfg!(windows) { "Force stop" } else { "End process" });
            button.set_enabled(cx, killable);
        }
        self.style_button(cx, live_id!(kill_button), ids!(kill_button), if killable { Look::Danger } else { Look::DangerOff });

        let show_pins = !self.model.pins.is_empty() && self.density != Density::Small;
        self.set_visible(cx, live_id!(pinned_wrap), ids!(pinned_wrap), show_pins);
        self.set_visible(cx, live_id!(rule_pinned), ids!(rule_pinned), show_pins);

        // Status: what is shown on the left, the machine and the recording
        // on the right.
        let view = self.model.view_sample().cloned();
        let left = match (&self.model.notice, &view) {
            (Some(notice), _) => notice.clone(),
            (None, Some(sample)) => {
                let selected = self
                    .model
                    .selected
                    .and_then(|key| self.model.find_meta(key))
                    .map(|meta| format!(" · {} selected", meta.name))
                    .unwrap_or_default();
                format!("{} processes{selected}", sample.processes.len())
            }
            (None, None) => "Waiting for the first sample".to_string(),
        };
        self.ui.label(cx, ids!(status_left)).set_text(cx, &left);
        // Compact: the machine and the retained span. A journal problem is
        // named briefly; load/budget counters go to the log, not the chrome.
        let right = match &view {
            Some(sample) => {
                let retained = persist::format_span_ms(self.model.store.retained_ms());
                let average_cost = if self.sample_costs.is_empty() { 0.0 } else { self.sample_costs.iter().sum::<f64>() / self.sample_costs.len() as f64 };
                // Only worth a word when collection eats a real share of the interval.
                let cost = if average_cost > self.model.interval_ms as f64 * 0.25 {
                    format!(" · sampling takes {average_cost:.0} ms of every {}", interval_text(self.model.interval_ms))
                } else {
                    String::new()
                };
                let journal = if self.model.journal_status.contains("in memory only") { " · history not saved" } else { "" };
                format!(
                    "System {:.1}% · {} cores · up {} · {retained} retained{cost}{journal}",
                    sample.system.cpu_total,
                    sample.system.cores.len(),
                    format_uptime(sample.system.uptime_secs),
                )
            }
            None => String::new(),
        };
        self.ui.label(cx, ids!(status_right)).set_text(cx, &right);
    }

    /// Keyboard shortcuts that are not the focused widget's own.
    fn handle_key(&mut self, cx: &mut Cx, key: &KeyEvent) {
        if self.ui.text_input(cx, ids!(filter_input)).key_focus(cx) || key.modifiers.logo || key.modifiers.control {
            return;
        }
        let model = &mut self.model;
        match key.key_code {
            KeyCode::Escape => {
                if self.armed.take().is_some() {
                    model.notice = Some("force cancelled".to_string());
                    model.requests.redraw = true;
                } else {
                    model.go_live();
                }
            }
            KeyCode::KeyL => model.go_live(),
            KeyCode::Comma => model.step(-1),
            KeyCode::Period => model.step(1),
            KeyCode::KeyP => {
                if let Some(key) = model.selected {
                    let name = model.find_meta(key).map(|meta| meta.name.clone()).unwrap_or_default();
                    model.toggle_pin(key, &name);
                }
            }
            KeyCode::KeyK | KeyCode::Delete | KeyCode::Backspace => model.requests.terminate = Some(key.modifiers.shift),
            KeyCode::KeyT => {
                model.tree = !model.tree;
                model.changed();
            }
            KeyCode::KeyI => self.toggle_inspector(cx),
            KeyCode::Space => {
                if let Some(key) = model.selected.filter(|_| model.tree) {
                    if !model.collapsed.remove(&key) {
                        model.collapsed.insert(key);
                    }
                    model.changed();
                }
            }
            _ => {}
        }
    }

    fn toggle_inspector(&mut self, cx: &mut Cx) {
        // An explicit choice: selection no longer opens it by itself.
        self.model.inspector_auto = false;
        self.model.inspector_open = !self.model.inspector_open;
        self.model.requests.inspect = true;
        self.apply_inspector(cx);
        self.model.changed();
    }

    /// Wide enough: the inspector shares the body with the table. On a
    /// phone-sized tile it takes the body over while open (Hide or the
    /// Inspector button gives the table back); nowhere is it unreachable.
    fn apply_inspector(&mut self, cx: &mut Cx) {
        let collapse = if !self.model.inspector_open {
            SplitterCollapse::B
        } else if self.density == Density::Phone {
            SplitterCollapse::A
        } else {
            SplitterCollapse::None
        };
        if self.inspector_applied == Some(collapse) {
            return;
        }
        self.inspector_applied = Some(collapse);
        self.ui.splitter(cx, ids!(body_split)).set_collapse(cx, collapse);
    }

    /// Fold the layout down as the window shrinks. Strips are hidden whole;
    /// toolbar controls that no longer fit move to a second toolbar row, so
    /// nothing becomes unreachable.
    fn apply_layout(&mut self, cx: &mut Cx, size: Vec2d) {
        let density = Density::for_size(size);
        let changed = !self.density_applied || self.density != density || self.layout_size != size;
        if !changed {
            return;
        }
        self.layout_size = size;
        self.density = density;
        self.density_applied = true;
        let small = density == Density::Small;
        let phone = density == Density::Phone;
        let roomy = matches!(density, Density::Full | Density::Medium);
        self.model.compact = phone;
        let narrow = size.x < 760.0;
        // Below this the interval, History range and Freeze move to the
        // toolbar's second row; nothing is hidden.
        let wrap = size.x < 1000.0;
        for (id, path, visible) in [
            (live_id!(toolbar), ids!(toolbar), !small),
            (live_id!(rule_toolbar), ids!(rule_toolbar), !small),
            (live_id!(metric_wrap), ids!(metric_wrap), roomy),
            (live_id!(rule_metrics), ids!(rule_metrics), roomy),
            (live_id!(rule_band), ids!(rule_band), !small),
            (live_id!(body_split), ids!(body_split), !small),
            (live_id!(status_bar), ids!(status_bar), !small),
            (live_id!(rule_status), ids!(rule_status), !small),
            (live_id!(inspector_toggle), ids!(inspector_toggle), !narrow),
            (live_id!(freeze_toggle), ids!(freeze_toggle), !wrap),
            (live_id!(refresh_wrap), ids!(refresh_wrap), !wrap),
            (live_id!(history_wrap), ids!(history_wrap), !wrap),
            (live_id!(tool_row_more), ids!(tool_row_more), wrap),
            (live_id!(inspector_toggle_more), ids!(inspector_toggle_more), narrow),
        ] {
            self.set_visible(cx, id, path, visible);
        }
        let band_height = if small { Size::fill() } else if phone { Size::Fixed(84.0) } else if density == Density::Medium { Size::Fixed(92.0) } else { Size::Fixed(100.0) };
        // The chips share their row with the stepping controls and wrap onto
        // more rows only when that row is too narrow.
        let controls = if size.x >= 1180.0 { 300.0 } else { 200.0 };
        let rows = SeriesLegend::rows_for((size.x - 24.0 - 8.0 - controls).max(120.0));
        if let Some(mut legend) = self.ui.widget(cx, ids!(series_legend)).borrow_mut::<SeriesLegend>() {
            legend.set_height(rows as f64 * 24.0 + 2.0);
        }
        if let Some(mut band) = self.ui.widget(cx, ids!(history_band)).borrow_mut::<HistoryBand>() {
            band.set_height(band_height);
        }
        let filter_width = if narrow { 140.0 } else if size.x < 900.0 { 170.0 } else if size.x < 1180.0 { 200.0 } else { 260.0 };
        let mut filter = self.ui.widget(cx, ids!(filter_input));
        script_apply_eval!(cx, filter, {width: #(filter_width)});
        // The inspector's share follows the window: ~250 at 900 tall, ~180 at 650.
        let inspector_height = (size.y * 0.3).clamp(180.0, 320.0);
        self.ui.splitter(cx, ids!(body_split)).set_align(cx, SplitterAlign::FromB(inspector_height));
        self.apply_inspector(cx);
        self.update_chrome(cx);
        self.ui.redraw(cx);
        log!("task: layout {density:?} at {:.0}x{:.0}", size.x, size.y);
    }

    fn layout_from_window(&mut self, cx: &mut Cx) {
        let size = self.ui.window(cx, ids!(main_window)).get_inner_size(cx);
        // Before the first draw the window reports nothing; the geometry event
        // that follows carries the real size.
        if size.x > 1.0 && size.y > 1.0 {
            self.apply_layout(cx, size);
        }
    }

    /// Paint the chrome in the app's surface roles: the caption, toolbar,
    /// tab bar, table header and status bar raised; content on the
    /// background; rules quiet; the splitter a 1 px line.
    fn apply_theme(&mut self, cx: &mut Cx) {
        let theme = self.model.theme;
        let background = theme.background;
        let raised = theme.raised();
        let well = theme.well();
        let pill = theme.well_strong();
        let rule = theme.rule();
        let foreground = theme.foreground;
        let secondary = theme.secondary();
        let accent = theme.accent;
        let mut app_bg = self.ui.view(cx, ids!(app_bg));
        script_apply_eval!(cx, app_bg, {draw_bg +: {color: #(background)}});
        // Makepad's own caption bar (a SolidView on theme.color_app_caption_bar)
        // takes the toolbar's surface so the two read as one header.
        let mut caption = self.ui.view(cx, ids!(caption_bar));
        script_apply_eval!(cx, caption, {draw_bg +: {color: #(raised)}});
        for path in [ids!(toolbar), ids!(inspector_tabs), ids!(status_bar)] {
            let mut view = self.ui.view(cx, path);
            script_apply_eval!(cx, view, {draw_bg +: {color: #(raised)}});
        }
        for path in [ids!(rule_toolbar), ids!(rule_band), ids!(rule_metrics), ids!(rule_pinned), ids!(rule_status)] {
            let mut view = self.ui.view(cx, path);
            script_apply_eval!(cx, view, {draw_bg +: {color: #(rule)}});
        }
        let mut split = self.ui.widget(cx, ids!(body_split));
        script_apply_eval!(cx, split, {draw_bg +: {color_bg: #(background) color: #(rule) color_hover: #(accent) color_drag: #(accent)}});
        for path in [ids!(scope_seg), ids!(view_seg)] {
            let mut segments = self.ui.widget(cx, path);
            script_apply_eval!(cx, segments, {
                draw_bg +: {color: #(well) border_color: #(rule)}
                draw_pill +: {color: #(pill) border_color: #(rule)}
                draw_text +: {color: #(secondary)}
                draw_text_selected +: {color: #(foreground)}
            });
        }
        for path in [ids!(status_left), ids!(status_right), ids!(refresh_label), ids!(refresh_label_more), ids!(history_label), ids!(history_label_more)] {
            let mut label = self.ui.label(cx, path);
            script_apply_eval!(cx, label, {draw_text +: {color: #(secondary)}});
        }
        if let Some(mut table) = self.ui.widget(cx, ids!(process_table)).borrow_mut::<ProcessTable>() {
            table.apply_theme(cx, theme);
        }
        self.styled.clear();
        self.segments = None;
        self.kill_state = None;
        self.update_chrome(cx);
        self.ui.redraw(cx);
    }
}

/// The looks a button can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    Plain,
    On,
    TabActive,
    TabIdle,
    Danger,
    DangerOff,
}

const TAB_IDS: [(LiveId, &[LiveId]); 8] = [
    (live_id!(tab_info), ids!(tab_info)),
    (live_id!(tab_activity), ids!(tab_activity)),
    (live_id!(tab_history), ids!(tab_history)),
    (live_id!(tab_threads), ids!(tab_threads)),
    (live_id!(tab_memory), ids!(tab_memory)),
    (live_id!(tab_files), ids!(tab_files)),
    (live_id!(tab_ports), ids!(tab_ports)),
    (live_id!(tab_libraries), ids!(tab_libraries)),
];

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(index) = self.ui.segmented_control(cx, ids!(scope_seg)).selected(actions) {
            self.model.apps_only = index == 1;
            self.segments = None;
            self.model.changed();
        }
        if let Some(index) = self.ui.segmented_control(cx, ids!(view_seg)).selected(actions) {
            self.model.tree = index == 1;
            self.segments = None;
            self.model.changed();
        }
        if self.ui.button(cx, ids!(freeze_toggle)).clicked(actions) || self.ui.button(cx, ids!(freeze_toggle_more)).clicked(actions) {
            let model = &mut self.model;
            model.freeze = !model.freeze;
            model.notice = Some(if model.freeze { "Row order frozen; values keep updating".to_string() } else { "Row order follows the sort again".to_string() });
            model.changed();
        }
        // The interval picker exists in both toolbar rows; keep them agreeing.
        let picked = self
            .ui
            .drop_down(cx, ids!(refresh_select))
            .changed(actions)
            .or_else(|| self.ui.drop_down(cx, ids!(refresh_select_more)).changed(actions));
        if let Some(choice) = picked {
            let millis = REFRESH_CHOICES_MS.get(choice).copied().unwrap_or(REFRESH_CHOICES_MS[DEFAULT_REFRESH]);
            self.model.interval_ms = millis;
            self.model.changed();
            self.ui.drop_down(cx, ids!(refresh_select)).set_selected_item(cx, choice);
            self.ui.drop_down(cx, ids!(refresh_select_more)).set_selected_item(cx, choice);
            self.send(Command::Interval(millis));
            log!("task: refresh interval now {millis} ms");
        }
        let model = &mut self.model;
        let range = self
            .ui
            .drop_down(cx, ids!(range_select))
            .changed(actions)
            .or_else(|| self.ui.drop_down(cx, ids!(range_select_more)).changed(actions));
        if let Some(choice) = range {
            model.range = choice.min(RANGES_MS.len() - 1);
            model.custom_span_ms = None;
            if !model.live {
                model.window_end_ms = Some(model.cursor_ms + model.range_ms() / 10);
            }
            model.changed();
            self.ui.drop_down(cx, ids!(range_select)).set_selected_item(cx, choice);
            self.ui.drop_down(cx, ids!(range_select_more)).set_selected_item(cx, choice);
        }
        let model = &mut self.model;
        if self.ui.button(cx, ids!(step_back)).clicked(actions) {
            model.step(-1);
        }
        if self.ui.button(cx, ids!(step_forward)).clicked(actions) {
            model.step(1);
        }
        if self.ui.button(cx, ids!(live_button)).clicked(actions) {
            model.go_live();
        }
        if let Some(filter) = self.ui.text_input(cx, ids!(filter_input)).changed(actions) {
            model.filter = filter;
            model.notice = None;
            model.changed();
        }
        if let Some(modifiers) = self.ui.button(cx, ids!(kill_button)).clicked_modifiers(actions) {
            model.requests.terminate = Some(modifiers.shift);
        }
        let toggle = self.ui.button(cx, ids!(inspector_toggle)).clicked(actions)
            || self.ui.button(cx, ids!(inspector_toggle_more)).clicked(actions)
            || self.ui.button(cx, ids!(close_inspector)).clicked(actions);
        if toggle {
            self.toggle_inspector(cx);
        }
        for (index, (_, path)) in TAB_IDS.iter().enumerate() {
            if self.ui.button(cx, path).clicked(actions) {
                if let Some(mut body) = self.ui.widget(cx, ids!(inspector_body)).borrow_mut::<InspectorBody>() {
                    body.set_tab(cx, INSPECTOR_TABS[index]);
                }
                if !self.model.inspector_open {
                    self.toggle_inspector(cx);
                }
                self.model.requests.redraw = true;
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        // The Makepad WM palette retints the stock widgets before anything is built.
        makepad_wm_theme::apply(vm);
        // The custom widgets must exist in mod.widgets before the UI below
        // does `use mod.widgets.*`.
        crate::widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if matches!(event, Event::LiveEdit) {
            self.model.theme = Theme::from_palette(cx.with_vm(makepad_wm_theme::current_for_vm));
            self.apply_theme(cx);
            self.density_applied = false;
            self.layout_from_window(cx);
        }
        if let Event::Startup = event {
            // Checked once: a warm-pool instance stays dormant until
            // `WmEvent::Adopted` or a real input wakes it (see `Dormancy`).
            self.dormancy = Dormancy::start(makepad_wm_api::warm_start());
            self.model.theme = Theme::from_palette(cx.with_vm(makepad_wm_theme::current_for_vm));
            self.apply_theme(cx);
            // `--size WxH` lets a test drive the breakpoints without a WM.
            if let Some(size) = size_from_args() {
                self.ui.window(cx, ids!(main_window)).resize(cx, size);
            }
            self.layout_from_window(cx);
            // No-ops while dormant — see `start_sampler`.
            self.start_sampler(cx);
        }
        // The window is often an wm tile, so the layout follows its size
        // rather than assuming a desktop-sized window.
        if let Event::WindowGeomChange(geom) = event {
            self.apply_layout(cx, geom.new_geom.inner_size);
        }
        if let Event::Signal = event {
            self.poll_worker(cx);
        }
        if self.motion.next_frame.is_event(event).is_some() {
            self.motion.requested = false;
            self.animate(cx);
        }
        // `StudioToApp::Custom` from wm reaches a hosted app as
        // `Event::Custom(json)`; `Adopted` is what wakes a warm instance.
        if let Event::Custom(json) = event {
            if let Some(makepad_wm_api::WmEvent::Adopted) = makepad_wm_api::WmEvent::parse(json) {
                self.wake(cx);
            }
        }
        // Defensive fallback: a lost `Adopted` message must not leave a
        // visibly-adopted, actually-being-used instance sampling nothing.
        if self.dormancy.is_dormant() && is_wake_input(event) {
            self.wake(cx);
        }
        if let Event::KeyDown(key) = event {
            self.handle_key(cx, key);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::with_data(&mut self.model));
        self.after_event(cx);
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

fn interval_text(ms: u64) -> String {
    if ms < 1000 { format!("{:.1} s", ms as f64 / 1000.0) } else { format!("{} s", ms / 1000) }
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

/// CPU time as `h:mm:ss.ss` (or `m:ss.ss`).
pub fn format_duration_ns(ns: u64) -> String {
    let centis = ns / 10_000_000;
    let seconds = centis / 100;
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{:02}.{:02}", seconds % 60, centis % 100)
    } else {
        format!("{minutes}:{:02}.{:02}", seconds % 60, centis % 100)
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
        // Nothing may panic when MAKEPAD_WM_THEME_SPLASH is unset or bogus.
        let theme = Theme::from_environment();
        assert!(theme.accent.w > 0.0);
    }

    // ---- warm-pool dormancy ----

    #[test]
    fn non_warm_starts_active() {
        let dormancy = Dormancy::start(false);
        assert_eq!(dormancy, Dormancy::Active);
        assert!(!dormancy.is_dormant());
    }

    #[test]
    fn warm_starts_dormant_and_adopted_wakes_exactly_once() {
        let mut dormancy = Dormancy::start(true);
        assert!(dormancy.is_dormant());
        // Adopted wakes it...
        assert!(dormancy.wake());
        assert!(!dormancy.is_dormant());
        assert_eq!(dormancy, Dormancy::Woken);
        // ...and a second Adopted (or a stray input) never fires again.
        assert!(!dormancy.wake());
        assert_eq!(dormancy, Dormancy::Woken);
    }

    #[test]
    fn waking_an_already_active_instance_is_a_no_op() {
        let mut dormancy = Dormancy::start(false);
        assert!(!dormancy.wake());
        assert_eq!(dormancy, Dormancy::Active);
    }

    #[test]
    fn key_and_pointer_events_are_wake_input() {
        assert!(is_wake_input(&Event::KeyDown(KeyEvent::default())));
        assert!(is_wake_input(&Event::MouseDown(MouseDownEvent {
            abs: dvec2(0.0, 0.0),
            button: MouseButton::PRIMARY,
            window_id: WindowId(0, 0),
            modifiers: KeyModifiers::default(),
            handled: std::cell::Cell::new(Area::default()),
            time: 0.0,
        })));
        // Touch ("finger") input wakes it too — same match arm as the mouse
        // and keyboard cases above; `TouchUpdateEvent` is not part of the
        // widgets crate's public re-export surface so it is not
        // constructible from an app crate's test.
        // A timer tick or a signal drain is not a human touching the app.
        assert!(!is_wake_input(&Event::Signal));
    }

    #[test]
    fn input_wakes_a_dormant_instance_the_same_as_adopted() {
        let mut dormancy = Dormancy::start(true);
        assert!(is_wake_input(&Event::KeyDown(KeyEvent::default())));
        assert!(dormancy.wake());
        assert!(!dormancy.is_dormant());
    }
}

#[cfg(test)]
mod application_style_tests {
    include!("../../../widgets/tests/support/app_style.rs");
}
