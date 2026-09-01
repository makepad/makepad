//! The charts, drawn as shaders.
//!
//! One widget, three forms, because a finance dashboard only ever needs
//! three: a series over time (net worth, balance), a comparison across a
//! handful of periods (income against spending), and the same series
//! shrunk into a table row (a sparkline). Each is a handful of instanced
//! quads with an SDF in the pixel shader — no geometry pass, no texture,
//! and the whole chart batches into a few draw calls, which is why a
//! sparkline per row of a scrolling ledger costs nothing.
//!
//! The visual rules come from the dataviz guidance and are deliberate:
//!
//! * **No gridlines by default.** A line over a dark surface reads on its
//!   own; a grid competes with it. Two faint rules mark the extremes, and
//!   that is all the scale anyone reads off a trend.
//! * **Selective labels.** The first, last and extreme values are labelled
//!   — never every point.
//! * **The fill is a gradient to nothing.** A flat fill under a line reads
//!   as a solid shape and hides the line; the fade keeps the line the
//!   subject.
//! * **Bars sit on the baseline with a rounded top and a 2px gap.** The
//!   gap is the surface showing through, which is what separates adjacent
//!   bars without a stroke around each one.

use makepad_widgets::*;

/// The area under a series: one quad per sample interval, each shading the
/// slice between the curve and the baseline.
///
/// The top edge is interpolated ACROSS the quad (`top_left` → `top_right`),
/// so a slice is a trapezoid rather than a staircase, and the edge is
/// antialiased against the fill rather than left to the rasteriser.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAreaFill {
    #[deref]
    draw_super: DrawQuad,
    /// Height of the curve at this quad's left edge, 0 = top of the plot.
    #[live]
    pub top_left: f32,
    #[live]
    pub top_right: f32,
    /// Colour at the curve, fading to fully transparent at the baseline.
    #[live]
    pub color_top: Vec4f,
    /// How far down the fade reaches: 1.0 fades across the whole plot.
    #[live(1.0)]
    pub fade: f32,
}

/// One segment of the line, as an SDF capsule so the joins are round and
/// the edges are antialiased at any angle.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLineSeg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub y0: f32,
    #[live]
    pub y1: f32,
    #[live]
    pub color_line: Vec4f,
    #[live(2.0)]
    pub thickness: f32,
    /// A soft outer glow, which is what stops a 2px line looking thin on a
    /// dark surface without having to thicken it.
    #[live(0.0)]
    pub glow: f32,
}

/// A bar, anchored to the baseline with a rounded top.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawBar {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color_bar: Vec4f,
    /// 0 at the baseline, 1 at the top of the plot.
    #[live]
    pub height_frac: f32,
    #[live(4.0)]
    pub radius: f32,
    /// Bars that hang below the baseline round the other way.
    #[live(0.0)]
    pub downward: f32,
}

/// A proportion bar — track and fill in one quad.
///
/// Drawn rather than laid out on purpose: a fill sized by layout needs the
/// track's measured width, which does not exist until after a layout pass,
/// so the first frame draws empty bars and every resize needs a re-measure.
/// A shader that takes the fraction as an instance has neither problem, and
/// it is one quad instead of three widgets per row.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMeter {
    #[deref]
    draw_super: DrawQuad,
    /// 0..1 of the track.
    #[live]
    pub fraction: f32,
    #[live]
    pub color_track: Vec4f,
    #[live]
    pub color_fill: Vec4f,
    /// A second, dimmer mark on the same track — what was budgeted, or the
    /// same period last year. Negative hides it.
    #[live(-1.0)]
    pub marker: f32,
    #[live]
    pub color_marker: Vec4f,
}

/// The point at the end of a series — "you are here".
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawDot {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color_dot: Vec4f,
    #[live]
    pub color_ring: Vec4f,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The `#[live]` fields on each struct become the shader's instances;
    // the script block only carries the pixel program.
    set_type_default() do #(DrawAreaFill::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            // Where the curve sits at this pixel's x, 0 = top of the plot.
            let top = mix(self.top_left, self.top_right, self.pos.x)
            let y = self.pos.y
            let feather = 1.0 / max(self.rect_size.y, 1.0)
            let inside = smoothstep(top - feather, top + feather, y)
            if inside <= 0.0 {
                return #0000
            }
            // Fade to nothing on the way down, so the fill never reads as
            // a solid block and the line stays the subject.
            let depth = (y - top) / max(self.fade * (1.0 - top), 0.001)
            let strength = clamp(1.0 - depth, 0.0, 1.0)
            let alpha = inside * strength * strength * self.color_top.a
            return vec4(self.color_top.rgb * alpha, alpha)
        }
    }

    set_type_default() do #(DrawLineSeg::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let p = self.pos * self.rect_size
            let a = vec2(0.0, self.y0 * self.rect_size.y)
            let b = vec2(self.rect_size.x, self.y1 * self.rect_size.y)
            // Distance to the segment: the capsule SDF, so joins are round
            // and the edge is antialiased at any angle.
            let pa = p - a
            let ba = b - a
            let h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0)
            let d = length(pa - ba * h)
            let half = self.thickness * 0.5
            let line = clamp(1.0 - smoothstep(half - 0.75, half + 0.75, d), 0.0, 1.0)
            let halo = clamp(1.0 - smoothstep(half, half + max(self.glow, 0.001), d), 0.0, 1.0)
            let alpha = clamp(line + halo * 0.3 * (1.0 - line), 0.0, 1.0) * self.color_line.a
            return vec4(self.color_line.rgb * alpha, alpha)
        }
    }

    set_type_default() do #(DrawBar::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let size = self.rect_size
            let sdf = Sdf2d.viewport(self.pos * size)
            // Round the far end only. A bar rounded at both ends reads as a
            // floating pill instead of a measurement standing on an axis,
            // so the baseline end is pushed outside the quad and clipped
            // square. (Sdf2d takes the DIAMETER as its corner argument.)
            let r = min(self.radius, size.x * 0.5)
            if self.downward > 0.5 {
                sdf.box(0.0, 0.0 - r * 2.0, size.x, size.y + r * 2.0, r * 0.5)
            } else {
                sdf.box(0.0, 0.0, size.x, size.y + r * 2.0, r * 0.5)
            }
            sdf.fill(self.color_bar)
            return sdf.result
        }
    }

    set_type_default() do #(DrawMeter::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let size = self.rect_size
            let sdf = Sdf2d.viewport(self.pos * size)
            let r = size.y * 0.25
            sdf.box(0.0, 0.0, size.x, size.y, r)
            sdf.fill(self.color_track)
            let w = max(self.fraction * size.x, size.y)
            sdf.box(0.0, 0.0, w, size.y, r)
            sdf.fill(self.color_fill)
            if self.marker >= 0.0 {
                // A hairline where the target sits, drawn over the fill so
                // it reads whether you are under or over it.
                let x = self.marker * size.x
                sdf.box(x - 1.0, 0.0 - 1.0, 2.0, size.y + 2.0, 0.0)
                sdf.fill(self.color_marker)
            }
            return sdf.result
        }
    }

    set_type_default() do #(DrawDot::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let size = self.rect_size
            let sdf = Sdf2d.viewport(self.pos * size)
            let r = min(size.x, size.y) * 0.5
            sdf.circle(size.x * 0.5, size.y * 0.5, r)
            sdf.fill(self.color_ring)
            sdf.circle(size.x * 0.5, size.y * 0.5, r * 0.42)
            sdf.fill(self.color_dot)
            return sdf.result
        }
    }

    mod.widgets.MeterBase = #(Meter::register_widget(vm))
    mod.widgets.Meter = set_type_default() do mod.widgets.MeterBase{
        width: Fill
        height: 6
        draw_meter +: {
            color_track: #x272a35
            color_fill: #x5e6ad2
            color_marker: #xa2a8b8
        }
    }

    mod.widgets.FinanceChartBase = #(FinanceChart::register_widget(vm))
    mod.widgets.FinanceChart = set_type_default() do mod.widgets.FinanceChartBase{
        width: Fill
        height: Fill
        color_line: #x3987e5
        color_fill: #x3987e5
        color_second: #xd95926
        color_axis: #x6b7784
        color_rule: #x2a323d
        draw_text +: {
            color: #x9aa7b4
            text_style: theme.font_regular{font_size: 7.5}
        }
    }
}

/// What a chart is drawing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// A series over time, filled to the baseline.
    Area,
    /// Two series compared per period, as paired bars.
    Bars,
    /// A series with no chrome at all, for a table cell.
    Spark,
}

impl Default for Form {
    fn default() -> Form {
        Form::Area
    }
}

/// Padding inside the plot, so a line at the maximum is not clipped by the
/// widget's own edge and labels have somewhere to sit.
const PAD_TOP: f64 = 14.0;
const PAD_BOTTOM: f64 = 18.0;
const PAD_RIGHT: f64 = 54.0;

#[derive(Script, ScriptHook, Widget)]
pub struct FinanceChart {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_area: DrawAreaFill,
    #[live]
    draw_line: DrawLineSeg,
    #[live]
    draw_bar: DrawBar,
    #[live]
    draw_dot: DrawDot,
    #[live]
    draw_rule: DrawColor,
    #[live]
    draw_text: DrawText,

    #[live]
    color_line: Vec4f,
    #[live]
    color_fill: Vec4f,
    #[live]
    color_second: Vec4f,
    #[live]
    color_axis: Vec4f,
    #[live]
    color_rule: Vec4f,

    #[rust]
    form: Form,
    #[rust]
    series: Vec<f64>,
    #[rust]
    second: Vec<f64>,
    /// Labels under the bars, and the value labels' formatter output.
    #[rust]
    labels: Vec<String>,
    #[rust]
    value_labels: Vec<(f64, String)>,
    #[rust]
    area: Area,
}

impl FinanceChart {
    pub fn set_area(&mut self, values: &[f64], marks: Vec<(f64, String)>) {
        self.form = Form::Area;
        self.series = values.to_vec();
        self.value_labels = marks;
    }

    pub fn set_spark(&mut self, values: &[f64]) {
        self.form = Form::Spark;
        self.series = values.to_vec();
    }

    pub fn set_bars(&mut self, first: &[f64], second: &[f64], labels: Vec<String>) {
        self.form = Form::Bars;
        self.series = first.to_vec();
        self.second = second.to_vec();
        self.labels = labels;
    }

    /// The value range to draw against.
    ///
    /// Zero is included for bars (a bar chart that does not start at zero
    /// lies about proportion) but NOT for a trend line, where the story is
    /// the change and a forced zero flattens it into a straight line.
    fn bounds(&self) -> (f64, f64) {
        let mut low = f64::MAX;
        let mut high = f64::MIN;
        for value in self.series.iter().chain(self.second.iter()) {
            low = low.min(*value);
            high = high.max(*value);
        }
        if low > high {
            return (0.0, 1.0);
        }
        if self.form == Form::Bars {
            low = low.min(0.0);
            high = high.max(0.0);
        }
        if (high - low).abs() < 1e-9 {
            high = low + 1.0;
        }
        // A little headroom, so the peak is not welded to the top edge.
        let margin = (high - low) * 0.08;
        (low - margin, high + margin)
    }
}

impl Widget for FinanceChart {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.area = Area::Empty;
        if rect.size.x < 2.0 || rect.size.y < 2.0 || self.series.len() < 2 {
            return DrawStep::done();
        }
        let spark = self.form == Form::Spark;
        let plot = Rect {
            pos: dvec2(rect.pos.x, rect.pos.y + if spark { 1.0 } else { PAD_TOP }),
            size: dvec2(
                rect.size.x - if spark { 0.0 } else { PAD_RIGHT },
                rect.size.y - if spark { 2.0 } else { PAD_TOP + PAD_BOTTOM },
            ),
        };
        if plot.size.x < 2.0 || plot.size.y < 2.0 {
            return DrawStep::done();
        }
        let (low, high) = self.bounds();
        let span = high - low;
        let y_of = |value: f64| -> f64 { ((high - value) / span).clamp(0.0, 1.0) };

        match self.form {
            Form::Area | Form::Spark => {
                let count = self.series.len();
                let step = plot.size.x / (count - 1) as f64;

                if !spark {
                    // Two faint rules at the extremes: enough scale to read
                    // against, far less than a grid.
                    self.draw_rule.color = self.color_rule;
                    for fraction in [0.0, 1.0] {
                        self.draw_rule.draw_abs(
                            cx,
                            Rect {
                                pos: dvec2(plot.pos.x, plot.pos.y + plot.size.y * fraction),
                                size: dvec2(plot.size.x, 1.0),
                            },
                        );
                    }
                }

                // The fill, one slice per interval.
                self.draw_area.color_top = Vec4f {
                    w: if spark { 0.35 } else { 0.55 },
                    ..self.color_fill
                };
                self.draw_area.fade = 1.0;
                for index in 0..count - 1 {
                    self.draw_area.top_left = y_of(self.series[index]) as f32;
                    self.draw_area.top_right = y_of(self.series[index + 1]) as f32;
                    self.draw_area.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(plot.pos.x + step * index as f64, plot.pos.y),
                            size: dvec2(step + 0.5, plot.size.y),
                        },
                    );
                }

                // The line on top.
                self.draw_line.color_line = self.color_line;
                self.draw_line.thickness = if spark { 1.5 } else { 2.0 };
                self.draw_line.glow = if spark { 0.0 } else { 5.0 };
                for index in 0..count - 1 {
                    self.draw_line.y0 = y_of(self.series[index]) as f32;
                    self.draw_line.y1 = y_of(self.series[index + 1]) as f32;
                    self.draw_line.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(plot.pos.x + step * index as f64, plot.pos.y),
                            size: dvec2(step, plot.size.y),
                        },
                    );
                }

                if !spark {
                    // "You are here", and the only labelled points: the
                    // last value, and whatever the caller marked.
                    let last = *self.series.last().unwrap();
                    let dot = dvec2(
                        plot.pos.x + plot.size.x,
                        plot.pos.y + y_of(last) * plot.size.y,
                    );
                    self.draw_dot.color_ring = self.color_line;
                    self.draw_dot.color_dot = Vec4f { x: 1.0, y: 1.0, z: 1.0, w: 1.0 };
                    self.draw_dot.draw_abs(
                        cx,
                        Rect { pos: dot - dvec2(4.0, 4.0), size: dvec2(8.0, 8.0) },
                    );
                    self.draw_text.color = self.color_axis;
                    for (value, text) in &self.value_labels {
                        let y = plot.pos.y + y_of(*value) * plot.size.y;
                        self.draw_text.draw_abs(
                            cx,
                            dvec2(plot.pos.x + plot.size.x + 8.0, y - 6.0),
                            text,
                        );
                    }
                }
            }
            Form::Bars => {
                let count = self.series.len().max(1);
                let slot = plot.size.x / count as f64;
                // Two bars per period with a 2px gap between them, and a
                // wider gap between periods so the pairs read as pairs.
                let gap = 2.0;
                let group = (slot - 6.0).max(4.0);
                let bar = ((group - gap) * 0.5).max(2.0);
                let zero = y_of(0.0);

                self.draw_rule.color = self.color_rule;
                self.draw_rule.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(plot.pos.x, plot.pos.y + zero * plot.size.y),
                        size: dvec2(plot.size.x, 1.0),
                    },
                );

                for index in 0..count {
                    let left = plot.pos.x + slot * index as f64 + (slot - group) * 0.5;
                    for (offset, value, color) in [
                        (0.0, self.series.get(index).copied().unwrap_or(0.0), self.color_line),
                        (
                            bar + gap,
                            self.second.get(index).copied().unwrap_or(0.0),
                            self.color_second,
                        ),
                    ] {
                        let top = y_of(value);
                        let baseline = plot.pos.y + zero * plot.size.y;
                        let height = ((zero - top).abs() * plot.size.y).max(2.0);
                        let downward = value < 0.0;
                        // Both directions start ON the baseline, so the two
                        // series of a pair meet exactly at the axis.
                        let y = if downward { baseline } else { baseline - height };
                        self.draw_bar.color_bar = color;
                        self.draw_bar.downward = if downward { 1.0 } else { 0.0 };
                        self.draw_bar.draw_abs(
                            cx,
                            Rect { pos: dvec2(left + offset, y), size: dvec2(bar, height) },
                        );
                    }
                    // Period labels, thinned out so they never collide.
                    if let Some(label) = self.labels.get(index) {
                        let every = ((count as f64 * 42.0) / plot.size.x).ceil().max(1.0) as usize;
                        if index % every == 0 {
                            self.draw_text.color = self.color_axis;
                            self.draw_text.draw_abs(
                                cx,
                                dvec2(left, plot.pos.y + plot.size.y + 4.0),
                                label,
                            );
                        }
                    }
                }
            }
        }
        cx.add_aligned_rect_area(&mut self.area, rect);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

impl FinanceChartRef {
    pub fn set_area(&self, cx: &mut Cx, values: &[f64], marks: Vec<(f64, String)>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_area(values, marks);
            inner.redraw(cx);
        }
    }

    pub fn set_spark(&self, cx: &mut Cx, values: &[f64]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_spark(values);
            inner.redraw(cx);
        }
    }

    pub fn set_bars(&self, cx: &mut Cx, first: &[f64], second: &[f64], labels: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_bars(first, second, labels);
            inner.redraw(cx);
        }
    }
}

/// A proportion bar. One quad, one instance value.
#[derive(Script, ScriptHook, Widget)]
pub struct Meter {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_meter: DrawMeter,
    #[rust]
    area: Area,
}

impl Widget for Meter {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_meter.draw_abs(cx, rect);
        cx.add_aligned_rect_area(&mut self.area, rect);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

impl MeterRef {
    /// `fraction` is 0..1; `color` overrides the fill (for a status colour);
    /// `marker` places the target hairline, or hides it when negative.
    pub fn set(&self, cx: &mut Cx, fraction: f64, color: Option<Vec4f>, marker: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.draw_meter.fraction = fraction.clamp(0.0, 1.0) as f32;
            inner.draw_meter.marker = marker as f32;
            if let Some(color) = color {
                inner.draw_meter.color_fill = color;
            }
            inner.redraw(cx);
        }
    }
}
