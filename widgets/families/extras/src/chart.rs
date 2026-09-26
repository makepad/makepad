use crate::{
    family_api::measure,
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ChartViewBase = #(ChartView::register_widget(vm))

    mod.widgets.ChartView = set_type_default() do mod.widgets.ChartViewBase{
        width: Fill
        height: Fill
        candle_up_color: #x26a69a
        candle_down_color: #xef5350
        wick_color: #x888888
        grid_color: #x2a2a3e
        grid_text_color: #x777777
        border_color: #x3a3a4e
        high_line_color: #x4db6ac
        low_line_color: #xef9a9a
        bg_color: #x1a1a2e
        candle_width_fraction: 0.7
        line_color: #x4fc3f7
        line_width: 2.0
        fill_color: #x4fc3f733
        bar_color: #x42a5f5
        dot_color: #x66bb6a
        dot_radius: 4.0
        bar_width_fraction: 0.7
        plot_margin: Inset{left: 60.0, top: 10.0, right: 10.0, bottom: 24.0}

        draw_bg +: {
            draw_depth: 0.0
            color: #x1a1a2e
        }

        draw_grid_line +: {
            draw_depth: 0.1
            color: #x2a2a3e
        }

        draw_vector +: {
            draw_depth: 2.0
        }

        draw_text +: {
            draw_depth: 3.0
            color: #x777777
        }
    }

    mod.widgets.CandlestickChartBase = #(CandlestickChart::register_widget(vm))

    mod.widgets.CandlestickChart = set_type_default() do mod.widgets.CandlestickChartBase{
        width: Fill
        height: Fill
    }

    mod.widgets.LineChartBase = #(LineChart::register_widget(vm))

    mod.widgets.LineChart = set_type_default() do mod.widgets.LineChartBase{
        width: Fill
        height: Fill
    }

    mod.widgets.BarChartBase = #(BarChart::register_widget(vm))

    mod.widgets.BarChart = set_type_default() do mod.widgets.BarChartBase{
        width: Fill
        height: Fill
    }

    mod.widgets.AreaChartBase = #(AreaChart::register_widget(vm))

    mod.widgets.AreaChart = set_type_default() do mod.widgets.AreaChartBase{
        width: Fill
        height: Fill
    }

    mod.widgets.ScatterChartBase = #(ScatterChart::register_widget(vm))

    mod.widgets.ScatterChart = set_type_default() do mod.widgets.ScatterChartBase{
        width: Fill
        height: Fill
    }

    mod.widgets.OhlcChartBase = #(OhlcChart::register_widget(vm))

    mod.widgets.OhlcChart = set_type_default() do mod.widgets.OhlcChartBase{
        width: Fill
        height: Fill
    }

    mod.widgets.SparklineBase = #(Sparkline::register_widget(vm))

    mod.widgets.Sparkline = set_type_default() do mod.widgets.SparklineBase{
        width: Fill
        height: Fill
        color_up: #x26a69a
        color_down: #xef5350
        baseline_alpha: 0.35
        gap: 1.0
    }

    set_type_default() do #(DrawChartSegment::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn(){
            let p = self.pos * self.rect_size
            let a = self.seg_a
            let b = self.seg_b
            if self.mode > 0.5 {
                let t = clamp((p.x - a.x) / max(b.x - a.x, 0.0001), 0.0, 1.0)
                let ly = mix(a.y, b.y, t)
                let below = p.y - ly
                if below < 0.0 {
                    return #0000
                }
                let fade = clamp(1.0 - below / max(self.fade_len, 1.0), 0.0, 1.0)
                let alpha = self.color.a * fade * fade
                return vec4(self.color.rgb * alpha, alpha)
            }
            let pa = p - a
            let ba = b - a
            let h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0)
            let d = length(pa - ba * h)
            let aa = 1.0 - smoothstep(self.thickness * 0.5 - 0.75, self.thickness * 0.5 + 0.75, d)
            return vec4(self.color.rgb * self.color.a * aa, self.color.a * aa)
        }
    }

    mod.widgets.TrendChartBase = #(TrendChart::register_widget(vm))

    mod.widgets.TrendChart = set_type_default() do mod.widgets.TrendChartBase{
        width: Fill
        height: Fill
        color_bg: #x14142a
        color_grid: #x272a44
        color_line: #x4fc3f7
        color_line_2: #xff8a65
        color_line_3: #x81c784
        color_line_4: #xce93d8
        color_fill: #x4fc3f73f
        color_up: #x26a69a
        color_down: #xef5350
        color_text: #x8890a8
        color_accent: #xffb74d
        line_width: 2.0
        show_legend: false
        range_min: 0.0
        range_max: 0.0
        series: []

        // The face is named outright: a merge into a text style the type
        // default has not set yet left the tile with a style that measured
        // but drew nothing -- a key of swatches with no names beside them,
        // a gutter with no numbers.
        draw_text +: {
            text_style: theme.font_regular{font_size: 8.0}
            color: #x8890a8
        }
    }
}

// ---- Data types ----

#[derive(Clone, Debug)]
pub struct Candle {
    pub time: f64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// A simple (x, y) data point for line/bar/area/scatter charts.
#[derive(Clone, Debug)]
pub struct DataPoint {
    pub x: f64,
    pub y: f64,
}

/// Trait for simple (x, y) series data.
pub trait PointData {
    fn point_len(&self) -> usize;
    fn point_is_empty(&self) -> bool {
        self.point_len() == 0
    }
    fn get_points(&self, start: usize, end: usize) -> &[DataPoint];
}

/// Default flat in-memory point data source.
pub struct FlatPointData {
    pub points: Vec<DataPoint>,
}

impl Default for FlatPointData {
    fn default() -> Self {
        Self { points: Vec::new() }
    }
}

impl FlatPointData {
    pub fn new(points: Vec<DataPoint>) -> Self {
        Self { points }
    }
}

impl PointData for FlatPointData {
    fn point_len(&self) -> usize {
        self.points.len()
    }
    fn get_points(&self, start: usize, end: usize) -> &[DataPoint] {
        let s = start.min(self.points.len());
        let e = end.min(self.points.len());
        &self.points[s..e]
    }
}

// ---- Rows: the data as markup writes it ----
//
// The data model lives in the base. `TrendChart` here and every shape in
// `chart_shapes` read their lines through it, so the shapes import from
// this file and this file imports nothing from them.

/// One part of a chart as it was written down: a name, and the numbers
/// after it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub label: String,
    pub values: Vec<f64>,
}

impl Row {
    pub fn new(label: &str, values: &[f64]) -> Self {
        Self { label: label.to_string(), values: values.to_vec() }
    }

    /// The row's first number, a row without one counting as nothing. A
    /// line that carried no number is a part worth nothing, which is a
    /// thing a chart can draw; it is not a part that is missing.
    pub fn value(&self) -> f64 {
        self.values.first().copied().unwrap_or(0.0)
    }

    pub fn value_at(&self, i: usize) -> f64 {
        self.values.get(i).copied().unwrap_or(0.0)
    }
}

/// Read one line of markup as a label and the numbers after it.
///
/// `"Rent 420"`, `"Public transport 120"`, `"Speed: 4 3 5 2"`. A colon, if
/// there is one, splits the two. Without one the numbers are the longest
/// run of them at the END of the line, so a label may be several words and
/// may hold a number of its own as long as a word comes after it — `"Q1
/// 2024 480"` is one quarter worth 480 and not three numbers.
pub fn parse_row(line: &str) -> Row {
    if let Some((name, rest)) = line.split_once(':') {
        return Row {
            label: name.trim().to_string(),
            values: rest.split_whitespace().filter_map(|t| t.parse::<f64>().ok()).collect(),
        };
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut first = words.len();
    while first > 0 && words[first - 1].parse::<f64>().is_ok() {
        first -= 1;
    }
    Row {
        label: words[..first].join(" "),
        values: words[first..].iter().filter_map(|t| t.parse::<f64>().ok()).collect(),
    }
}

pub fn parse_rows(lines: &[String]) -> Vec<Row> {
    lines.iter().map(|line| parse_row(line)).collect()
}

/// A number as a chart writes it: whole when it is whole, one decimal when
/// it is not. Charts are read at a glance, and a trailing `.00` on every
/// part of every one of them is noise.
pub fn fmt_value(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 && v.abs() < 1e15 {
        format!("{}", v.round() as i64)
    } else {
        format!("{:.1}", v)
    }
}

// ---- DataSource trait ----

pub trait DataSource {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn get_range(&self, start: usize, end: usize) -> &[Candle];
    fn get_averaged(&self, start: usize, end: usize, bucket_size: usize) -> Vec<Candle>;
}

// ---- FlatDataSource ----

pub struct FlatDataSource {
    pub candles: Vec<Candle>,
}

impl Default for FlatDataSource {
    fn default() -> Self {
        Self {
            candles: Vec::new(),
        }
    }
}

impl FlatDataSource {
    pub fn new(candles: Vec<Candle>) -> Self {
        Self { candles }
    }
}

impl DataSource for FlatDataSource {
    fn len(&self) -> usize {
        self.candles.len()
    }

    fn get_range(&self, start: usize, end: usize) -> &[Candle] {
        let s = start.min(self.candles.len());
        let e = end.min(self.candles.len());
        &self.candles[s..e]
    }

    fn get_averaged(&self, start: usize, end: usize, bucket_size: usize) -> Vec<Candle> {
        let s = start.min(self.candles.len());
        let e = end.min(self.candles.len());
        let bucket_size = bucket_size.max(1);
        let mut result = Vec::new();
        let mut i = s;
        while i < e {
            let bucket_end = (i + bucket_size).min(e);
            let slice = &self.candles[i..bucket_end];
            if slice.is_empty() {
                break;
            }
            let open = slice[0].open;
            let close = slice[slice.len() - 1].close;
            let time = slice[0].time;
            let mut high = f64::NEG_INFINITY;
            let mut low = f64::INFINITY;
            let mut volume = 0.0;
            for c in slice {
                if c.high > high {
                    high = c.high;
                }
                if c.low < low {
                    low = c.low;
                }
                volume += c.volume;
            }
            result.push(Candle {
                time,
                open,
                high,
                low,
                close,
                volume,
            });
            i = bucket_end;
        }
        result
    }
}

// ---- Fake data generator ----

pub fn generate_fake_stock_data(count: usize, start_price: f64) -> Vec<Candle> {
    let mut candles = Vec::with_capacity(count);
    let mut price = start_price;
    let mut seed: u64 = 12345;

    for i in 0..count {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let r1 = (seed as f64) / (u64::MAX as f64);

        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let r2 = (seed as f64) / (u64::MAX as f64);

        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let r3 = (seed as f64) / (u64::MAX as f64);

        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let r4 = (seed as f64) / (u64::MAX as f64);

        let open = price;
        let change = (r1 - 0.48) * 0.06 * price;
        let close = open + change;

        let body_high = open.max(close);
        let body_low = open.min(close);
        let high = body_high + r2 * 0.015 * price;
        let low = body_low - r3 * 0.015 * price;

        let volume = 1000.0 + r4 * 9000.0 * (1.0 + (change / price).abs() * 5.0);

        candles.push(Candle {
            time: i as f64,
            open,
            high,
            low: low.max(0.01),
            close: close.max(0.01),
            volume,
        });

        price = close.max(0.01);
    }
    candles
}

// ---- ChartViewport ----

#[derive(Clone, Debug)]
pub struct ChartViewport {
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
}

impl Default for ChartViewport {
    fn default() -> Self {
        Self {
            x_min: 0.0,
            x_max: 100.0,
            y_min: 0.0,
            y_max: 100.0,
        }
    }
}

impl ChartViewport {
    pub fn x_range(&self) -> f64 {
        self.x_max - self.x_min
    }
    pub fn y_range(&self) -> f64 {
        self.y_max - self.y_min
    }
}

// ---- ChartView widget ----

#[derive(Script, ScriptHook, Widget)]
pub struct ChartView {
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
    draw_bg: DrawColor,
    #[live]
    draw_grid_line: DrawColor,
    #[live]
    draw_vector: DrawVector,
    #[live]
    draw_text: DrawText,

    #[rust]
    pub viewport: ChartViewport,
    #[rust]
    rect: Rect,
    #[rust]
    plot_rect: Rect,

    // Pan/zoom interaction
    #[rust]
    drag_start_abs: Option<DVec2>,
    #[rust]
    drag_start_viewport: ChartViewport,

    // Styling
    #[live]
    pub candle_up_color: Vec4f,
    #[live]
    pub candle_down_color: Vec4f,
    #[live]
    pub wick_color: Vec4f,
    #[live]
    pub grid_color: Vec4f,
    #[live]
    pub grid_text_color: Vec4f,
    #[live]
    pub border_color: Vec4f,
    #[live]
    pub high_line_color: Vec4f,
    #[live]
    pub low_line_color: Vec4f,
    #[live]
    pub bg_color: Vec4f,
    #[live(0.7)]
    pub candle_width_fraction: f32,
    #[live]
    pub line_color: Vec4f,
    #[live(2.0)]
    pub line_width: f32,
    #[live]
    pub fill_color: Vec4f,
    #[live]
    pub bar_color: Vec4f,
    #[live]
    pub dot_color: Vec4f,
    #[live(4.0)]
    pub dot_radius: f32,
    #[live(0.7)]
    pub bar_width_fraction: f32,

    #[live]
    pub plot_margin: Inset,
}

impl Widget for ChartView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // Who owns this pointer? The hit below is taken with
        // `capture_overload`, so this chart co-captures EVERY press landing
        // inside it, one a child control already holds included — the answer
        // therefore has to be asked for rather than assumed from having been
        // handed a hit. The chart's own co-capture is `mine` and does not
        // count. Asked once, before the hit, and used at the press and on
        // every move: the press and a child's capture can land in either
        // order inside one event.
        let held_outside = cx.fingers.is_mouse_held_outside(&[self.draw_bg.area()]);
        match event.hits_with_capture_overload(cx, self.draw_bg.area(), true) {
            Hit::FingerDown(fe) if press_starts_pan(fe.is_primary_hit(), held_outside) => {
                self.drag_start_abs = Some(fe.abs);
                self.drag_start_viewport = self.viewport.clone();
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                if pan_stands_down(held_outside) {
                    // Handed back mid-gesture: whatever took the pointer owns
                    // the rest of this press, and a half-finished pan left
                    // waiting here would resume the moment that control let
                    // go.
                    self.drag_start_abs = None;
                } else if let Some(start_abs) = self.drag_start_abs {
                    let delta = fe.abs - start_abs;
                    let pr = &self.plot_rect;
                    if pr.size.x > 0.0 && pr.size.y > 0.0 {
                        let dx = delta.x / pr.size.x * self.drag_start_viewport.x_range();
                        let dy = delta.y / pr.size.y * self.drag_start_viewport.y_range();
                        self.viewport.x_min = self.drag_start_viewport.x_min - dx;
                        self.viewport.x_max = self.drag_start_viewport.x_max - dx;
                        self.viewport.y_min = self.drag_start_viewport.y_min + dy;
                        self.viewport.y_max = self.drag_start_viewport.y_max + dy;
                    }
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(_) => {
                self.drag_start_abs = None;
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerScroll(fs) => {
                let scroll = if fs.scroll.y.abs() > f64::EPSILON {
                    fs.scroll.y
                } else {
                    fs.scroll.x
                };
                self.zoom_at(cx, scroll, fs.abs);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.rect = cx.walk_turtle(walk);
        self.compute_plot_rect();
        self.draw_bg.draw_abs(cx, self.rect);
        DrawStep::done()
    }
}

impl ChartView {
    fn compute_plot_rect(&mut self) {
        let m = &self.plot_margin;
        self.plot_rect = Rect {
            pos: DVec2 {
                x: self.rect.pos.x + m.left,
                y: self.rect.pos.y + m.top,
            },
            size: DVec2 {
                x: (self.rect.size.x - m.left - m.right).max(1.0),
                y: (self.rect.size.y - m.top - m.bottom).max(1.0),
            },
        };
    }

    fn zoom_at(&mut self, cx: &mut Cx, scroll: f64, abs: DVec2) {
        let factor = if scroll > 0.0 { 0.9 } else { 1.0 / 0.9 };
        let pr = &self.plot_rect;
        let frac_x = ((abs.x - pr.pos.x) / pr.size.x).clamp(0.0, 1.0);
        let data_x = self.viewport.x_min + frac_x * self.viewport.x_range();
        let new_x_range = (self.viewport.x_range() * factor).clamp(2.0, 100000.0);
        self.viewport.x_min = data_x - frac_x * new_x_range;
        self.viewport.x_max = data_x + (1.0 - frac_x) * new_x_range;
        self.redraw(cx);
    }

    // ---- Immediate-mode drawing API ----

    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.rect = cx.walk_turtle(walk);
        self.compute_plot_rect();
        // Layer 0: background
        self.draw_bg.draw_abs(cx, self.rect);
        // Child turtle with clipping at the plot margins
        cx.begin_turtle(
            Walk {
                abs_pos: Some(self.rect.pos),
                width: Size::Fixed(self.rect.size.x),
                height: Size::Fixed(self.rect.size.y),
                ..Default::default()
            },
            Layout {
                clip_x: true,
                clip_y: true,
                padding: self.plot_margin,
                ..Layout::default()
            },
        );
        // Layer 2: vector candles (single begin/end session)
        self.draw_vector.begin();
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        self.draw_vector.end(cx);
        cx.end_turtle();
    }

    // Coordinate transforms

    pub fn data_to_px(&self, x: f64, y: f64) -> (f32, f32) {
        let pr = &self.plot_rect;
        let vp = &self.viewport;
        let px = pr.pos.x + (x - vp.x_min) / vp.x_range() * pr.size.x;
        let py = pr.pos.y + (1.0 - (y - vp.y_min) / vp.y_range()) * pr.size.y;
        (px as f32, py as f32)
    }

    pub fn px_to_data(&self, px: f32, py: f32) -> (f64, f64) {
        let pr = &self.plot_rect;
        let vp = &self.viewport;
        let x = vp.x_min + (px as f64 - pr.pos.x) / pr.size.x * vp.x_range();
        let y = vp.y_min + (1.0 - (py as f64 - pr.pos.y) / pr.size.y) * vp.y_range();
        (x, y)
    }

    // ---- Grid lines (DrawColor rects at depth 1) ----

    pub fn draw_grid_line_h(&mut self, cx: &mut Cx2d, y: f64, color: Vec4f) {
        let (_, py) = self.data_to_px(0.0, y);
        let pr = &self.plot_rect;
        self.draw_grid_line.color = color;
        self.draw_grid_line.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: pr.pos.x,
                    y: py as f64,
                },
                size: DVec2 {
                    x: pr.size.x,
                    y: 1.0,
                },
            },
        );
    }

    pub fn draw_grid_line_v(&mut self, cx: &mut Cx2d, x: f64, color: Vec4f) {
        let (px, _) = self.data_to_px(x, 0.0);
        let pr = &self.plot_rect;
        self.draw_grid_line.color = color;
        self.draw_grid_line.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: px as f64,
                    y: pr.pos.y,
                },
                size: DVec2 {
                    x: 1.0,
                    y: pr.size.y,
                },
            },
        );
    }

    pub fn draw_grid_y(&mut self, cx: &mut Cx2d, values: &[f64], labels: &[String]) {
        let color = self.grid_color;
        let text_color = self.grid_text_color;
        for (i, &val) in values.iter().enumerate() {
            self.draw_grid_line_h(cx, val, color);
            if i < labels.len() {
                let (_, py) = self.data_to_px(0.0, val);
                self.draw_text.color = text_color;
                self.draw_text.draw_abs(
                    cx,
                    dvec2(self.rect.pos.x + 4.0, py as f64 - 5.0),
                    &labels[i],
                );
            }
        }
    }

    pub fn draw_grid_x(&mut self, cx: &mut Cx2d, values: &[f64], labels: &[String]) {
        let color = self.grid_color;
        let text_color = self.grid_text_color;
        let label_y = self.rect.pos.y + self.rect.size.y - self.plot_margin.bottom + 4.0;
        for (i, &val) in values.iter().enumerate() {
            self.draw_grid_line_v(cx, val, color);
            if i < labels.len() {
                let (px, _) = self.data_to_px(val, 0.0);
                self.draw_text.color = text_color;
                self.draw_text
                    .draw_abs(cx, dvec2(px as f64 - 10.0, label_y), &labels[i]);
            }
        }
    }

    // ---- Plot border (DrawColor rects at depth 1) ----

    pub fn draw_plot_border(&mut self, cx: &mut Cx2d) {
        let pr = &self.plot_rect;
        let color = self.border_color;
        self.draw_grid_line.color = color;
        // Top edge
        self.draw_grid_line.draw_abs(
            cx,
            Rect {
                pos: pr.pos,
                size: DVec2 {
                    x: pr.size.x,
                    y: 1.0,
                },
            },
        );
        // Bottom edge
        self.draw_grid_line.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: pr.pos.x,
                    y: pr.pos.y + pr.size.y,
                },
                size: DVec2 {
                    x: pr.size.x,
                    y: 1.0,
                },
            },
        );
        // Left edge
        self.draw_grid_line.draw_abs(
            cx,
            Rect {
                pos: pr.pos,
                size: DVec2 {
                    x: 1.0,
                    y: pr.size.y,
                },
            },
        );
        // Right edge
        self.draw_grid_line.draw_abs(
            cx,
            Rect {
                pos: DVec2 {
                    x: pr.pos.x + pr.size.x,
                    y: pr.pos.y,
                },
                size: DVec2 {
                    x: 1.0,
                    y: pr.size.y,
                },
            },
        );
    }

    // ---- Min/max dashed lines (DrawColor rects at depth 1) ----

    pub fn draw_hline_dashed(&mut self, cx: &mut Cx2d, y: f64, color: Vec4f, label: &str) {
        let (px1, py) = self.data_to_px(self.viewport.x_min, y);
        let (px2, _) = self.data_to_px(self.viewport.x_max, y);
        let dash_len = 6.0_f32;
        let gap_len = 4.0_f32;
        self.draw_grid_line.color = color;
        let mut x = px1;
        while x < px2 {
            let end = (x + dash_len).min(px2);
            self.draw_grid_line.draw_abs(
                cx,
                Rect {
                    pos: DVec2 {
                        x: x as f64,
                        y: py as f64,
                    },
                    size: DVec2 {
                        x: (end - x) as f64,
                        y: 1.0,
                    },
                },
            );
            x = end + gap_len;
        }
        // Label at right edge
        self.draw_text.color = color;
        let label_x = self.rect.pos.x + self.rect.size.x - self.plot_margin.right - 45.0;
        self.draw_text
            .draw_abs(cx, dvec2(label_x, py as f64 - 5.0), label);
    }

    // ---- Candlestick drawing (DrawVector at depth 2) ----

    pub fn draw_candle(&mut self, candle: &Candle, slot_width: f64) {
        let is_up = candle.close >= candle.open;
        let body_color = if is_up {
            self.candle_up_color
        } else {
            self.candle_down_color
        };
        let wick_color = self.wick_color;

        let body_top = candle.open.max(candle.close);
        let body_bot = candle.open.min(candle.close);
        let candle_w = slot_width * self.candle_width_fraction as f64;
        let x_center = candle.time + 0.5;
        let x_left = x_center - candle_w * 0.5;

        // Wick
        self.draw_vector
            .set_color(wick_color.x, wick_color.y, wick_color.z, wick_color.w);
        let (wx, wy_high) = self.data_to_px(x_center, candle.high);
        let (_, wy_low) = self.data_to_px(x_center, candle.low);
        self.draw_vector.move_to(wx, wy_high);
        self.draw_vector.line_to(wx, wy_low);
        self.draw_vector.stroke(1.0);

        // Body
        self.draw_vector
            .set_color(body_color.x, body_color.y, body_color.z, body_color.w);
        let (bx, by_top) = self.data_to_px(x_left, body_top);
        let (bx2, by_bot) = self.data_to_px(x_left + candle_w, body_bot);
        let bw = (bx2 - bx).max(1.0);
        let bh = (by_bot - by_top).max(1.0);
        self.draw_vector.rect(bx, by_top, bw, bh);
        self.draw_vector.fill();
    }

    // ---- Generic drawing (DrawVector) ----

    pub fn set_color(&mut self, color: Vec4f) {
        self.draw_vector
            .set_color(color.x, color.y, color.z, color.w);
    }

    pub fn draw_line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, width: f32) {
        let (px1, py1) = self.data_to_px(x1, y1);
        let (px2, py2) = self.data_to_px(x2, y2);
        self.draw_vector.move_to(px1, py1);
        self.draw_vector.line_to(px2, py2);
        self.draw_vector.stroke(width);
    }

    // ---- Line series drawing (DrawVector) ----

    /// Draw a connected line through a series of data points.
    pub fn draw_line_series(&mut self, points: &[DataPoint], color: Vec4f, width: f32) {
        if points.len() < 2 {
            return;
        }
        self.draw_vector
            .set_color(color.x, color.y, color.z, color.w);
        let (px, py) = self.data_to_px(points[0].x, points[0].y);
        self.draw_vector.move_to(px, py);
        for p in &points[1..] {
            let (px, py) = self.data_to_px(p.x, p.y);
            self.draw_vector.line_to(px, py);
        }
        self.draw_vector.stroke(width);
    }

    /// Draw a filled area from data points down to y_base (typically 0 or viewport.y_min).
    pub fn draw_filled_area(&mut self, points: &[DataPoint], y_base: f64, color: Vec4f) {
        if points.len() < 2 {
            return;
        }
        self.draw_vector
            .set_color(color.x, color.y, color.z, color.w);
        let (_, py_base) = self.data_to_px(points[0].x, y_base);
        let (px0, py0) = self.data_to_px(points[0].x, points[0].y);
        self.draw_vector.move_to(px0, py_base);
        self.draw_vector.line_to(px0, py0);
        for p in &points[1..] {
            let (px, py) = self.data_to_px(p.x, p.y);
            self.draw_vector.line_to(px, py);
        }
        let (px_last, _) = self.data_to_px(points[points.len() - 1].x, points[points.len() - 1].y);
        self.draw_vector.line_to(px_last, py_base);
        self.draw_vector.close();
        self.draw_vector.fill();
    }

    /// Draw a single dot/circle at a data point.
    pub fn draw_dot(&mut self, x: f64, y: f64, radius: f32, color: Vec4f) {
        let (px, py) = self.data_to_px(x, y);
        self.draw_vector
            .set_color(color.x, color.y, color.z, color.w);
        self.draw_vector.circle(px, py, radius);
        self.draw_vector.fill();
    }

    /// Draw a vertical bar from y_base to the data point's y value.
    pub fn draw_bar(&mut self, x: f64, y: f64, slot_width: f64, y_base: f64, color: Vec4f) {
        let bar_w = slot_width * self.bar_width_fraction as f64;
        let x_left = x - bar_w * 0.5;
        let top = y.max(y_base);
        let bot = y.min(y_base);
        let (bx, by_top) = self.data_to_px(x_left, top);
        let (bx2, by_bot) = self.data_to_px(x_left + bar_w, bot);
        let bw = (bx2 - bx).max(1.0);
        let bh = (by_bot - by_top).max(1.0);
        self.draw_vector
            .set_color(color.x, color.y, color.z, color.w);
        self.draw_vector.rect(bx, by_top, bw, bh);
        self.draw_vector.fill();
    }

    /// Draw OHLC tick marks (open tick left, close tick right, high-low vertical line).
    pub fn draw_ohlc(&mut self, candle: &Candle, slot_width: f64) {
        let is_up = candle.close >= candle.open;
        let color = if is_up {
            self.candle_up_color
        } else {
            self.candle_down_color
        };
        let x_center = candle.time + 0.5;
        let tick_half = slot_width * self.candle_width_fraction as f64 * 0.5;

        self.draw_vector
            .set_color(color.x, color.y, color.z, color.w);

        // High-low vertical line
        let (cx_px, hy) = self.data_to_px(x_center, candle.high);
        let (_, ly) = self.data_to_px(x_center, candle.low);
        self.draw_vector.move_to(cx_px, hy);
        self.draw_vector.line_to(cx_px, ly);
        self.draw_vector.stroke(1.5);

        // Open tick (left side)
        let (left_px, _) = self.data_to_px(x_center - tick_half, candle.open);
        let (_, oy) = self.data_to_px(x_center, candle.open);
        self.draw_vector.move_to(left_px, oy);
        self.draw_vector.line_to(cx_px, oy);
        self.draw_vector.stroke(1.5);

        // Close tick (right side)
        let (right_px, _) = self.data_to_px(x_center + tick_half, candle.close);
        let (_, cy) = self.data_to_px(x_center, candle.close);
        self.draw_vector.move_to(cx_px, cy);
        self.draw_vector.line_to(right_px, cy);
        self.draw_vector.stroke(1.5);
    }

    // ---- Point data viewport helpers ----

    pub fn fit_point_data(&mut self, data: &dyn PointData) {
        if data.point_is_empty() {
            return;
        }
        let all = data.get_points(0, data.point_len());
        let mut x_min = f64::INFINITY;
        let mut x_max = f64::NEG_INFINITY;
        let mut y_min = f64::INFINITY;
        let mut y_max = f64::NEG_INFINITY;
        for p in all {
            if p.x < x_min {
                x_min = p.x;
            }
            if p.x > x_max {
                x_max = p.x;
            }
            if p.y < y_min {
                y_min = p.y;
            }
            if p.y > y_max {
                y_max = p.y;
            }
        }
        let x_pad = (x_max - x_min).max(1.0) * 0.05;
        let y_pad = (y_max - y_min).max(1.0) * 0.05;
        self.viewport = ChartViewport {
            x_min: x_min - x_pad,
            x_max: x_max + x_pad,
            y_min: y_min - y_pad,
            y_max: y_max + y_pad,
        };
    }

    pub fn fit_point_data_y(&mut self, data: &dyn PointData) {
        let vp = &self.viewport;
        let all = data.get_points(0, data.point_len());
        let mut y_min = f64::INFINITY;
        let mut y_max = f64::NEG_INFINITY;
        for p in all {
            if p.x >= vp.x_min && p.x <= vp.x_max {
                if p.y < y_min {
                    y_min = p.y;
                }
                if p.y > y_max {
                    y_max = p.y;
                }
            }
        }
        if y_min < y_max {
            let padding = (y_max - y_min) * 0.05;
            self.viewport.y_min = y_min - padding;
            self.viewport.y_max = y_max + padding;
        }
    }

    // ---- Viewport helpers ----

    pub fn set_viewport(&mut self, x_min: f64, x_max: f64, y_min: f64, y_max: f64) {
        self.viewport = ChartViewport {
            x_min,
            x_max,
            y_min,
            y_max,
        };
    }

    pub fn viewport(&self) -> &ChartViewport {
        &self.viewport
    }

    pub fn fit_data_y(&mut self, data: &dyn DataSource) {
        let vp = &self.viewport;
        let start = (vp.x_min.floor() as isize).max(0) as usize;
        let end = (vp.x_max.ceil() as usize + 1).min(data.len());
        if start >= end {
            return;
        }
        let candles = data.get_range(start, end);
        let mut y_min = f64::INFINITY;
        let mut y_max = f64::NEG_INFINITY;
        for c in candles {
            if c.low < y_min {
                y_min = c.low;
            }
            if c.high > y_max {
                y_max = c.high;
            }
        }
        if y_min < y_max {
            let padding = (y_max - y_min) * 0.05;
            self.viewport.y_min = y_min - padding;
            self.viewport.y_max = y_max + padding;
        }
    }

    pub fn fit_data(&mut self, data: &dyn DataSource) {
        if data.is_empty() {
            return;
        }
        let n = data.len();
        let all = data.get_range(0, n);
        let mut y_min = f64::INFINITY;
        let mut y_max = f64::NEG_INFINITY;
        for c in all {
            if c.low < y_min {
                y_min = c.low;
            }
            if c.high > y_max {
                y_max = c.high;
            }
        }
        let padding = (y_max - y_min).max(1.0) * 0.05;
        self.viewport = ChartViewport {
            x_min: -0.5,
            x_max: n as f64 + 0.5,
            y_min: y_min - padding,
            y_max: y_max + padding,
        };
    }

    pub fn plot_rect(&self) -> &Rect {
        &self.plot_rect
    }
}

// ---- Axis tick generation ----

fn nice_ticks(min: f64, max: f64, target_count: usize) -> Vec<f64> {
    let range = max - min;
    if range <= 0.0 || !range.is_finite() {
        return vec![];
    }
    let rough_step = range / target_count as f64;
    let mag = 10.0_f64.powf(rough_step.log10().floor());
    let norm = rough_step / mag;
    let nice_step = if norm <= 1.5 {
        1.0
    } else if norm <= 3.5 {
        2.0
    } else if norm <= 7.5 {
        5.0
    } else {
        10.0
    } * mag;

    let start = (min / nice_step).ceil() * nice_step;
    let mut ticks = Vec::new();
    let mut v = start;
    while v <= max {
        ticks.push(v);
        v += nice_step;
        if ticks.len() > 100 {
            break;
        }
    }
    ticks
}

// ---- CandlestickChart widget ----

#[derive(Script, ScriptHook, Widget)]
pub struct CandlestickChart {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    chart_view: ChartView,
    #[rust]
    data: FlatDataSource,
    #[rust]
    initialized: bool,
}

impl CandlestickChart {
    pub fn set_data(&mut self, candles: Vec<Candle>) {
        self.data = FlatDataSource::new(candles);
        self.initialized = false;
    }
}

impl Widget for CandlestickChart {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.chart_view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.initialized {
            if self.data.is_empty() {
                self.data = FlatDataSource::new(generate_fake_stock_data(500, 100.0));
            }
            self.chart_view.fit_data(&self.data);
            self.initialized = true;
        }

        // Auto-fit Y to visible data
        self.chart_view.fit_data_y(&self.data);

        // begin: draws bg (depth 0), starts draw_vector session (depth 2)
        self.chart_view.begin(cx, walk);

        let vp = self.chart_view.viewport().clone();

        // Grid lines + labels (depth 1 and 3) — interleaved freely
        let y_ticks = nice_ticks(vp.y_min, vp.y_max, 8);
        let y_labels: Vec<String> = y_ticks.iter().map(|v| format!("{:.2}", v)).collect();
        self.chart_view.draw_grid_y(cx, &y_ticks, &y_labels);

        let x_ticks = nice_ticks(vp.x_min, vp.x_max, 10);
        let x_labels: Vec<String> = x_ticks.iter().map(|v| format!("{}", *v as i64)).collect();
        self.chart_view.draw_grid_x(cx, &x_ticks, &x_labels);

        // Plot border (depth 1)
        self.chart_view.draw_plot_border(cx);

        // Visible data range
        let start_idx = (vp.x_min.floor() as isize - 1).max(0) as usize;
        let end_idx = (vp.x_max.ceil() as usize + 2).min(self.data.len());

        // Visible min/max
        let mut vis_high = f64::NEG_INFINITY;
        let mut vis_low = f64::INFINITY;
        if start_idx < end_idx {
            for c in self.data.get_range(start_idx, end_idx) {
                if c.high > vis_high {
                    vis_high = c.high;
                }
                if c.low < vis_low {
                    vis_low = c.low;
                }
            }
        }

        // Draw candles (depth 2 — all in one draw_vector session)
        let visible_count = end_idx.saturating_sub(start_idx);
        let plot_width = self.chart_view.plot_rect().size.x;
        let pixels_per_candle = if visible_count > 0 {
            plot_width / visible_count as f64
        } else {
            plot_width
        };

        if pixels_per_candle < 2.0 && visible_count > 0 {
            let bucket_size = (2.0 / pixels_per_candle).ceil() as usize;
            let averaged = self.data.get_averaged(start_idx, end_idx, bucket_size);
            let slot_width = bucket_size as f64;
            for candle in &averaged {
                self.chart_view.draw_candle(candle, slot_width);
            }
        } else if start_idx < end_idx {
            let candles = self.data.get_range(start_idx, end_idx);
            for candle in candles {
                self.chart_view.draw_candle(candle, 1.0);
            }
        }

        // Min/max lines (depth 1 + depth 3 labels)
        if vis_high.is_finite() && vis_low.is_finite() {
            let high_color = self.chart_view.high_line_color;
            let low_color = self.chart_view.low_line_color;
            self.chart_view.draw_hline_dashed(
                cx,
                vis_high,
                high_color,
                &format!("{:.2}", vis_high),
            );
            self.chart_view
                .draw_hline_dashed(cx, vis_low, low_color, &format!("{:.2}", vis_low));
        }

        // end: flushes draw_vector (depth 2)
        self.chart_view.end(cx);
        DrawStep::done()
    }
}

// ---- Fake point data generators ----

/// Generate a sine-wave-like data series with some noise.
pub fn generate_fake_line_data(count: usize) -> Vec<DataPoint> {
    let mut points = Vec::with_capacity(count);
    let mut seed: u64 = 54321;
    for i in 0..count {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let noise = (seed as f64 / u64::MAX as f64 - 0.5) * 10.0;
        let x = i as f64;
        let y = 50.0 + 30.0 * (x * 0.05).sin() + 15.0 * (x * 0.13).cos() + noise;
        points.push(DataPoint { x, y });
    }
    points
}

/// Generate bar chart data (positive values with some variation).
pub fn generate_fake_bar_data(count: usize) -> Vec<DataPoint> {
    let mut points = Vec::with_capacity(count);
    let mut seed: u64 = 67890;
    for i in 0..count {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let r = seed as f64 / u64::MAX as f64;
        let x = i as f64;
        let y = 20.0 + r * 80.0;
        points.push(DataPoint { x, y });
    }
    points
}

/// Generate scatter plot data (clustered with some spread).
pub fn generate_fake_scatter_data(count: usize) -> Vec<DataPoint> {
    let mut points = Vec::with_capacity(count);
    let mut seed: u64 = 11111;
    for _ in 0..count {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let r1 = seed as f64 / u64::MAX as f64;
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let r2 = seed as f64 / u64::MAX as f64;
        let x = r1 * 100.0;
        let y = x * 0.7 + (r2 - 0.5) * 40.0 + 10.0;
        points.push(DataPoint { x, y });
    }
    points
}

// ---- Helper: draw common grid/border for point-data charts ----

fn draw_point_chart_grid(chart_view: &mut ChartView, cx: &mut Cx2d, vp: &ChartViewport) {
    let y_ticks = nice_ticks(vp.y_min, vp.y_max, 8);
    let y_labels: Vec<String> = y_ticks.iter().map(|v| format!("{:.1}", v)).collect();
    chart_view.draw_grid_y(cx, &y_ticks, &y_labels);

    let x_ticks = nice_ticks(vp.x_min, vp.x_max, 10);
    let x_labels: Vec<String> = x_ticks.iter().map(|v| format!("{}", *v as i64)).collect();
    chart_view.draw_grid_x(cx, &x_ticks, &x_labels);

    chart_view.draw_plot_border(cx);
}

// ---- LineChart widget ----

#[derive(Script, ScriptHook, Widget)]
pub struct LineChart {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    chart_view: ChartView,
    #[rust]
    data: FlatPointData,
    #[rust]
    initialized: bool,
}

impl LineChart {
    pub fn set_data(&mut self, points: Vec<DataPoint>) {
        self.data = FlatPointData::new(points);
        self.initialized = false;
    }
}

impl Widget for LineChart {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.chart_view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.initialized {
            if self.data.point_is_empty() {
                self.data = FlatPointData::new(generate_fake_line_data(200));
            }
            self.chart_view.fit_point_data(&self.data);
            self.initialized = true;
        }

        self.chart_view.fit_point_data_y(&self.data);
        self.chart_view.begin(cx, walk);

        let vp = self.chart_view.viewport().clone();
        draw_point_chart_grid(&mut self.chart_view, cx, &vp);

        // Get visible points (with a bit of padding on each side for line continuity)
        let all = self.data.get_points(0, self.data.point_len());
        let visible: Vec<&DataPoint> = all
            .iter()
            .filter(|p| {
                p.x >= vp.x_min - vp.x_range() * 0.05 && p.x <= vp.x_max + vp.x_range() * 0.05
            })
            .collect();

        if visible.len() >= 2 {
            let pts: Vec<DataPoint> = visible.iter().map(|p| (*p).clone()).collect();
            let color = self.chart_view.line_color;
            let width = self.chart_view.line_width;
            self.chart_view.draw_line_series(&pts, color, width);
        }

        self.chart_view.end(cx);
        DrawStep::done()
    }
}

// ---- BarChart widget ----

#[derive(Script, ScriptHook, Widget)]
pub struct BarChart {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    chart_view: ChartView,
    #[rust]
    data: FlatPointData,
    #[rust]
    initialized: bool,
}

impl BarChart {
    pub fn set_data(&mut self, points: Vec<DataPoint>) {
        self.data = FlatPointData::new(points);
        self.initialized = false;
    }
}

impl Widget for BarChart {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.chart_view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.initialized {
            if self.data.point_is_empty() {
                self.data = FlatPointData::new(generate_fake_bar_data(30));
            }
            self.chart_view.fit_point_data(&self.data);
            // Include 0 in y-axis for bar charts
            if self.chart_view.viewport.y_min > 0.0 {
                self.chart_view.viewport.y_min = 0.0;
            }
            self.initialized = true;
        }

        self.chart_view.fit_point_data_y(&self.data);
        if self.chart_view.viewport.y_min > 0.0 {
            self.chart_view.viewport.y_min = 0.0;
        }

        self.chart_view.begin(cx, walk);

        let vp = self.chart_view.viewport().clone();
        draw_point_chart_grid(&mut self.chart_view, cx, &vp);

        let all = self.data.get_points(0, self.data.point_len());
        let bar_color = self.chart_view.bar_color;
        for p in all {
            if p.x >= vp.x_min - 1.0 && p.x <= vp.x_max + 1.0 {
                self.chart_view.draw_bar(p.x, p.y, 1.0, 0.0, bar_color);
            }
        }

        self.chart_view.end(cx);
        DrawStep::done()
    }
}

// ---- AreaChart widget ----

#[derive(Script, ScriptHook, Widget)]
pub struct AreaChart {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    chart_view: ChartView,
    #[rust]
    data: FlatPointData,
    #[rust]
    initialized: bool,
}

impl AreaChart {
    pub fn set_data(&mut self, points: Vec<DataPoint>) {
        self.data = FlatPointData::new(points);
        self.initialized = false;
    }
}

impl Widget for AreaChart {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.chart_view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.initialized {
            if self.data.point_is_empty() {
                self.data = FlatPointData::new(generate_fake_line_data(200));
            }
            self.chart_view.fit_point_data(&self.data);
            self.initialized = true;
        }

        self.chart_view.fit_point_data_y(&self.data);
        self.chart_view.begin(cx, walk);

        let vp = self.chart_view.viewport().clone();
        draw_point_chart_grid(&mut self.chart_view, cx, &vp);

        let all = self.data.get_points(0, self.data.point_len());
        let visible: Vec<DataPoint> = all
            .iter()
            .filter(|p| {
                p.x >= vp.x_min - vp.x_range() * 0.05 && p.x <= vp.x_max + vp.x_range() * 0.05
            })
            .cloned()
            .collect();

        if visible.len() >= 2 {
            // Filled area first (below line)
            let fill_color = self.chart_view.fill_color;
            self.chart_view
                .draw_filled_area(&visible, vp.y_min, fill_color);
            // Then the line on top
            let line_color = self.chart_view.line_color;
            let line_width = self.chart_view.line_width;
            self.chart_view
                .draw_line_series(&visible, line_color, line_width);
        }

        self.chart_view.end(cx);
        DrawStep::done()
    }
}

// ---- ScatterChart widget ----

#[derive(Script, ScriptHook, Widget)]
pub struct ScatterChart {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    chart_view: ChartView,
    #[rust]
    data: FlatPointData,
    #[rust]
    initialized: bool,
}

impl ScatterChart {
    pub fn set_data(&mut self, points: Vec<DataPoint>) {
        self.data = FlatPointData::new(points);
        self.initialized = false;
    }
}

impl Widget for ScatterChart {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.chart_view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.initialized {
            if self.data.point_is_empty() {
                self.data = FlatPointData::new(generate_fake_scatter_data(150));
            }
            self.chart_view.fit_point_data(&self.data);
            self.initialized = true;
        }

        self.chart_view.fit_point_data_y(&self.data);
        self.chart_view.begin(cx, walk);

        let vp = self.chart_view.viewport().clone();
        draw_point_chart_grid(&mut self.chart_view, cx, &vp);

        let all = self.data.get_points(0, self.data.point_len());
        let dot_color = self.chart_view.dot_color;
        let dot_radius = self.chart_view.dot_radius;
        for p in all {
            if p.x >= vp.x_min && p.x <= vp.x_max && p.y >= vp.y_min && p.y <= vp.y_max {
                self.chart_view.draw_dot(p.x, p.y, dot_radius, dot_color);
            }
        }

        self.chart_view.end(cx);
        DrawStep::done()
    }
}

// ---- OhlcChart widget ----

#[derive(Script, ScriptHook, Widget)]
pub struct OhlcChart {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    chart_view: ChartView,
    #[rust]
    data: FlatDataSource,
    #[rust]
    initialized: bool,
}

impl OhlcChart {
    pub fn set_data(&mut self, candles: Vec<Candle>) {
        self.data = FlatDataSource::new(candles);
        self.initialized = false;
    }
}

impl Widget for OhlcChart {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.chart_view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.initialized {
            if self.data.is_empty() {
                self.data = FlatDataSource::new(generate_fake_stock_data(500, 100.0));
            }
            self.chart_view.fit_data(&self.data);
            self.initialized = true;
        }

        self.chart_view.fit_data_y(&self.data);
        self.chart_view.begin(cx, walk);

        let vp = self.chart_view.viewport().clone();
        draw_point_chart_grid(&mut self.chart_view, cx, &vp);

        let start_idx = (vp.x_min.floor() as isize - 1).max(0) as usize;
        let end_idx = (vp.x_max.ceil() as usize + 2).min(self.data.len());

        if start_idx < end_idx {
            let candles = self.data.get_range(start_idx, end_idx);
            for candle in candles {
                self.chart_view.draw_ohlc(candle, 1.0);
            }
        }

        self.chart_view.end(cx);
        DrawStep::done()
    }
}

// ---- Sparkline widget ----

/// A tiny, dependency-free trend chart meant to live inside list rows and
/// [`crate::data_grid::DataGrid`] cells: value bars rising from the series
/// minimum, tinted by overall trend, drawn purely with batched quads so any
/// number of sparklines on screen coalesce into one draw call.
#[derive(Script, ScriptHook, Widget)]
pub struct Sparkline {
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
    draw_bar: DrawColor,
    #[live]
    pub color_up: Vec4f,
    #[live]
    pub color_down: Vec4f,
    /// Alpha of the bar body below the value cap.
    #[live(0.35)]
    pub baseline_alpha: f32,
    /// Horizontal gap between bars in pixels.
    #[live(1.0)]
    pub gap: f64,
    #[rust]
    values: Vec<f64>,
}

impl Sparkline {
    pub fn set_values(&mut self, values: &[f64]) {
        self.values.clear();
        self.values.extend_from_slice(values);
    }
}

impl Widget for Sparkline {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        let n = self.values.len();
        if n < 2 || rect.size.x <= 2.0 || rect.size.y <= 2.0 {
            return DrawStep::done();
        }
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for v in &self.values {
            min = min.min(*v);
            max = max.max(*v);
        }
        let range = (max - min).max(1e-9);
        let up = self.values[n - 1] >= self.values[0];
        let color = if up { self.color_up } else { self.color_down };
        let bw = rect.size.x / n as f64;
        let gap = self.gap.min(bw * 0.4);
        let h = rect.size.y;
        for (i, v) in self.values.iter().enumerate() {
            let t = (v - min) / range;
            let bar_h = (t * (h - 2.0)).max(1.0);
            let x = rect.pos.x + i as f64 * bw;
            let y = rect.pos.y + h - bar_h;
            // translucent body
            let mut body = color;
            body.w = self.baseline_alpha;
            self.draw_bar.color = body;
            self.draw_bar.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x, y),
                    size: dvec2((bw - gap).max(1.0), bar_h),
                },
            );
            // solid cap on the value
            self.draw_bar.color = color;
            self.draw_bar.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x, y),
                    size: dvec2((bw - gap).max(1.0), 2.0f64.min(bar_h)),
                },
            );
        }
        DrawStep::done()
    }
}

impl SparklineRef {
    pub fn set_values(&self, cx: &mut Cx, values: &[f64]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_values(values);
            inner.draw_bar.redraw(cx);
        }
    }
}

// ---- TrendChart: quad-based chart that composes anywhere ----

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawChartSegment {
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
    #[live]
    pub mode: f32,
    #[live(120.0)]
    pub fade_len: f32,
}

/// A self-contained series chart drawn entirely with instanced quads: one
/// or several SDF anti-aliased polylines with a gradient area fill, or
/// candlesticks, over a nice-tick grid with axis labels. No turtle
/// alignment, no vector paths — it renders correctly under any parent
/// (docks, grids, cells) and the whole chart batches into a handful of
/// draw calls.
///
/// # Several lines
///
/// The data is lines of markup, one per plotted line — `"cpu 32 35 41"`, a
/// name and then its numbers, read by [`parse_row`] exactly as the shapes
/// in [`crate::chart_shapes`] read theirs — or the same thing
/// from Rust through [`TrendChart::set_rows`]. [`TrendChart::set_series`]
/// is one line with no name, which is what it has always been. Every line
/// shares the one value axis and the one spacing along the bottom: the
/// longest line sets the spacing and a shorter one stops short, which is
/// the true picture of fewer samples so far.
///
/// The first line is drawn in `color_line` over `color_fill`, as it always
/// was. The second, third and fourth are `color_line_2`, `_3` and `_4`,
/// each over its own colour at `color_fill`'s opacity — so a fill made
/// transparent is transparent for every line — and a fifth starts round
/// again rather than fading past the fourth: two lines the same colour are
/// honestly ambiguous, where two nearly the same colour look
/// distinguishable and are not. With one line the last-value rule is
/// `color_accent`; with several it has to say WHICH line, so it takes the
/// line's colour.
///
/// # The key
///
/// `show_legend` draws a key at the top left of the plot — a swatch and a
/// name for every line that has a name — on a panel of the chart's own
/// background, so the lines do not run through the words. It is off unless
/// asked for. The library's position is that a legend is a list of labels
/// beside a picture, which is a layout decision belonging to whatever is
/// placing the chart, and it stands; the one exception is a tile with no
/// room beside it for anything, which is what this widget is for. The
/// panel is as wide as its names measure in the face they are drawn in,
/// not as wide as a count of characters guesses.
///
/// # A pinned axis
///
/// The value axis fits the data on every draw, with a little room above
/// and below. `range_max` above `range_min` pins it to exactly that span
/// instead, so two tiles side by side, or one tile frame after frame, put
/// the same value at the same height and compare. A value beyond a pinned
/// axis is drawn pegged to the edge it left by rather than off the chart.
#[derive(Script, ScriptHook, Widget)]
pub struct TrendChart {
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
    draw_bg: DrawColor,
    #[live]
    draw_grid: DrawColor,
    #[live]
    draw_seg: DrawChartSegment,
    #[live]
    draw_candle: DrawColor,
    #[live]
    draw_text: DrawText,

    #[live]
    pub color_bg: Vec4f,
    #[live]
    pub color_grid: Vec4f,
    /// The first line, over `color_fill` as written.
    #[live]
    pub color_line: Vec4f,
    /// The second, third and fourth lines, each over its own colour at
    /// `color_fill`'s opacity. A fifth line is `color_line` again.
    #[live]
    pub color_line_2: Vec4f,
    #[live]
    pub color_line_3: Vec4f,
    #[live]
    pub color_line_4: Vec4f,
    #[live]
    pub color_fill: Vec4f,
    #[live]
    pub color_up: Vec4f,
    #[live]
    pub color_down: Vec4f,
    #[live]
    pub color_text: Vec4f,
    #[live]
    pub color_accent: Vec4f,
    #[live(2.0)]
    pub line_width: f64,
    /// The key: a swatch and a name per named line, at the top left of the
    /// plot. Off unless asked for.
    #[live]
    pub show_legend: bool,
    /// The value axis, pinned to exactly this span while `range_max` is
    /// above `range_min`; otherwise the axis fits the data on every draw.
    #[live]
    pub range_min: f64,
    #[live]
    pub range_max: f64,
    /// One line of markup per plotted line: a name, then its numbers. See
    /// [`parse_row`].
    #[live]
    pub series: Vec<String>,
    /// Compact history when the surrounding UI already displays its value.
    #[live]
    pub sparkline: bool,

    #[rust]
    rows: Vec<Row>,
    #[rust]
    seeded_from: Vec<String>,
    #[rust]
    candles: Vec<Candle>,
}

/// Room the fitted axis leaves above and below the data, as a share of its
/// span, so a line does not run along the frame.
const AXIS_ROOM: f64 = 0.08;

/// The value axis: `pin_min..pin_max` exactly while the top is above the
/// bottom, else the data's `lo..hi` with [`AXIS_ROOM`] added each side. A
/// pin gets no room added, because it is what a host asked for: two tiles
/// pinned to the same numbers have to put the same value at the same
/// height, or they do not compare.
pub fn axis_range(lo: f64, hi: f64, pin_min: f64, pin_max: f64) -> (f64, f64) {
    if pin_max > pin_min {
        return (pin_min, pin_max);
    }
    // A flat line still gets an axis, and one in proportion to its numbers:
    // a fixed floor under a million is smaller than the difference between
    // two neighbouring f64s there, and a tick step made from it never
    // advanced.
    let floor = (hi.abs().max(lo.abs()) * 1e-6).max(1e-9);
    let room = (hi - lo).max(floor) * AXIS_ROOM;
    (lo - room, hi + room)
}

/// At most this many grid lines on the value axis: a step that cannot
/// advance a tick stops here rather than never.
const MAX_TICKS: usize = 64;

/// The ticks of the value axis: multiples of `step` from the first at or
/// above `min` up to and including `max`, so an axis pinned to a round
/// number labels its own ceiling; a fitted axis pads its ends and never
/// lands a tick on them.
pub fn axis_ticks(min: f64, max: f64, step: f64) -> Vec<f64> {
    let mut out = Vec::new();
    if !(step > 0.0) || !min.is_finite() || !max.is_finite() {
        return out;
    }
    let first = (min / step).ceil();
    for n in 0..MAX_TICKS {
        let tick = (first + n as f64) * step;
        if tick > max + step * 1e-6 {
            break;
        }
        out.push(tick);
    }
    out
}

/// The least and greatest number in the rows that can be drawn, and how
/// many points the longest of them has. A row with fewer than two numbers
/// has no line, and no say in the axis either. `None` when no row can be
/// drawn.
pub fn rows_extent(rows: &[Row]) -> Option<(f64, f64, usize)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut longest = 0;
    for row in rows.iter().filter(|row| row.values.len() >= 2) {
        for v in &row.values {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
        longest = longest.max(row.values.len());
    }
    (longest >= 2).then(|| (lo, hi, longest))
}

/// The key's measures, in layout points: the panel's inset, the swatch's
/// side, the gap from a swatch to its name, and the gap from one entry to
/// the next.
const KEY_PAD: f64 = 6.0;
const KEY_SWATCH: f64 = 8.0;
const KEY_SWATCH_GAP: f64 = 5.0;
const KEY_ENTRY_GAP: f64 = 12.0;
/// The box a line of the chart's text is drawn in, in font sizes; the
/// gutter's labels sit six points above their tick for the same reason.
const TEXT_BOX: f64 = 1.5;

/// Where the parts of a key go: the panel behind it, a swatch per entry
/// and the top-left corner of each name.
#[derive(Clone, Debug, PartialEq)]
pub struct LegendRow {
    pub panel: Rect,
    pub swatches: Vec<Rect>,
    pub names: Vec<DVec2>,
}

/// Lay a key out from `origin`, its panel's top-left corner: one entry per
/// name, in a row, given the MEASURED width of each name and the height of
/// a line of text. The widths have to come from the face the names are
/// drawn in — a key sized by a count of characters is the right width for
/// exactly one font, and clips or gapes in every other.
pub fn legend_row(origin: DVec2, widths: &[f64], text_h: f64) -> LegendRow {
    let mut swatches = Vec::with_capacity(widths.len());
    let mut names = Vec::with_capacity(widths.len());
    let mut x = origin.x + KEY_PAD;
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            x += KEY_ENTRY_GAP;
        }
        swatches.push(Rect {
            pos: dvec2(x, origin.y + KEY_PAD + (text_h - KEY_SWATCH) * 0.5),
            size: dvec2(KEY_SWATCH, KEY_SWATCH),
        });
        x += KEY_SWATCH + KEY_SWATCH_GAP;
        names.push(dvec2(x, origin.y + KEY_PAD));
        x += w;
    }
    let panel = if widths.is_empty() {
        Rect { pos: origin, size: dvec2(0.0, 0.0) }
    } else {
        Rect { pos: origin, size: dvec2(x + KEY_PAD - origin.x, text_h + KEY_PAD * 2.0) }
    };
    LegendRow { panel, swatches, names }
}

impl TrendChart {
    /// One line with no name. Clears any candles.
    pub fn set_series(&mut self, values: &[f64]) {
        self.set_rows(vec![Row::new("", values)]);
    }

    /// The lines from Rust instead of from markup, each a name and its
    /// numbers, in the order they are drawn and keyed. The markup lines are
    /// marked as already read, so the next draw does not put them back.
    /// Clears any candles.
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.seeded_from = self.series.clone();
        self.candles.clear();
    }

    /// Candles instead of lines. Clears the lines, markup ones included.
    pub fn set_candles(&mut self, candles: Vec<Candle>) {
        self.candles = candles;
        self.rows.clear();
        self.seeded_from = self.series.clone();
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn candles(&self) -> &[Candle] {
        &self.candles
    }

    /// The colour line `i` is drawn in, the four going round again after
    /// the fourth.
    pub fn line_color(&self, i: usize) -> Vec4f {
        match i % 4 {
            0 => self.color_line,
            1 => self.color_line_2,
            2 => self.color_line_3,
            _ => self.color_line_4,
        }
    }

    /// Re-read the markup lines if they have changed since the last draw.
    fn sync(&mut self) {
        if self.seeded_from != self.series {
            self.seeded_from = self.series.clone();
            self.rows = parse_rows(&self.series);
            // The markup is lines; candles that were set from Rust would
            // otherwise keep the tile, drawn ahead of them.
            self.candles.clear();
        }
    }

    fn nice_step(raw: f64) -> f64 {
        let mag = 10f64.powf(raw.abs().max(1e-12).log10().floor());
        let n = raw / mag;
        let nice = if n < 1.5 {
            1.0
        } else if n < 3.0 {
            2.0
        } else if n < 7.0 {
            5.0
        } else {
            10.0
        };
        nice * mag
    }

    /// The key, on a panel of the chart's background at the top left of
    /// `plot`: a swatch in each named line's colour, then its name. A named
    /// line with no numbers yet is still in the key — it is a line the host
    /// wrote down.
    fn draw_key(&mut self, cx: &mut Cx2d, plot: Rect) {
        let named: Vec<usize> = (0..self.rows.len()).filter(|&r| !self.rows[r].label.is_empty()).collect();
        if named.is_empty() {
            return;
        }
        let widths: Vec<f64> = named.iter().map(|&r| measure(&self.draw_text, cx, &self.rows[r].label)).collect();
        let text_h = self.draw_text.text_style.font_size as f64 * TEXT_BOX;
        let key = legend_row(plot.pos + dvec2(6.0, 6.0), &widths, text_h);
        // The panel goes through the grid's quad, not the tile's: the
        // tile's is the widget's area, and the last rect it draws is what
        // the overlay and the tree would take for the whole chart.
        self.draw_grid.color = self.color_bg;
        self.draw_grid.draw_abs(cx, key.panel);
        self.draw_text.color = self.color_text;
        for (k, &r) in named.iter().enumerate() {
            self.draw_grid.color = self.line_color(r);
            self.draw_grid.draw_abs(cx, key.swatches[k]);
            self.draw_text.draw_abs(cx, key.names[k], &self.rows[r].label);
        }
    }
}

impl Widget for TrendChart {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.sync();
        let rect = cx.walk_turtle(walk);
        self.draw_bg.color = self.color_bg;
        self.draw_bg.draw_abs(cx, rect);

        // data range: the candles when there are any, else every line
        let (lo, hi, n) = if !self.candles.is_empty() {
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            for c in &self.candles {
                lo = lo.min(c.low);
                hi = hi.max(c.high);
            }
            (lo, hi, self.candles.len())
        } else {
            rows_extent(&self.rows).unwrap_or((0.0, 0.0, 0))
        };
        if n < 2 || rect.size.x < (if self.sparkline { 12.0 } else { 80.0 })
            || rect.size.y < (if self.sparkline { 8.0 } else { 60.0 }) {
            return DrawStep::done();
        }
        let (min, max) = axis_range(lo, hi, self.range_min, self.range_max);
        let range = max - min;

        let gutter = 54.0;
        let plot = if self.sparkline {
            Rect { pos: rect.pos + dvec2(2.0, 2.0), size: rect.size - dvec2(4.0, 4.0) }
        } else {
            Rect { pos: rect.pos + dvec2(10.0, 8.0), size: rect.size - dvec2(gutter + 18.0, 18.0) }
        };
        // a value beyond a pinned axis is pegged to the edge it left by
        let py = |v: f64| plot.pos.y + (1.0 - ((v - min) / range).clamp(0.0, 1.0)) * plot.size.y;

        if !self.sparkline {
            // horizontal grid at nice ticks, labels in the right gutter
            let step = Self::nice_step(range / 5.0);
            self.draw_grid.color = self.color_grid;
            self.draw_text.color = self.color_text;
            for tick in axis_ticks(min, max, step) {
                let y = py(tick);
                self.draw_grid.draw_abs(cx, Rect {
                    pos: dvec2(plot.pos.x, y),
                    size: dvec2(plot.size.x, 1.0),
                });
                let label = if step >= 1.0 {
                    format!("{:.0}", tick)
                } else {
                    format!("{:.2}", tick)
                };
                self.draw_text
                    .draw_abs(cx, dvec2(plot.pos.x + plot.size.x + 6.0, y - 6.0), &label);
            }
            // vertical grid every ~90px
            let vticks = (plot.size.x / 90.0).max(1.0) as usize;
            for i in 1..=vticks {
                let x = plot.pos.x + plot.size.x * i as f64 / vticks as f64;
                self.draw_grid.draw_abs(cx, Rect {
                    pos: dvec2(x, plot.pos.y),
                    size: dvec2(1.0, plot.size.y),
                });
            }
        }
        if !self.candles.is_empty() {
            let slot = plot.size.x / n as f64;
            let bw = (slot * 0.62).max(1.0);
            for (i, c) in self.candles.iter().enumerate() {
                let x = plot.pos.x + (i as f64 + 0.5) * slot;
                let up = c.close >= c.open;
                let color = if up { self.color_up } else { self.color_down };
                self.draw_candle.color = color;
                // wick
                self.draw_candle.draw_abs(cx, Rect {
                    pos: dvec2(x - 0.5, py(c.high)),
                    size: dvec2(1.0, (py(c.low) - py(c.high)).max(1.0)),
                });
                // body
                let (top, bottom) = if up {
                    (py(c.close), py(c.open))
                } else {
                    (py(c.open), py(c.close))
                };
                self.draw_candle.draw_abs(cx, Rect {
                    pos: dvec2(x - bw * 0.5, top),
                    size: dvec2(bw, (bottom - top).max(1.0)),
                });
            }
        } else {
            // the longest line sets the spacing; a shorter one stops short
            let dx = plot.size.x / (n - 1) as f64;
            let count = self.rows.len();
            let several = self.rows.iter().filter(|row| row.values.len() >= 2).count() > 1;
            let colors: Vec<Vec4f> = (0..count).map(|r| self.line_color(r)).collect();
            // area fills: one column quad per segment, shader fades below the line
            self.draw_seg.mode = 1.0;
            self.draw_seg.fade_len = (plot.size.y * 0.85) as f32;
            for r in 0..count {
                let values = &self.rows[r].values;
                if values.len() < 2 {
                    continue;
                }
                self.draw_seg.color = if r == 0 {
                    self.color_fill
                } else {
                    Vec4f { w: self.color_fill.w, ..colors[r] }
                };
                for i in 0..values.len() - 1 {
                    let x0 = plot.pos.x + i as f64 * dx;
                    let y0 = py(values[i]);
                    let y1 = py(values[i + 1]);
                    let top = y0.min(y1);
                    let quad = Rect {
                        pos: dvec2(x0, top),
                        size: dvec2(dx, plot.pos.y + plot.size.y - top),
                    };
                    self.draw_seg.seg_a = Vec2f {
                        x: 0.0,
                        y: (y0 - top) as f32,
                    };
                    self.draw_seg.seg_b = Vec2f {
                        x: quad.size.x as f32,
                        y: (y1 - top) as f32,
                    };
                    self.draw_seg.draw_abs(cx, quad);
                }
            }
            // anti-aliased polylines on top of every fill
            self.draw_seg.mode = 0.0;
            self.draw_seg.thickness = self.line_width as f32;
            let m = self.line_width + 2.0;
            for r in 0..count {
                let values = &self.rows[r].values;
                if values.len() < 2 {
                    continue;
                }
                self.draw_seg.color = colors[r];
                for i in 0..values.len() - 1 {
                    let x0 = plot.pos.x + i as f64 * dx;
                    let x1 = x0 + dx;
                    let y0 = py(values[i]);
                    let y1 = py(values[i + 1]);
                    let top = y0.min(y1) - m;
                    let bottom = y0.max(y1) + m;
                    let quad = Rect {
                        pos: dvec2(x0 - m, top),
                        size: dvec2(x1 - x0 + 2.0 * m, bottom - top),
                    };
                    self.draw_seg.seg_a = Vec2f {
                        x: (x0 - quad.pos.x) as f32,
                        y: (y0 - quad.pos.y) as f32,
                    };
                    self.draw_seg.seg_b = Vec2f {
                        x: (x1 - quad.pos.x) as f32,
                        y: (y1 - quad.pos.y) as f32,
                    };
                    self.draw_seg.draw_abs(cx, quad);
                }
            }
            // last-value rule and label per line: the accent when there is
            // one line, the line's own colour when it has to say which
            if !self.sparkline {
                for r in 0..count {
                    let values = &self.rows[r].values;
                    if values.len() < 2 {
                        continue;
                    }
                    let last = *values.last().unwrap();
                    let color = if several { colors[r] } else { self.color_accent };
                    let y = py(last);
                    self.draw_grid.color = color;
                    let mut x = plot.pos.x;
                    while x < plot.pos.x + plot.size.x {
                        self.draw_grid.draw_abs(cx, Rect {
                            pos: dvec2(x, y),
                            size: dvec2(4.0, 1.0),
                        });
                        x += 8.0;
                    }
                    self.draw_text.color = color;
                    self.draw_text.draw_abs(
                        cx,
                        dvec2(plot.pos.x + plot.size.x + 6.0, y - 6.0),
                        &format!("{:.2}", last),
                    );
                }
            }
        }
        if self.show_legend {
            self.draw_key(cx, plot);
        }
        DrawStep::done()
    }
}

impl TrendChartRef {
    pub fn set_series(&self, cx: &mut Cx, values: &[f64]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_series(values);
            inner.draw_bg.redraw(cx);
        }
    }

    pub fn set_rows(&self, cx: &mut Cx, rows: Vec<Row>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_rows(rows);
            inner.draw_bg.redraw(cx);
        }
    }

    pub fn set_candles(&self, cx: &mut Cx, candles: Vec<Candle>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_candles(candles);
            inner.draw_bg.redraw(cx);
        }
    }

    /// Pin the value axis to `min..max`. A top not above its bottom lets
    /// the axis fit the data again.
    pub fn pin_range(&self, cx: &mut Cx, min: f64, max: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.range_min = min;
            inner.range_max = max;
            inner.draw_bg.redraw(cx);
        }
    }
}

/// Whether a press may start the chart's pan.
///
/// Panning is a gesture the chart starts from a press that is not
/// necessarily its own: the hit is taken with `capture_overload`, so the
/// chart is handed the `FingerDown` for every press inside it, including one
/// a child control already captured. So it has to decide, and the rule it
/// decides by is the app-wide one — a control that is dragged continuously
/// locks the pointer, and any other gesture that would start from the same
/// press stands down until the release.
///
/// `mouse_held_outside` is [`CxFingers::is_mouse_held_outside`] asked with
/// the chart's own area: true means a slider, fader, scroll bar or resizer
/// owns the mouse right now, and this press is that control's. A TOUCH
/// capture answers `false` there by design, so a finger that lands on a
/// control may still pan the chart under it, the way it may still drag a
/// list — which is why no touch exemption is spelled out here.
fn press_starts_pan(is_primary_hit: bool, mouse_held_outside: bool) -> bool {
    is_primary_hit && !mouse_held_outside
}

/// Whether a pan already under way must stand down on this move.
///
/// The press and a child's capture can land in either order inside one
/// event, and a control can take the pointer AFTER the pan began, so the
/// question is re-asked on every move rather than only at the press.
fn pan_stands_down(mouse_held_outside: bool) -> bool {
    mouse_held_outside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// The operator's rule, on the chart's own gesture: a press the pointer
    /// is already locked to must not also pan the chart under it.
    ///
    /// `capture_overload` is what makes this reachable — a press a child
    /// slider captured is handed to the chart as well, and without the
    /// question being asked the chart would pan through the whole of that
    /// control's drag.
    #[test]
    fn a_press_another_control_holds_never_starts_a_pan() {
        assert!(
            press_starts_pan(true, false),
            "an ordinary press on the chart pans it"
        );
        assert!(
            !press_starts_pan(true, true),
            "a press the pointer is locked to belongs to whatever holds it"
        );
        // A secondary button is not a pan on an unheld pointer either.
        assert!(!press_starts_pan(false, false));
        assert!(!press_starts_pan(false, true));
    }

    /// And the other half, asked again on every move: a control that takes
    /// the pointer after the pan began ends the pan there and then.
    #[test]
    fn a_pan_stands_down_the_move_a_control_takes_the_mouse() {
        assert!(pan_stands_down(true));
        assert!(!pan_stands_down(false), "an unheld pointer keeps panning");
    }

    #[test]
    fn several_lines_share_one_axis_that_covers_them_all() {
        let rows = vec![Row::new("cpu", &[30.0, 45.0, 40.0]), Row::new("memory", &[60.0, 62.0, 70.0, 71.0])];
        let (lo, hi, longest) = rows_extent(&rows).unwrap();
        // The least of one line and the greatest of the other, not the
        // first line's own span: a second line drawn on the first line's
        // axis would leave the frame the moment it was higher.
        assert_eq!((lo, hi), (30.0, 71.0));
        // The longest line sets the spacing along the bottom.
        assert_eq!(longest, 4);
        // And the fitted axis leaves room each side, so neither line runs
        // along the frame.
        let (min, max) = axis_range(lo, hi, 0.0, 0.0);
        assert!(min < lo && max > hi);
        assert!(close(max - min, 41.0 * (1.0 + 2.0 * AXIS_ROOM)));
    }

    #[test]
    fn a_line_of_one_number_is_not_a_line_and_has_no_say_in_the_axis() {
        let rows = vec![Row::new("cpu", &[30.0, 45.0]), Row::new("waiting", &[900.0])];
        let (lo, hi, longest) = rows_extent(&rows).unwrap();
        assert_eq!((lo, hi, longest), (30.0, 45.0, 2));
        // And when no row has two numbers there is nothing to draw at all,
        // which is an answer and not a division by zero.
        assert_eq!(rows_extent(&[Row::new("waiting", &[900.0])]), None);
        assert_eq!(rows_extent(&[]), None);
    }

    #[test]
    fn a_pinned_axis_is_exactly_the_pin_whatever_the_data_does() {
        // The same pin over two frames of different data is the same axis,
        // which is the whole point of pinning: the two frames compare.
        assert_eq!(axis_range(30.0, 45.0, 0.0, 100.0), (0.0, 100.0));
        assert_eq!(axis_range(12.0, 240.0, 0.0, 100.0), (0.0, 100.0));
        // No room is added round a pin. The host asked for these edges.
        assert_eq!(axis_range(50.0, 50.0, 20.0, 80.0), (20.0, 80.0));
        // A top not above its bottom is no pin: the axis fits the data.
        let fitted = axis_range(30.0, 45.0, 0.0, 0.0);
        assert_eq!(axis_range(30.0, 45.0, 100.0, 100.0), fitted);
        assert_eq!(axis_range(30.0, 45.0, 100.0, 0.0), fitted);
    }

    #[test]
    fn a_key_is_as_wide_as_its_names_measure_and_no_wider() {
        let origin = dvec2(100.0, 50.0);
        let text_h = 12.0;
        // Three names of very different measured widths — the kind of
        // spread a proportional face gives "cpu", "memory" and "disk io".
        let widths = [14.0, 61.0, 33.0];
        let key = legend_row(origin, &widths, text_h);
        assert_eq!(key.swatches.len(), 3);
        assert_eq!(key.names.len(), 3);
        assert_eq!(key.panel.pos, origin);
        // The panel is the inset, then per entry a swatch, its gap and the
        // name's own width, with a gap between entries. Nothing in it is
        // a count of characters.
        let entries: f64 = widths.iter().map(|w| KEY_SWATCH + KEY_SWATCH_GAP + w).sum();
        let expect_w = KEY_PAD * 2.0 + entries + KEY_ENTRY_GAP * 2.0;
        assert!(close(key.panel.size.x, expect_w));
        assert!(close(key.panel.size.y, text_h + KEY_PAD * 2.0));
        // Each name starts where the swatch before it ends, and each entry
        // starts where the measured name before it ended plus the gap: the
        // second entry is 61 wide apart from the third, not 33 apart.
        for k in 0..3 {
            assert!(close(key.names[k].x, key.swatches[k].pos.x + KEY_SWATCH + KEY_SWATCH_GAP));
            assert!(close(key.names[k].y, origin.y + KEY_PAD));
        }
        assert!(close(key.swatches[1].pos.x, key.names[0].x + widths[0] + KEY_ENTRY_GAP));
        assert!(close(key.swatches[2].pos.x, key.names[1].x + widths[1] + KEY_ENTRY_GAP));
        // The swatch sits in the middle of the text's box.
        let swatch_mid = key.swatches[0].pos.y + KEY_SWATCH * 0.5;
        assert!(close(swatch_mid, origin.y + KEY_PAD + text_h * 0.5));
        // The last name ends one inset short of the panel's right edge.
        let last_end = key.names[2].x + widths[2];
        assert!(close(last_end + KEY_PAD, origin.x + key.panel.size.x));
    }

    #[test]
    fn a_key_with_no_names_takes_no_room() {
        let key = legend_row(dvec2(10.0, 10.0), &[], 12.0);
        assert!(key.swatches.is_empty());
        assert!(key.names.is_empty());
        assert_eq!(key.panel.size, dvec2(0.0, 0.0));
    }

    /// A TrendChart as `TrendChart{}` builds one: the type default applied
    /// through the real machinery, which is exactly what a page gets.
    fn declared() -> (Cx, WidgetRef) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let chart = cx.with_vm(|vm| {
            let widgets = vm.module(id!(widgets));
            let value = vm.bx.heap.value(widgets, LiveId::from_str("TrendChart").into(), NoTrap);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!chart.is_empty(), "TrendChart built no widget");
        (cx, chart)
    }

    /// Any widget of the library, built as a page builds it.
    fn declared_as(cx: &mut Cx, name: &str) -> WidgetRef {
        let widget = cx.with_vm(|vm| {
            let widgets = vm.module(id!(widgets));
            let value = vm.bx.heap.value(widgets, LiveId::from_str(name).into(), NoTrap);
            WidgetRef::script_from_value(vm, value)
        });
        assert!(!widget.is_empty(), "{name} built no widget");
        widget
    }

    /// The tile's text is drawn in the face the rest of the library draws
    /// in, at the tile's size. A style that only merged a size into the
    /// raw default was left with a family of one member that named no
    /// file: its words measured, and none of them drew.
    #[test]
    fn as_declared_the_text_is_in_the_library_face() {
        let (mut cx, chart) = declared();
        let label = declared_as(&mut cx, "Label");
        let chart = chart.borrow::<TrendChart>().unwrap();
        let label = label.borrow::<crate::Label>().unwrap();
        let (tile, library) = (&chart.draw_text.text_style, &label.draw_text.text_style);
        assert_eq!(
            tile.font_family.member_ids().collect::<Vec<_>>(),
            library.font_family.member_ids().collect::<Vec<_>>(),
            "the tile's face is not the library's"
        );
        assert_eq!(tile.font_size, 8.0);
    }

    #[test]
    fn a_pinned_axis_labels_its_ceiling_and_a_fitted_one_never_lands_on_its_ends() {
        assert_eq!(axis_ticks(0.0, 100.0, 20.0), vec![0.0, 20.0, 40.0, 60.0, 80.0, 100.0]);
        let (min, max) = axis_range(0.0, 100.0, 0.0, 0.0);
        let ticks = axis_ticks(min, max, TrendChart::nice_step((max - min) / 5.0));
        assert!(ticks.first().copied().unwrap() > min);
        assert!(ticks.last().copied().unwrap() < max);
    }

    /// A flat line at a million: the axis has room in proportion to the
    /// number, and the ticks advance and end.
    #[test]
    fn a_flat_series_of_large_numbers_has_an_axis_and_a_finite_number_of_ticks() {
        let (min, max) = axis_range(1_048_576.0, 1_048_576.0, 0.0, 0.0);
        assert!(max > min);
        let ticks = axis_ticks(min, max, TrendChart::nice_step((max - min) / 5.0));
        assert!(!ticks.is_empty() && ticks.len() <= MAX_TICKS);
        assert!(ticks.windows(2).all(|w| w[1] > w[0]), "the ticks do not advance: {ticks:?}");
        // A step below what an f64 can add to the number still ends.
        assert!(axis_ticks(1e6, 1e6 + 2e-10, 5e-11).len() <= MAX_TICKS);
        assert!(axis_ticks(0.0, 1.0, 0.0).is_empty());
    }

    #[test]
    fn as_declared_the_key_is_off_the_axis_fits_and_the_lines_have_four_colours() {
        let (_cx, chart) = declared();
        let chart = chart.borrow::<TrendChart>().unwrap();
        assert!(!chart.show_legend, "a legend is the placer's decision unless the tile asks");
        assert!(chart.range_max <= chart.range_min, "no pin: the axis fits the data");
        assert!(chart.series.is_empty() && chart.rows().is_empty(), "no line was invented");
        // Four colours, no two alike, and the fifth line is the first's.
        let colors: Vec<Vec4f> = (0..4).map(|i| chart.line_color(i)).collect();
        for a in 0..4 {
            for b in 0..a {
                assert_ne!(colors[a], colors[b], "lines {} and {} share a colour", a, b);
            }
        }
        assert_eq!(chart.line_color(0), chart.color_line, "the first line is what it always was");
        assert_eq!(chart.line_color(4), chart.line_color(0));
        assert_eq!(chart.line_color(5), chart.line_color(1));
    }

    #[test]
    fn set_series_is_one_unnamed_line_and_lines_and_candles_replace_each_other() {
        let (_cx, chart) = declared();
        let mut chart = chart.borrow_mut::<TrendChart>().unwrap();
        chart.set_series(&[1.0, 2.0, 3.0]);
        assert_eq!(chart.rows(), &[Row::new("", &[1.0, 2.0, 3.0])]);
        chart.set_rows(vec![Row::new("cpu", &[1.0, 2.0]), Row::new("memory", &[3.0, 4.0])]);
        assert_eq!(chart.rows().len(), 2);
        assert_eq!(chart.rows()[1].label, "memory");
        // The chart draws lines or candles over its one axis, never both.
        chart.set_candles(vec![Candle { time: 0.0, open: 1.0, high: 2.0, low: 0.5, close: 1.5, volume: 0.0 }]);
        assert!(chart.rows().is_empty());
        assert_eq!(chart.candles().len(), 1);
        chart.set_series(&[4.0, 5.0]);
        assert_eq!(chart.rows().len(), 1);
        assert!(chart.candles().is_empty());
    }

    #[test]
    fn a_line_is_read_as_a_name_and_the_numbers_after_it() {
        assert_eq!(parse_row("Rent 420"), Row::new("Rent", &[420.0]));
        assert_eq!(
            parse_row("Public transport 120"),
            Row::new("Public transport", &[120.0]),
            "a name may be several words"
        );
        assert_eq!(
            parse_row("Q1 2024 480"),
            Row::new("Q1", &[2024.0, 480.0]),
            "the numbers are the whole trailing run"
        );
        assert_eq!(
            parse_row("Speed: 4 3 5 2"),
            Row::new("Speed", &[4.0, 3.0, 5.0, 2.0]),
            "a colon says where the name stops"
        );
        assert_eq!(
            parse_row("Q1 2024: 480"),
            Row::new("Q1 2024", &[480.0]),
            "which is how a name keeps a number of its own"
        );
    }

    #[test]
    fn a_line_with_no_number_is_a_part_worth_nothing_and_not_a_broken_one() {
        let row = parse_row("Nothing here");
        assert_eq!(row.label, "Nothing here");
        assert!(row.values.is_empty());
        assert_eq!(row.value(), 0.0, "which a chart can draw");
        assert_eq!(row.value_at(3), 0.0);
        assert_eq!(parse_row(""), Row::default());
        // A run of numbers with no name is a nameless part, not a name.
        assert_eq!(parse_row("12 40 8"), Row::new("", &[12.0, 40.0, 8.0]));
    }

    #[test]
    fn a_whole_number_is_written_without_a_decimal_point() {
        assert_eq!(fmt_value(420.0), "420");
        assert_eq!(fmt_value(0.0), "0");
        assert_eq!(fmt_value(-7.0), "-7");
        assert_eq!(fmt_value(3.14), "3.1");
    }
}
