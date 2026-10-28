//! The temperature tray's flyout: one well per kind of part (CPU, GPU, storage, memory, board,
//! fans, power), each a graph of its sensors' history under the warning band of their limit, a
//! core strip for per-core CPUs, and a legend table (now, min and max over the shown range).
//! The range chips in the hero pick 2 min / 10 min / 1 h / 24 h; Ctrl+wheel over a graph zooms,
//! a drag pans back in time ("Live" returns), hovering reads every graph at that moment, and the
//! wheel scrolls the wells when there are more than fit. Drawn only while open, from the
//! `Temps` history the WM feeds every second (`shell/temps.rs`).

use super::*;
use super::super::ui::HAlign;
use crate::shell::rgb;
use crate::shell::temps::{Group, Kind, Sensor, Temps, Unit};
use makepad_shell_kit::graph::{self, Span};

/// The range chips, in seconds.
pub(super) const RANGES: [(f64, &str); 4] = [(120.0, "2m"), (600.0, "10m"), (3600.0, "1h"), (86400.0, "24h")];
const MIN_RANGE: f64 = 60.0;
const MAX_RANGE: f64 = 86400.0;

const HERO_H: f64 = 46.0;
const WELL_HEAD: f64 = 20.0;
/// The graph's height: the CPU and GPU wells get the tall one.
const PLOT_MAIN: f64 = 84.0;
const PLOT_SMALL: f64 = 54.0;
const AXIS_H: f64 = 13.0;
const CORES_H: f64 = 24.0;
const LEGEND_HEAD: f64 = 15.0;
const LEGEND_ROW: f64 = 17.0;
const WELL_PAD: f64 = 8.0;
const WELL_GAP: f64 = 10.0;

/// The panel's own view state: the range, how far it is panned back, the hover, the first well.
#[derive(Clone, Debug)]
pub struct TempsView {
    pub range: f64,
    /// seconds back from now at the graphs' right edge
    pub back: f64,
    /// the time the pointer is over in a graph
    pub hover: Option<f64>,
    pub scroll: usize,
    /// a drag pan: where it began (x), the pan then, and seconds per pixel
    pub drag: Option<(f64, f64, f64)>,
    /// the graphs as last drawn: their rect and span
    pub plots: Vec<(Rect, Span)>,
    /// at the last draw: how far the wells can scroll (the last ones then fill the card), and
    /// how many fit from the top
    pub max_scroll: usize,
    pub fit_from_top: usize,
}

impl Default for TempsView {
    fn default() -> Self {
        Self { range: 600.0, back: 0.0, hover: None, scroll: 0, drag: None, plots: Vec::new(), max_scroll: 0, fit_from_top: 0 }
    }
}

/// One well: its sensors (indices into `Temps::sensors`), in panel order.
pub(super) struct Well {
    group: Group,
    unit: Unit,
    /// sensors drawn as traces
    traces: Vec<usize>,
    /// per-core sensors, drawn as the core strip
    cores: Vec<usize>,
}

impl Well {
    fn height(&self) -> f64 {
        let plot = plot_height(self.group);
        let rows = legend_rows(self.traces.len());
        WELL_HEAD + plot + AXIS_H + if self.cores.is_empty() { 0.0 } else { CORES_H + 4.0 } + LEGEND_HEAD + rows as f64 * LEGEND_ROW + WELL_PAD
    }
}

fn plot_height(group: Group) -> f64 {
    if matches!(group, Group::Cpu | Group::Gpu) { PLOT_MAIN } else { PLOT_SMALL }
}

fn legend_cols(n: usize) -> usize {
    if n > 3 { 2 } else { 1 }
}

fn legend_rows(n: usize) -> usize {
    n.div_ceil(legend_cols(n))
}

/// The sensors grouped into wells: by group, then unit (a well has one axis).
pub(super) fn wells(sensors: &[Sensor]) -> Vec<Well> {
    let mut out: Vec<Well> = Vec::new();
    for (i, s) in sensors.iter().enumerate() {
        let w = match out.iter_mut().find(|w| w.group == s.group && w.unit == s.unit) {
            Some(w) => w,
            None => {
                out.push(Well { group: s.group, unit: s.unit, traces: Vec::new(), cores: Vec::new() });
                out.last_mut().unwrap()
            }
        };
        if s.kind == Kind::CpuCore { w.cores.push(i) } else { w.traces.push(i) }
    }
    // a CPU that reports only cores draws them as traces
    for w in &mut out {
        if w.traces.is_empty() {
            w.traces = std::mem::take(&mut w.cores);
        }
    }
    out.sort_by_key(|w| (w.group, w.unit));
    out
}

/// The categorical hues (fixed order, never cycled into red, which is the alarm's), stepped for a
/// light or a dark card.
fn series_color(i: usize, dark: bool) -> Vec4f {
    const LIGHT: [u32; 7] = [0x2a78d6, 0xeb6834, 0x1baf7a, 0xeda100, 0xe87ba4, 0x008300, 0x6250d6];
    const DARK: [u32; 7] = [0x3987e5, 0xd95926, 0x199e70, 0xc98500, 0xd55181, 0x2f9b2f, 0x9085e9];
    let c = if dark { DARK[i % 7] } else { LIGHT[i % 7] };
    rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn mix(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    Vec4f { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, z: a.z + (b.z - a.z) * t, w: 1.0 }
}

/// Round `v` up / down to a multiple of `step`.
fn ceil_to(v: f32, step: f32) -> f32 {
    (v / step).ceil() * step
}
fn floor_to(v: f32, step: f32) -> f32 {
    (v / step).floor() * step
}

/// A 1-2-5 step that cuts `span` into at most `lines` parts.
fn nice_step(span: f32, lines: f32) -> f32 {
    let raw = (span / lines).max(1e-3);
    let mag = 10f32.powf(raw.log10().floor());
    [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag)
}

/// "-8m", "-1h", "-30s", or "now".
fn ago(s: f64) -> String {
    let s = s.round();
    if s < 1.0 {
        "now".into()
    } else if s < 120.0 {
        format!("\u{2212}{}s", s)
    } else if s < 7200.0 {
        let m = s / 60.0;
        if m.fract().abs() < 0.01 { format!("\u{2212}{}m", m) } else { format!("\u{2212}{}:{:02}", (s / 60.0).floor(), s as i64 % 60) }
    } else {
        format!("\u{2212}{}h", (s / 3600.0).round())
    }
}

/// A tick every this many seconds for a range.
fn tick_step(range: f64) -> f64 {
    [15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0, 14400.0]
        .into_iter()
        .find(|s| range / s <= 6.0)
        .unwrap_or(21600.0)
}

impl ShellPanel {
    pub(super) fn temps_height(&self) -> f64 {
        let w = wells(&self.temps.sensors);
        if w.is_empty() {
            return HERO_H + 60.0;
        }
        HERO_H + 8.0 + w.iter().map(|w| w.height() + WELL_GAP).sum::<f64>()
    }

    fn temps_palette(&self) -> (Vec4f, Vec4f, Vec4f, Vec4f, bool) {
        let tok = self.tokens;
        let fg = alpha(tok.popups.text, 1.0);
        let bg = alpha(tok.popups.background, 1.0);
        let dark = bg.x * 0.2126 + bg.y * 0.7152 + bg.z * 0.0722 < 0.5;
        // text in three strengths, mixed toward the card (readable on any theme)
        (fg, mix(bg, fg, 0.72), mix(bg, fg, 0.5), bg, dark)
    }

    /// The time span the graphs show now.
    fn temps_span(&self) -> (f64, f64) {
        let t1 = self.temps.last_at - self.temps_view.back;
        (t1 - self.temps_view.range, t1)
    }

    pub(super) fn draw_temps(&mut self, cx: &mut Cx2d, body: Rect) {
        let tok = self.tokens;
        let (fg, secondary, tertiary, _bg, dark) = self.temps_palette();
        let err = rgb(0xe3, 0x49, 0x48);
        let warn = rgb(0xed, 0xa1, 0x00);
        // the span first: it reads the history's newest time
        let (t0, t1) = self.temps_span();
        let temps = std::mem::take(&mut self.temps);
        self.temps_view.plots.clear();

        // the hero: the hottest reading (or what is too hot), and the range chips on the right
        let (hero, mut rest) = cut_top(body, HERO_H);
        let hot: Vec<String> = temps.sensors.iter().enumerate().filter(|(i, _)| temps.alarmed(*i)).map(|(i, s)| format!("{} {}", s.name(true), s.unit.format(temps.now[i]))).collect();
        let meta = if !hot.is_empty() {
            format!("Too hot: {}", hot.join(", "))
        } else {
            temps.sensors.iter().zip(&temps.now).filter(|(s, v)| s.unit == Unit::Celsius && s.limit.is_some() && !v.is_nan())
                .max_by(|a, b| (a.1 - a.0.limit.unwrap()).total_cmp(&(b.1 - b.0.limit.unwrap())))
                .map_or_else(|| if temps.is_empty() { "No sensors".to_string() } else { "Reading\u{2026}".to_string() }, |(s, v)| format!("{} {} {} \u{b7} {:.0}\u{b0} under its limit", s.group.title(), s.label, s.unit.format(*v), s.limit.unwrap_or(0.0) - v))
        };
        let mut chips: Vec<(Hit, String, bool)> = RANGES.iter().enumerate().map(|(i, (r, l))| (Hit::TempsRange(i), l.to_string(), (self.temps_view.range - r).abs() < 0.5)).collect();
        if self.temps_view.back > 0.5 {
            chips.insert(0, (Hit::TempsLive, "Live".to_string(), false));
        }
        let px = tok.font.caption;
        let mut x = hero.pos.x + hero.size.x;
        let chip_h = 22.0;
        let cy = hero.pos.y + ((hero.size.y - chip_h) * 0.5).floor();
        let mut chip_rects = Vec::new();
        for (hit, label, selected) in chips.iter().rev() {
            let w = self.d.button_width(cx, &tok, false, label, px).max(34.0);
            x -= w;
            chip_rects.push((*hit, rect(x, cy, w, chip_h), label.clone(), *selected));
            x -= 3.0;
        }
        let trailing = hero.pos.x + hero.size.x - x + 8.0;
        let hero_fg = if hot.is_empty() { fg } else { err };
        self.d.panel_hero(cx, hero, &tok, hero_fg, Ico::Thermometer, "Temperatures", &meta, trailing);
        for (hit, r, label, selected) in chip_rects {
            let state = if selected { CtrlState::Selected } else if self.hot == Some(hit) { CtrlState::Hover } else { CtrlState::Normal };
            self.d.button(cx, r, &tok, state, None, &label, px, if selected { fg } else { secondary }, true);
            self.hits.push((hit, r));
        }
        let (sep, r2) = cut_top(rest, 8.0);
        rest = r2;
        self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 3.0, sep.size.x, 1.0), tok.popups.text, 0.12);

        let all = wells(&temps.sensors);
        if all.is_empty() {
            self.d.label(cx, cut_top(rest, 40.0).0, false, tok.font.body, tertiary, HAlign::Left, "This machine reports no temperature sensors.");
            self.temps = temps;
            return;
        }
        let bottom = rest.pos.y + rest.size.y;
        // how many wells fit from the top, and the scroll at which the last ones fill the card
        let heights: Vec<f64> = all.iter().map(|w| w.height() + WELL_GAP).collect();
        let fits = |from: usize| {
            let mut y = rest.pos.y;
            heights[from..].iter().take_while(|h| { y += **h; y - WELL_GAP <= bottom + 0.5 }).count().max(1)
        };
        self.temps_view.fit_from_top = fits(0);
        self.temps_view.max_scroll = (0..all.len()).find(|s| s + fits(*s) >= all.len()).unwrap_or(0);
        self.temps_view.scroll = self.temps_view.scroll.min(self.temps_view.max_scroll);
        let hover = self.temps_view.hover;
        let mut y = rest.pos.y;
        let mut drawn = 0;
        for (wi, well) in all.iter().enumerate().skip(self.temps_view.scroll) {
            let h = well.height();
            if y + h > bottom + 0.5 && drawn > 0 {
                break;
            }
            drawn += 1;
            let r = rect(rest.pos.x, y, rest.size.x, h);
            self.draw_temp_well(cx, &temps, well, wi, r, t0, t1, hover, (fg, secondary, tertiary, dark, warn, err));
            y += h + WELL_GAP;
        }
        let hidden = all.len() - self.temps_view.scroll - drawn;
        if hidden > 0 || self.temps_view.scroll > 0 {
            let note = match (self.temps_view.scroll, hidden) {
                (0, h) => format!("{h} more below \u{2014} scroll"),
                (s, 0) => format!("{s} above \u{2014} scroll up"),
                (s, h) => format!("{s} above, {h} below"),
            };
            let nr = rect(rest.pos.x, (bottom - 12.0).max(y - WELL_GAP), rest.size.x, 12.0);
            self.d.label(cx, nr, false, tok.font.caption * 0.9, tertiary, HAlign::Right, &note);
        }
        self.temps = temps;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_temp_well(&mut self, cx: &mut Cx2d, temps: &Temps, well: &Well, wi: usize, r: Rect, t0: f64, t1: f64, hover: Option<f64>, colors: (Vec4f, Vec4f, Vec4f, bool, Vec4f, Vec4f)) {
        let (fg, secondary, tertiary, dark, warn, err) = colors;
        let tok = self.tokens;
        let sensors = &temps.sensors;
        let alarmed = well.traces.iter().chain(&well.cores).any(|i| temps.alarmed(*i));
        let well_bg = alpha(fg, 0.04);
        let rule = if alarmed { alpha(err, 0.7) } else { alpha(fg, 0.10) };
        self.d.bordered(cx, r, well_bg, rule, rule, 0.0, 1.0);
        let inner = rect(r.pos.x + 8.0, r.pos.y, r.size.x - 16.0, r.size.y);

        // the head: the group, its part when there is one, and the unit
        let mut devices: Vec<&str> = well.traces.iter().chain(&well.cores).map(|i| sensors[*i].device.as_str()).collect();
        devices.dedup();
        let one_device = devices.len() == 1;
        let head = rect(inner.pos.x, r.pos.y + 3.0, inner.size.x, WELL_HEAD - 3.0);
        let title = well.group.title();
        let tw = self.d.measure(cx, true, tok.font.body_small, title);
        self.d.label(cx, head, true, tok.font.body_small, if alarmed { err } else { fg }, HAlign::Left, title);
        let unit = well.unit.name();
        let uw = self.d.measure(cx, false, tok.font.caption, unit);
        if one_device {
            let dev = crate::shell::temps::short_device(devices[0]);
            let dr = rect(head.pos.x + tw + 6.0, head.pos.y, (head.size.x - tw - 6.0 - uw - 8.0).max(0.0), head.size.y);
            self.d.label_elided(cx, dr, false, tok.font.caption, secondary, HAlign::Left, &dev);
        }
        self.d.label(cx, head, false, tok.font.caption, tertiary, HAlign::Right, unit);

        // the plot and its scale
        let plot_h = plot_height(well.group);
        // grid lines the plot has room for
        let lines = if plot_h >= PLOT_MAIN { 4.0 } else { 3.0 };
        let plot = rect(inner.pos.x, r.pos.y + WELL_HEAD, inner.size.x, plot_h);
        let shown: Vec<usize> = well.traces.iter().chain(&well.cores).copied().collect();
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for i in &shown {
            let (a, b) = temps.min_max(*i, t0, t1);
            if !a.is_nan() {
                lo = lo.min(a);
                hi = hi.max(b);
            }
        }
        if lo > hi {
            (lo, hi) = (30.0, 60.0);
        }
        let limit = shown.iter().filter_map(|i| sensors[*i].limit).fold(f32::NAN, |a, b| if a.is_nan() || b < a { b } else { a });
        let crit = shown.iter().filter_map(|i| sensors[*i].crit).fold(f32::NAN, |a, b| if a.is_nan() || b < a { b } else { a });
        let (lo, hi, step) = match well.unit {
            Unit::Celsius => {
                // the limit is part of the picture while it is within reach
                let top = if !limit.is_nan() && limit - hi < 30.0 { hi.max(limit + 3.0) } else { hi + 4.0 };
                let step = nice_step(top - (lo - 4.0), lines).max(5.0);
                (floor_to(lo - 4.0, step).max(0.0), ceil_to(top, step), step)
            }
            Unit::Percent => (0.0, 100.0, 50.0),
            _ => {
                let floor = if well.unit == Unit::Watt { 10.0 } else { 1000.0 };
                let top = (hi * 1.15).max(floor);
                let step = nice_step(top, lines);
                (0.0, ceil_to(top, step), step)
            }
        };
        let span = Span { t0, t1, lo, hi };
        self.d.solid(cx, plot, alpha(fg, 0.025));
        // the warning band: from the limit to the critical point (or the top)
        if !limit.is_nan() && limit < hi {
            let y_lim = span.y(plot, limit);
            let y_top = if !crit.is_nan() && crit > limit && crit < hi { span.y(plot, crit) } else { plot.pos.y };
            self.d.solid(cx, rect(plot.pos.x, plot.pos.y, plot.size.x, (y_top - plot.pos.y).max(0.0)), alpha(err, 0.10));
            self.d.solid(cx, rect(plot.pos.x, y_top, plot.size.x, (y_lim - y_top).max(0.0)), alpha(warn, 0.09));
            let mut x = plot.pos.x;
            while x < plot.pos.x + plot.size.x {
                self.d.solid(cx, rect(x, y_lim.round(), 4.0f64.min(plot.pos.x + plot.size.x - x), 1.0), alpha(err, 0.55));
                x += 7.0;
            }
            let label = format!("limit {}", well.unit.format(limit));
            let lw = self.d.measure(cx, false, tok.font.caption * 0.85, &label);
            self.d.label(cx, rect(plot.pos.x + plot.size.x - lw - 3.0, (y_lim - 12.0).max(plot.pos.y), lw, 11.0), false, tok.font.caption * 0.85, alpha(err, 0.9), HAlign::Left, &label);
        }
        // the grid: values on the left, time ticks
        let mut v = lo + step;
        while v < hi - step * 0.25 {
            let gy = span.y(plot, v).round();
            self.d.solid(cx, rect(plot.pos.x, gy, plot.size.x, 1.0), alpha(fg, 0.06));
            self.d.label(cx, rect(plot.pos.x + 3.0, gy - 11.0, 40.0, 10.0), false, tok.font.caption * 0.85, tertiary, HAlign::Left, &well.unit.format(v).replace(" W", ""));
            v += step;
        }
        let tick = tick_step(t1 - t0);
        let now = temps.last_at;
        let mut tt = ((now - t1) / tick).ceil() * tick; // seconds back of the first tick at or left of the right edge
        let axis = rect(plot.pos.x, plot.pos.y + plot.size.y, plot.size.x, AXIS_H);
        while now - tt >= t0 {
            let tx = span.x(plot, now - tt).round();
            if tx > plot.pos.x + 1.0 && tx < plot.pos.x + plot.size.x - 1.0 {
                self.d.solid(cx, rect(tx, plot.pos.y, 1.0, plot.size.y), alpha(fg, 0.05));
            }
            let label = ago(tt);
            let lw = self.d.measure(cx, false, tok.font.caption * 0.85, &label);
            let lx = (tx - lw * 0.5).clamp(axis.pos.x, axis.pos.x + axis.size.x - lw);
            self.d.label(cx, rect(lx, axis.pos.y, lw, axis.size.y), false, tok.font.caption * 0.85, tertiary, HAlign::Left, &label);
            tt += tick;
        }

        // the traces: thin, the alarmed one heavier, a soft fill only when alone
        let px_s = (t1 - t0) / plot.size.x.max(1.0);
        let gap = temps.gap_for(t0, t1);
        let alone = well.traces.len() == 1;
        let with_device = !one_device;
        for (k, i) in well.traces.iter().enumerate() {
            let color = series_color(k, dark);
            let pts = temps.series(*i, t0, t1, px_s);
            let hot = temps.alarmed(*i);
            graph::trace(&mut self.trace, cx, plot, &mut pts.into_iter(), span, gap, if hot { err } else { color }, if alone { 0.16 } else { 0.0 }, if hot { 2.4 } else { 1.6 });
        }
        // the hover line
        if let Some(t) = hover.filter(|t| *t >= t0 && *t <= t1) {
            let hx = span.x(plot, t).round();
            self.d.solid(cx, rect(hx, plot.pos.y, 1.0, plot.size.y), alpha(fg, 0.35));
            for (k, i) in well.traces.iter().enumerate() {
                let v = temps.value_at(*i, t, t0, t1);
                if !v.is_nan() {
                    let vy = span.y(plot, v);
                    self.d.solid(cx, rect(hx - 2.5, vy - 2.5, 5.0, 5.0), series_color(k, dark));
                }
            }
        }
        self.temps_view.plots.push((plot, span));
        self.hits.push((Hit::TempsPlot(wi), plot));
        let mut y = plot.pos.y + plot.size.y + AXIS_H;

        // the core strip: each core's reading now (or at the hover), shaded by its heat
        if !well.cores.is_empty() {
            let strip = rect(inner.pos.x, y + 4.0, inner.size.x, CORES_H);
            let label_w = 38.0;
            self.d.label(cx, rect(strip.pos.x, strip.pos.y, label_w, strip.size.y), false, tok.font.caption, secondary, HAlign::Left, "Cores");
            let n = well.cores.len() as f64;
            let cw = ((strip.size.x - label_w) / n).floor();
            for (k, i) in well.cores.iter().enumerate() {
                let v = match hover { Some(t) => temps.value_at(*i, t, t0, t1), None => temps.now.get(*i).copied().unwrap_or(f32::NAN) };
                let lim = sensors[*i].limit.unwrap_or(95.0);
                let heat = ((v - 30.0) / (lim - 30.0)).clamp(0.0, 1.0);
                let cell = rect(strip.pos.x + label_w + k as f64 * cw, strip.pos.y, cw - 2.0, strip.size.y);
                let shade = if temps.alarmed(*i) { alpha(err, 0.75) } else { alpha(series_color(1, dark), if v.is_nan() { 0.0 } else { 0.10 + 0.55 * heat }) };
                self.d.solid(cx, cell, shade);
                let text = if v.is_nan() { "\u{2014}".to_string() } else { format!("{:.0}", v) };
                self.d.label(cx, cell, false, tok.font.caption * 0.9, fg, HAlign::Center, &text);
            }
            y += CORES_H + 4.0;
        }

        // the legend table: swatch, name, now (or at the hover), min, max over the range
        let cols = legend_cols(well.traces.len());
        let col_w = ((inner.size.x - (cols - 1) as f64 * 16.0) / cols as f64).floor();
        let vw = 40.0;
        let small = tok.font.caption;
        let now_head = match hover { Some(t) => ago(now - t), None => "now".to_string() };
        for c in 0..cols {
            let cx0 = inner.pos.x + c as f64 * (col_w + 16.0);
            let hr = rect(cx0, y, col_w, LEGEND_HEAD);
            for (j, h) in [now_head.as_str(), "min", "max"].iter().enumerate() {
                let rr = rect(cx0 + col_w - vw * (3 - j) as f64, hr.pos.y, vw, hr.size.y);
                self.d.label(cx, rr, false, small * 0.9, tertiary, HAlign::Right, h);
            }
        }
        y += LEGEND_HEAD;
        let rows = legend_rows(well.traces.len());
        for (k, i) in well.traces.iter().enumerate() {
            let (c, row) = (k / rows, k % rows);
            let cx0 = inner.pos.x + c as f64 * (col_w + 16.0);
            let rr = rect(cx0, y + row as f64 * LEGEND_ROW, col_w, LEGEND_ROW);
            let s = &sensors[*i];
            let hot = temps.alarmed(*i);
            if hot {
                self.d.solid(cx, rect(rr.pos.x - 4.0, rr.pos.y, rr.size.x + 8.0, rr.size.y), alpha(err, 0.16));
            }
            let color = if hot { err } else { series_color(k, dark) };
            self.d.solid(cx, rect(rr.pos.x, rr.pos.y + (rr.size.y * 0.5 - 1.0).round(), 12.0, 2.5), color);
            let name_w = col_w - 18.0 - vw * 3.0 - 4.0;
            let name = if hot { format!("{} \u{2014} too hot", s.name(with_device)) } else { s.name(with_device) };
            self.d.label_elided(cx, rect(rr.pos.x + 18.0, rr.pos.y, name_w, rr.size.y), hot, tok.font.body_small, if hot { err } else { secondary }, HAlign::Left, &name);
            let v = match hover { Some(t) => temps.value_at(*i, t, t0, t1), None => temps.now.get(*i).copied().unwrap_or(f32::NAN) };
            let (mn, mx) = temps.min_max(*i, t0, t1);
            let vals = [(v, true), (mn, false), (mx, false)];
            for (j, (val, strong)) in vals.iter().enumerate() {
                let vr = rect(rr.pos.x + col_w - vw * (3 - j) as f64, rr.pos.y, vw, rr.size.y);
                let col = if hot && *strong { err } else if *strong { fg } else { secondary };
                self.d.label(cx, vr, *strong, tok.font.body_small, col, HAlign::Right, &s.unit.format(*val));
            }
        }
    }

    /// A press in the temperature panel: a range chip, Live, or the start of a pan.
    pub(super) fn temps_press(&mut self, cx: &mut Cx, hit: Hit, p: Vec2d) {
        match hit {
            Hit::TempsRange(i) => {
                self.temps_view.range = RANGES[i].0;
                self.temps_clamp_pan();
            }
            Hit::TempsLive => self.temps_view.back = 0.0,
            Hit::TempsPlot(_) => {
                let s_per_px = self.temps_view.plots.iter().find(|(r, _)| contains(*r, p)).map_or(1.0, |(r, s)| (s.t1 - s.t0) / r.size.x.max(1.0));
                self.temps_view.drag = Some((p.x, self.temps_view.back, s_per_px));
            }
            _ => return,
        }
        self.redraw(cx);
    }

    /// The pointer moved over the open temperature panel: a pan in progress, or the hover readout.
    pub(super) fn temps_pointer(&mut self, cx: &mut Cx, p: Vec2d) {
        if let Some((x0, back0, s_per_px)) = self.temps_view.drag {
            self.temps_view.back = back0 + (p.x - x0) * s_per_px;
            self.temps_clamp_pan();
            self.temps_view.hover = None;
            self.redraw(cx);
            return;
        }
        let hover = self.temps_view.plots.iter().find(|(r, _)| contains(*r, p)).map(|(r, s)| s.t0 + (p.x - r.pos.x) / r.size.x.max(1.0) * (s.t1 - s.t0));
        if hover != self.temps_view.hover {
            self.temps_view.hover = hover;
            self.redraw(cx);
        }
    }

    pub(super) fn temps_release(&mut self) {
        self.temps_view.drag = None;
    }

    /// The wheel: Ctrl over a graph zooms about the pointer's moment, sideways pans, otherwise it
    /// scrolls the wells when they do not all fit.
    pub(super) fn temps_scroll(&mut self, cx: &mut Cx, abs: Vec2d, scroll: Vec2d, modifiers: KeyModifiers) {
        let over_plot = self.temps_view.plots.iter().find(|(r, _)| contains(*r, abs)).copied();
        if let (Some((r, s)), true) = (over_plot, modifiers.control || modifiers.logo) {
            if scroll.y.abs() > 0.0 {
                let f = if scroll.y > 0.0 { 1.25 } else { 0.8 };
                let anchor = s.t0 + (abs.x - r.pos.x) / r.size.x.max(1.0) * (s.t1 - s.t0);
                let range = (self.temps_view.range * f).clamp(MIN_RANGE, MAX_RANGE);
                // keep the moment under the pointer where it is
                let frac = (abs.x - r.pos.x) / r.size.x.max(1.0);
                let t1 = anchor + (1.0 - frac) * range;
                self.temps_view.range = range;
                self.temps_view.back = (self.temps.last_at - t1).max(0.0);
                self.temps_clamp_pan();
                self.redraw(cx);
            }
            return;
        }
        if scroll.x.abs() > scroll.y.abs() || (over_plot.is_some() && modifiers.shift) {
            let d = if scroll.x.abs() > 0.0 { scroll.x } else { scroll.y };
            self.temps_view.back += d.signum() * self.temps_view.range * 0.1;
            self.temps_clamp_pan();
            self.redraw(cx);
            return;
        }
        if scroll.y > 0.0 && self.temps_view.scroll < self.temps_view.max_scroll {
            self.temps_view.scroll += 1;
            self.redraw(cx);
        } else if scroll.y < 0.0 && self.temps_view.scroll > 0 {
            self.temps_view.scroll -= 1;
            self.redraw(cx);
        }
    }

    /// The pan stays within the history there is.
    fn temps_clamp_pan(&mut self) {
        let oldest = self.temps.coarse.first_time().into_iter().chain(self.temps.fine.first_time()).fold(self.temps.last_at, f64::min);
        let max_back = (self.temps.last_at - oldest - self.temps_view.range).max(0.0);
        self.temps_view.back = self.temps_view.back.clamp(0.0, max_back);
    }

    /// The WM opened the panel for a warning: show the newest moment and the alarmed well.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn temps_show_alarm(&mut self) {
        self.temps_view.back = 0.0;
        self.temps_view.hover = None;
        let all = wells(&self.temps.sensors);
        if let Some(w) = all.iter().position(|w| w.traces.iter().chain(&w.cores).any(|i| self.temps.alarmed(*i))) {
            // the wells above the alarmed one scroll away only when they would hide it
            self.temps_view.scroll = if w < self.temps_view.fit_from_top { 0 } else { w.min(self.temps_view.max_scroll) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(key: &str, group: Group, unit: Unit, kind: Kind) -> Sensor {
        Sensor { key: key.into(), device: "d".into(), label: key.into(), group, unit, kind, limit: None, crit: None }
    }

    #[test]
    fn wells_split_by_group_and_unit_and_cores_go_to_the_strip() {
        let sensors = vec![
            s("pkg", Group::Cpu, Unit::Celsius, Kind::CpuPackage),
            s("c0", Group::Cpu, Unit::Celsius, Kind::CpuCore),
            s("c1", Group::Cpu, Unit::Celsius, Kind::CpuCore),
            s("gpu", Group::Gpu, Unit::Celsius, Kind::GpuCore),
            s("fan", Group::Fans, Unit::Percent, Kind::Fan),
            s("rpm", Group::Fans, Unit::Rpm, Kind::Fan),
        ];
        let w = wells(&sensors);
        assert_eq!(w.len(), 4);
        assert_eq!((w[0].traces.clone(), w[0].cores.clone()), (vec![0], vec![1, 2]));
        // Fans: the RPM well before the percent well (unit order)
        assert_eq!((w[2].unit, w[3].unit), (Unit::Rpm, Unit::Percent));
    }

    #[test]
    fn ticks_and_ages_read_well() {
        assert_eq!(tick_step(120.0), 30.0);
        assert_eq!(tick_step(600.0), 120.0);
        assert_eq!(tick_step(3600.0), 600.0);
        assert_eq!(tick_step(86400.0), 14400.0);
        assert_eq!(nice_step(91.0, 3.0), 50.0);
        assert_eq!(nice_step(70.0, 4.0), 20.0);
        assert_eq!(nice_step(30.0, 3.0), 10.0);
        assert_eq!(ago(0.2), "now");
        assert_eq!(ago(30.0), "\u{2212}30s");
        assert_eq!(ago(480.0), "\u{2212}8m");
        assert_eq!(ago(14400.0), "\u{2212}4h");
    }
}
