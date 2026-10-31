//! The compact faces of the data and 3D controls: a row's chip (about
//! 130 × 18 points, beside the value's name) and the inline control in the
//! code. Each draws from a summary made once per change of the sites
//! ([`Summary`]): a list of any length is at most one mark per pixel
//! column.

use crate::data::*;
use crate::gizmo::{az_el, AXIS_INK};
use crate::pad::PadState;
use crate::plot::{envelope, fmt_range};
use crate::*;

/// Bins a list's chip is summarised into (merged further to the pixels).
const BINS: usize = 96;
/// Points of a shape's outline in a chip.
const SHAPE_PTS: usize = 160;

/// What a chip draws, made once per sites revision.
#[derive(Clone, Debug, Default)]
pub(crate) struct Summary {
    pub lo: f64,
    pub hi: f64,
    pub count: usize,
    /// Per bin, the lowest and highest of the items in it.
    pub bins: Vec<(f32, f32)>,
    /// A shape fitted into 0..1 (x right, y up), aspect kept.
    pub shape: Vec<[f32; 2]>,
    /// A table's records as heat, a bin each (0..1; NaN: no numbers).
    pub heat: Vec<f32>,
}

/// Points fitted into 0..1 about their middle, one scale for both axes,
/// at most [`SHAPE_PTS`] of them (evenly picked).
pub(crate) fn fit_shape(points: &[[f64; 2]]) -> Vec<[f32; 2]> {
    if points.is_empty() {
        return Vec::new();
    }
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in points {
        for a in 0..2 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    let s = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-12);
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let n = points.len();
    let take = n.min(SHAPE_PTS);
    (0..take).map(|i| {
        let p = points[if take <= 1 { 0 } else { i * (n - 1) / (take - 1) }];
        [((p[0] - c[0]) / s + 0.5) as f32, ((p[1] - c[1]) / s + 0.5) as f32]
    }).collect()
}

/// The summary of a site's value.
pub(crate) fn summarize(source: &str, site: &Site) -> Summary {
    match &site.control {
        Control::Data { values, pairs, .. } => {
            let (lo, hi) = values.iter().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v as f64), hi.max(*v as f64)));
            let bins = envelope(values.iter().map(|v| *v as f64), BINS).into_iter().map(|(a, b)| (a as f32, b as f32)).collect();
            let shape = if *pairs && values.len() % 2 == 0 {
                fit_shape(&values.chunks_exact(2).map(|p| [p[0] as f64, p[1] as f64]).collect::<Vec<_>>())
            } else {
                Vec::new()
            };
            Summary { lo, hi, count: values.len(), bins, shape, heat: Vec::new() }
        }
        Control::Table { .. } => {
            let Some(table) = lit_at(source, site.range.start).and_then(|l| table_of(&l)) else { return Summary::default() };
            // Per column its range; per record the mean of where its
            // numbers sit in theirs.
            let cols = table.heads.len();
            let mut ranges = vec![(f64::MAX, f64::MIN); cols];
            for c in &table.cells {
                if let CellValue::Num(v) = c.value {
                    ranges[c.col] = (ranges[c.col].0.min(v), ranges[c.col].1.max(v));
                }
            }
            let mut sums = vec![(0.0f64, 0usize); table.rows];
            for c in &table.cells {
                if let CellValue::Num(v) = c.value {
                    let (lo, hi) = ranges[c.col];
                    let f = if hi > lo { (v - lo) / (hi - lo) } else { 0.5 };
                    sums[c.row].0 += f;
                    sums[c.row].1 += 1;
                }
            }
            let per: Vec<f32> = sums.iter().map(|(s, n)| if *n == 0 { f32::NAN } else { (*s / *n as f64) as f32 }).collect();
            let bins = per.len().min(64).max(1);
            let heat = (0..bins.min(per.len())).map(|b| {
                let (a, z) = (b * per.len() / bins, ((b + 1) * per.len() / bins).max(b * per.len() / bins + 1));
                let vals: Vec<f32> = per[a..z].iter().copied().filter(|v| !v.is_nan()).collect();
                if vals.is_empty() { f32::NAN } else { vals.iter().sum::<f32>() / vals.len() as f32 }
            }).collect();
            // A table of shapes (records of x, y numbers): the first one's.
            let first: Vec<f64> = table.cells.iter().filter(|c| c.row == 0).filter_map(|c| if let CellValue::Num(v) = c.value { Some(v) } else { None }).collect();
            let all_shapes = table.cells.iter().all(|c| matches!(c.value, CellValue::Num(_))) && cols >= 6 && first.len() % 2 == 0 && first.len() >= 6;
            let shape = if all_shapes { fit_shape(&first.chunks_exact(2).map(|p| [p[0], p[1]]).collect::<Vec<_>>()) } else { Vec::new() };
            Summary { lo: 0.0, hi: 0.0, count: table.rows, bins: Vec::new(), shape, heat }
        }
        Control::Path3 { keys } => {
            // From above: x right, z toward you (down).
            let shape = fit_shape(&keys.iter().map(|k| [k[0] as f64, -(k[2] as f64)]).collect::<Vec<_>>());
            Summary { count: keys.len(), shape, ..Summary::default() }
        }
        _ => Summary::default(),
    }
}

/// A short value's control as its text is now: colours, points and
/// directions read again (a few bytes); anything else as the host gave it.
fn live_control(source: &str, site: &Site) -> Control {
    let start = site.range.start;
    let v = || vector_items(source, start).into_iter().map(|(_, v)| v as f32).collect::<Vec<f32>>();
    match &site.control {
        Control::Swatches { .. } => {
            let colors: Vec<[f32; 4]> = color_items(source, start).into_iter().map(|(_, c)| c).collect();
            if colors.is_empty() { site.control.clone() } else { Control::Swatches { colors } }
        }
        Control::Point(_) => match v()[..] {
            [x, y] => Control::Point([x, y]),
            _ => site.control.clone(),
        },
        Control::Point3(_) => match v()[..] {
            [x, y, z] => Control::Point3([x, y, z]),
            _ => site.control.clone(),
        },
        Control::Direction(_) => match v()[..] {
            [x, y, z] => Control::Direction([x, y, z]),
            _ => site.control.clone(),
        },
        other => other.clone(),
    }
}

/// What a list's index means, in words.
fn axis_words(axis: &DataAxis) -> String {
    match axis {
        DataAxis::Index => "x: index".into(),
        DataAxis::Time { per_second, start } => format!("x: seconds ({}/s from {}s)", fmt_axis(*per_second), fmt_axis(*start)),
    }
}

impl CodeValues {
    /// A text's width as drawn (`glyph`: the small face).
    pub(crate) fn text_w(&self, cx: &mut Cx, text: &str, glyph: bool) -> f64 {
        if text.is_empty() {
            return 0.0;
        }
        let dt = if glyph { &self.draw_glyph } else { &self.draw_label };
        dt.layout(cx, 0.0, 0.0, None, false, Align::default(), text).size_in_lpxs.width as f64
    }

    /// A round dot.
    pub(crate) fn dot(&mut self, cx: &mut Cx2d, at: DVec2, r: f64, color: Vec4f) {
        self.draw_ctl.shape = 0.0;
        self.draw_ctl.fill = color;
        self.draw_ctl.edge = vec4(0.0, 0.0, 0.0, 0.0);
        self.draw_ctl.stroke = 0.0;
        self.draw_ctl.radius = r as f32;
        self.draw_ctl.draw_abs(cx, Rect { pos: at - dvec2(r, r), size: dvec2(r * 2.0, r * 2.0) });
    }

    /// A line from `a` to `b`, `w` wide, antialiased.
    pub(crate) fn seg(&mut self, cx: &mut Cx2d, a: DVec2, b: DVec2, w: f64, color: Vec4f) {
        let pad = w * 0.5 + 1.5;
        let lo = dvec2(a.x.min(b.x), a.y.min(b.y)) - dvec2(pad, pad);
        let hi = dvec2(a.x.max(b.x), a.y.max(b.y)) + dvec2(pad, pad);
        self.draw_ctl.shape = 5.0;
        self.draw_ctl.fill = vec4((a.x - lo.x) as f32, (a.y - lo.y) as f32, (b.x - lo.x) as f32, (b.y - lo.y) as f32);
        self.draw_ctl.edge = color;
        self.draw_ctl.stroke = w as f32;
        self.draw_ctl.draw_abs(cx, Rect { pos: lo, size: hi - lo });
    }

    /// A list's bins in `rect`: a bar per item from the lowest when there
    /// are few, else the min/max envelope, one mark per pixel column.
    fn draw_bins(&mut self, cx: &mut Cx2d, rect: Rect, s: &Summary, ink: Vec4f) {
        let n = s.bins.len();
        if n == 0 || !(s.hi >= s.lo) {
            return;
        }
        let pad = ((s.hi - s.lo) * 0.08).max(1e-9);
        let (lo, hi) = (s.lo - pad, s.hi + pad);
        let y_of = |v: f64| rect.pos.y + rect.size.y - ((v - lo) / (hi - lo)).clamp(0.0, 1.0) * rect.size.y;
        let cols = (rect.size.x.floor() as usize).max(1);
        if s.count <= n && s.count * 2 <= cols {
            // Few: a thin bar each, up from the bottom.
            let step = rect.size.x / n as f64;
            let w = (step - 1.0).clamp(1.0, 4.0);
            for (i, (v, _)) in s.bins.iter().enumerate() {
                let y = y_of(*v as f64);
                let h = (rect.pos.y + rect.size.y - y).max(1.0);
                self.solid(cx, Rect { pos: dvec2(rect.pos.x + i as f64 * step + (step - w) * 0.5, rect.pos.y + rect.size.y - h), size: dvec2(w, h) }, ink);
            }
            return;
        }
        let m = cols.min(n);
        let w = rect.size.x / m as f64;
        for c in 0..m {
            let (a, z) = (c * n / m, ((c + 1) * n / m).max(c * n / m + 1).min(n));
            let (mut mn, mut mx) = (f32::MAX, f32::MIN);
            for b in &s.bins[a..z] {
                mn = mn.min(b.0);
                mx = mx.max(b.1);
            }
            if mn > mx {
                continue;
            }
            let (ya, yb) = (y_of(mx as f64), y_of(mn as f64));
            self.solid(cx, Rect { pos: dvec2(rect.pos.x + c as f64 * w, ya - 0.5), size: dvec2((w - 0.2).max(0.8), (yb - ya) + 1.0) }, ink);
        }
    }

    /// A shape (0..1, y up) fitted into `rect`, aspect kept.
    fn draw_shape(&mut self, cx: &mut Cx2d, rect: Rect, shape: &[[f32; 2]], ink: Vec4f) {
        let side = rect.size.x.min(rect.size.y);
        let o = rect.pos + (rect.size - dvec2(side, side)) * 0.5;
        let at = |p: [f32; 2]| dvec2(o.x + p[0] as f64 * side, o.y + (1.0 - p[1] as f64) * side);
        if shape.len() == 1 {
            self.dot(cx, at(shape[0]), 1.5, ink);
        }
        for k in 1..shape.len() {
            self.seg(cx, at(shape[k - 1]), at(shape[k]), 1.1, ink);
        }
    }

    /// A strip of swatches, each remembered for a click (which item).
    fn draw_swatches(&mut self, cx: &mut Cx2d, index: usize, rect: Rect, colors: &[[f32; 4]], hot: bool) -> f64 {
        let n = colors.len().max(1);
        let w = (rect.size.x / n as f64).clamp(3.0, 14.0);
        let mut x = rect.pos.x;
        let open_item = match &self.popup {
            Some(Popup::Wheel { site, item: Some(k), .. }) if *site == index => Some(*k),
            _ => None,
        };
        for (k, c) in colors.iter().enumerate() {
            if x + w > rect.pos.x + rect.size.x + 0.5 {
                break;
            }
            let r = Rect { pos: dvec2(x, rect.pos.y), size: dvec2(w - 1.0, rect.size.y) };
            let on = open_item == Some(k);
            self.draw_ctl.shape = 2.0;
            self.draw_ctl.fill = vec4(c[0], c[1], c[2], c[3]);
            self.draw_ctl.edge = if on { vec4(1.0, 0.48, 0.18, 1.0) } else if hot { vec4(1.0, 1.0, 1.0, 0.35) } else { vec4(1.0, 1.0, 1.0, 0.16) };
            self.draw_ctl.stroke = if on { 1.5 } else { 1.0 };
            self.draw_ctl.radius = 2.0;
            self.draw_ctl.draw_abs(cx, r);
            self.strip_hits.push((r, index, k));
            x += w;
        }
        x - rect.pos.x
    }

    /// A heat strip: a bin per run of records, dark to the accent.
    fn draw_heat(&mut self, cx: &mut Cx2d, rect: Rect, heat: &[f32]) {
        let n = heat.len();
        if n == 0 {
            return;
        }
        let m = (rect.size.x.floor() as usize).max(1).min(n);
        let w = rect.size.x / m as f64;
        for c in 0..m {
            let (a, z) = (c * n / m, ((c + 1) * n / m).max(c * n / m + 1).min(n));
            let vals: Vec<f32> = heat[a..z].iter().copied().filter(|v| !v.is_nan()).collect();
            let color = if vals.is_empty() {
                vec4(1.0, 1.0, 1.0, 0.12)
            } else {
                let v = vals.iter().sum::<f32>() / vals.len() as f32;
                // From the well's blue-grey to the accent.
                let (d, o) = (vec4(0.18, 0.24, 0.32, 1.0), vec4(1.0, 0.48, 0.18, 1.0));
                vec4(d.x + (o.x - d.x) * v, d.y + (o.y - d.y) * v, d.z + (o.z - d.z) * v, 1.0)
            };
            self.solid(cx, Rect { pos: dvec2(rect.pos.x + c as f64 * w, rect.pos.y), size: dvec2((w - 0.3).max(0.8), rect.size.y) }, color);
        }
    }

    /// A tiny 2D pad with the point's dot.
    fn draw_point_pad(&mut self, cx: &mut Cx2d, rect: Rect, v: [f32; 2], hot: bool) {
        let side = rect.size.y.min(rect.size.x);
        let r = Rect { pos: rect.pos + dvec2(0.0, (rect.size.y - side) * 0.5), size: dvec2(side, side) };
        self.boxed(cx, r, vec4(0.03, 0.035, 0.045, 1.0), if hot { vec4(1.0, 0.48, 0.18, 0.9) } else { vec4(1.0, 1.0, 1.0, 0.2) }, 2.5);
        let st = PadState::around([v[0] as f64, v[1] as f64]);
        let u = (((v[0] as f64) - st.lo[0]) / (st.hi[0] - st.lo[0])).clamp(0.0, 1.0);
        let w = (((v[1] as f64) - st.lo[1]) / (st.hi[1] - st.lo[1])).clamp(0.0, 1.0);
        let p = r.pos + dvec2(2.0 + u * (side - 4.0), 2.0 + w * (side - 4.0));
        self.solid(cx, Rect { pos: dvec2(r.pos.x + 1.0, p.y.round()), size: dvec2(side - 2.0, 1.0) }, vec4(0.31, 0.79, 0.69, 0.35));
        self.solid(cx, Rect { pos: dvec2(p.x.round(), r.pos.y + 1.0), size: dvec2(1.0, side - 2.0) }, vec4(0.31, 0.79, 0.69, 0.35));
        self.dot(cx, p, 2.3, vec4(1.0, 0.48, 0.18, 1.0));
    }

    /// A direction's sphere and arrow, as the 3D view first shows it.
    fn draw_sphere_arrow(&mut self, cx: &mut Cx2d, rect: Rect, d: [f32; 3], ink: Vec4f) {
        let side = rect.size.y.min(rect.size.x);
        let r = Rect { pos: rect.pos + dvec2(0.0, (rect.size.y - side) * 0.5), size: dvec2(side, side) };
        let orbit = crate::gizmo::Orbit::framing([0.0; 3], 1.35);
        let cam = crate::gizmo::Cam::new(&orbit, Rect { pos: dvec2(0.0, 0.0), size: dvec2(1.0, 1.0) });
        let u = normalize3([d[0] as f64, d[1] as f64, d[2] as f64]);
        let x = crate::gizmo::dot(u, cam.right);
        let y = crate::gizmo::dot(u, cam.up);
        let z = -crate::gizmo::dot(u, cam.fwd);
        self.draw_ctl.shape = 4.0;
        self.draw_ctl.fill = vec4(x as f32, y as f32, z as f32, 1.0);
        self.draw_ctl.edge = ink;
        self.draw_ctl.stroke = 1.0;
        self.draw_ctl.draw_abs(cx, r);
    }

    /// The chip of a data or 3D value, in `value_rect` (the chip's box is
    /// drawn): `true` when it drew one.
    pub(crate) fn draw_value_chip(&mut self, cx: &mut Cx2d, index: usize, value_rect: Rect, hot: bool) -> bool {
        let Some(site) = self.sites.get(index).cloned() else { return false };
        let summary = self.summaries.get(index).cloned().unwrap_or_default();
        let ink = if hot { vec4(1.0, 0.62, 0.32, 1.0) } else { vec4(0.88, 0.9, 0.93, 1.0) };
        let dim = vec4(0.55, 0.58, 0.63, 1.0);
        let ty = value_rect.pos.y + (value_rect.size.y - 10.0) * 0.5;
        let right = value_rect.pos.x + value_rect.size.x;
        // Short values are read as they are now (a popup's drag shows in
        // its chip at once); long ones from their summary.
        let control = live_control(&self.source, &site);
        match &control {
            Control::Data { pairs, .. } => {
                let plot_w = (value_rect.size.x * 0.42).clamp(24.0, 60.0);
                let plot = Rect { pos: value_rect.pos + dvec2(0.0, 1.0), size: dvec2(plot_w, value_rect.size.y - 2.0) };
                let line = if hot { vec4(1.0, 0.6, 0.3, 1.0) } else { vec4(0.55, 0.78, 0.95, 1.0) };
                if *pairs && !summary.shape.is_empty() {
                    self.draw_shape(cx, plot, &summary.shape, line);
                } else {
                    self.draw_bins(cx, plot, &summary, line);
                }
                let mut x = plot.pos.x + plot.size.x + 6.0;
                if summary.count > 0 {
                    let range = fmt_range(summary.lo, summary.hi);
                    let w = self.text_w(cx, &range, true);
                    if x + w <= right {
                        self.draw_glyph.color = ink;
                        self.draw_glyph.draw_abs(cx, dvec2(x, ty + 1.0), &range);
                        x += w + 6.0;
                        let count = if *pairs { format!("{} pts", summary.count / 2) } else { format!("{}", summary.count) };
                        let cw = self.text_w(cx, &count, true);
                        if x + cw <= right {
                            self.draw_glyph.color = dim;
                            self.draw_glyph.draw_abs(cx, dvec2(x, ty + 1.0), &count);
                        }
                    }
                }
            }
            Control::Swatches { colors } => {
                let strip = Rect { pos: value_rect.pos + dvec2(0.0, 1.0), size: dvec2((value_rect.size.x - 22.0).max(12.0), value_rect.size.y - 2.0) };
                let used = self.draw_swatches(cx, index, strip, colors, hot);
                self.draw_glyph.color = dim;
                self.draw_glyph.draw_abs(cx, dvec2(strip.pos.x + used + 5.0, ty + 1.0), &colors.len().to_string());
            }
            Control::Table { rows, cols, .. } => {
                let label = format!("{rows}×{cols}");
                self.draw_glyph.color = ink;
                self.draw_glyph.draw_abs(cx, dvec2(value_rect.pos.x, ty + 1.0), &label);
                let x = value_rect.pos.x + self.text_w(cx, &label, true) + 7.0;
                let area = Rect { pos: dvec2(x, value_rect.pos.y + 2.0), size: dvec2((right - x).min(72.0).max(8.0), value_rect.size.y - 4.0) };
                if !summary.shape.is_empty() {
                    self.draw_shape(cx, area, &summary.shape, if hot { vec4(1.0, 0.6, 0.3, 1.0) } else { vec4(0.55, 0.78, 0.95, 1.0) });
                } else {
                    let strip = Rect { pos: area.pos + dvec2(0.0, area.size.y * 0.2), size: dvec2(area.size.x, area.size.y * 0.6) };
                    self.draw_heat(cx, strip, &summary.heat);
                }
            }
            Control::Point(v) => {
                let pad = Rect { pos: value_rect.pos, size: dvec2(value_rect.size.y, value_rect.size.y) };
                self.draw_point_pad(cx, pad, *v, hot);
                self.draw_label.color = ink;
                self.draw_label.draw_abs(cx, dvec2(pad.pos.x + pad.size.x + 6.0, ty), &format!("{}, {}", fmt_axis(v[0] as f64), fmt_axis(v[1] as f64)));
            }
            Control::Point3(v) => {
                let mut x = value_rect.pos.x;
                for a in 0..3 {
                    self.draw_glyph.color = Vec4f::from_u32((AXIS_INK[a] << 8) | 0xff);
                    self.draw_glyph.draw_abs(cx, dvec2(x, ty + 1.0), ["x", "y", "z"][a]);
                    let t = fmt_axis(v[a] as f64);
                    self.draw_label.color = ink;
                    self.draw_label.draw_abs(cx, dvec2(x + 7.0, ty), &t);
                    x += 7.0 + self.text_w(cx, &t, false) + 7.0;
                }
            }
            Control::Direction(d) => {
                let ball = Rect { pos: value_rect.pos, size: dvec2(value_rect.size.y, value_rect.size.y) };
                self.draw_sphere_arrow(cx, ball, *d, if hot { vec4(1.0, 0.6, 0.3, 1.0) } else { vec4(1.0, 0.55, 0.25, 1.0) });
                let (az, el) = az_el([d[0] as f64, d[1] as f64, d[2] as f64]);
                self.draw_label.color = ink;
                self.draw_label.draw_abs(cx, dvec2(ball.pos.x + ball.size.x + 6.0, ty), &format!("az {az:.0}°  el {el:.0}°"));
            }
            Control::Path3 { keys } => {
                let area = Rect { pos: value_rect.pos + dvec2(0.0, 1.0), size: dvec2(40.0, value_rect.size.y - 2.0) };
                self.draw_shape(cx, area, &summary.shape, if hot { vec4(1.0, 0.6, 0.3, 1.0) } else { vec4(1.0, 0.55, 0.25, 0.9) });
                self.draw_glyph.color = ink;
                self.draw_glyph.draw_abs(cx, dvec2(area.pos.x + area.size.x + 6.0, ty + 1.0), &format!("{} keys", keys.len()));
            }
            _ => return false,
        }
        true
    }

    /// The inline control (in the code) of a data or 3D value: `true` when
    /// it drew one.
    pub(crate) fn draw_value_inline(&mut self, cx: &mut Cx2d, index: usize, rect: Rect, hot: bool) -> bool {
        let Some(site) = self.sites.get(index).cloned() else { return false };
        let summary = self.summaries.get(index).cloned().unwrap_or_default();
        let edge = if hot { vec4(1.0, 0.48, 0.18, 1.0) } else { vec4(1.0, 1.0, 1.0, 0.22) };
        let ground = vec4(1.0, 1.0, 1.0, if hot { 0.12 } else { 0.06 });
        let inner = Rect { pos: rect.pos + dvec2(3.0, 2.0), size: rect.size - dvec2(6.0, 4.0) };
        let line = if hot { vec4(1.0, 0.6, 0.3, 1.0) } else { vec4(0.55, 0.78, 0.95, 1.0) };
        match &live_control(&self.source, &site) {
            Control::Data { pairs, .. } => {
                self.boxed(cx, rect, ground, edge, 2.5);
                if *pairs && !summary.shape.is_empty() {
                    self.draw_shape(cx, inner, &summary.shape, line);
                } else {
                    self.draw_bins(cx, inner, &summary, line);
                }
            }
            Control::Swatches { colors } => {
                self.draw_swatches(cx, index, rect, colors, hot);
            }
            Control::Table { .. } => {
                self.boxed(cx, rect, ground, edge, 2.5);
                if !summary.shape.is_empty() {
                    self.draw_shape(cx, inner, &summary.shape, line);
                } else {
                    self.draw_heat(cx, Rect { pos: inner.pos + dvec2(0.0, inner.size.y * 0.2), size: dvec2(inner.size.x, inner.size.y * 0.6) }, &summary.heat);
                }
            }
            Control::Point(v) => self.draw_point_pad(cx, rect, *v, hot),
            Control::Point3(_) => {
                self.boxed(cx, rect, ground, edge, 2.5);
                // Three small axes from a corner.
                let o = dvec2(inner.pos.x + inner.size.x * 0.38, inner.pos.y + inner.size.y * 0.72);
                let l = inner.size.y * 0.62;
                self.seg(cx, o, o + dvec2(l, l * 0.25), 1.2, Vec4f::from_u32((AXIS_INK[0] << 8) | 0xff));
                self.seg(cx, o, o - dvec2(0.0, l), 1.2, Vec4f::from_u32((AXIS_INK[1] << 8) | 0xff));
                self.seg(cx, o, o + dvec2(-l * 0.7, l * 0.3), 1.2, Vec4f::from_u32((AXIS_INK[2] << 8) | 0xff));
                self.dot(cx, o + dvec2(l * 0.45, -l * 0.55), 1.8, vec4(1.0, 0.48, 0.18, 1.0));
            }
            Control::Direction(d) => self.draw_sphere_arrow(cx, rect, *d, if hot { vec4(1.0, 0.6, 0.3, 1.0) } else { vec4(1.0, 0.55, 0.25, 1.0) }),
            Control::Path3 { .. } => {
                self.boxed(cx, rect, ground, edge, 2.5);
                self.draw_shape(cx, inner, &summary.shape, vec4(1.0, 0.55, 0.25, 0.95));
            }
            _ => return false,
        }
        true
    }

    /// What a hovered chip says of its value (data and 3D values).
    pub(crate) fn chip_tip(&self, index: usize) -> Option<String> {
        let site = self.sites.get(index)?;
        let s = self.summaries.get(index)?;
        Some(match &site.control {
            Control::Data { pairs, name, axis, .. } => {
                let name = if name.is_empty() { "list".to_string() } else { name.clone() };
                if *pairs {
                    format!("{name} · {} x, y points · a shape · click to edit", s.count / 2)
                } else {
                    format!("{name} · {} items · {} · {}", s.count, fmt_range(s.lo, s.hi), axis_words(axis))
                }
            }
            Control::Swatches { colors } => format!("{} colours · click one to change it", colors.len()),
            Control::Table { rows, cols, heads } => {
                let named = heads.iter().any(|h| h.parse::<usize>().is_err());
                if named { format!("{rows} records · {}", heads.join(", ")) } else { format!("{rows} rows × {cols} · click to edit") }
            }
            Control::Point(v) => format!("x {}, y {} · click to drag on a pad", fmt_axis(v[0] as f64), fmt_axis(v[1] as f64)),
            Control::Point3(v) => format!("x {}  y {}  z {} · click to move in 3D", fmt_axis(v[0] as f64), fmt_axis(v[1] as f64), fmt_axis(v[2] as f64)),
            Control::Direction(d) => {
                let (az, el) = az_el([d[0] as f64, d[1] as f64, d[2] as f64]);
                format!("direction · azimuth {az:.0}°, elevation {el:.0}° · click to turn")
            }
            Control::Path3 { keys } => format!("{} keys in 3D · click to move them", keys.len()),
            _ => return None,
        })
    }

    /// The hovered chip's words, under it.
    pub(crate) fn draw_chip_tip(&mut self, cx: &mut Cx2d, index: usize, chip: Rect) {
        let Some(text) = self.chip_tip(index) else { return };
        let size = cx.current_pass_size();
        let w = self.text_w(cx, &text, false) + 16.0;
        let h = 21.0;
        let mut pos = dvec2(chip.pos.x, chip.pos.y + chip.size.y + 5.0);
        if pos.y + h > size.y - 2.0 {
            pos.y = chip.pos.y - h - 5.0;
        }
        pos.x = pos.x.min(size.x - w - 4.0).max(4.0);
        let r = Rect { pos, size: dvec2(w, h) };
        self.boxed(cx, r, vec4(0.11, 0.12, 0.15, 0.97), vec4(1.0, 1.0, 1.0, 0.18), 4.0);
        self.draw_label.color = vec4(0.9, 0.92, 0.94, 1.0);
        self.draw_label.draw_abs(cx, r.pos + dvec2(8.0, 5.0), &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lists_summarise_into_bins() {
        let values: Vec<f32> = (0..140_000).map(|k| (k % 997) as f32 * 0.01).collect();
        let site = Site { range: 0..1, control: Control::Data { values, pairs: false, name: "R_MYC".into(), axis: DataAxis::Index }, limits: Default::default() };
        let s = summarize("", &site);
        assert_eq!(s.bins.len(), BINS);
        assert_eq!(s.count, 140_000);
        assert!((s.hi - 9.96).abs() < 1e-4 && s.lo == 0.0);
    }

    #[test]
    fn shapes_fit_their_box() {
        let s = fit_shape(&[[0.0, 0.0], [10.0, 0.0], [10.0, 5.0]]);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0], [0.0, 0.25]);
        assert_eq!(s[1], [1.0, 0.25]);
        assert_eq!(s[2], [1.0, 0.75]);
        let many: Vec<[f64; 2]> = (0..5000).map(|k| [k as f64, (k as f64).sin()]).collect();
        assert_eq!(fit_shape(&many).len(), SHAPE_PTS);
    }

    #[test]
    fn tables_summarise_as_heat_and_shapes() {
        let src = "[[0, 0, 10, 0, 10, 10], [1, 1, 2, 2, 3, 3]]";
        let site = Site { range: 0..src.len(), control: Control::Table { rows: 2, cols: 6, heads: vec![] }, limits: Default::default() };
        let s = summarize(src, &site);
        assert_eq!(s.heat.len(), 2);
        assert_eq!(s.shape.len(), 3);
        let src = "[{n: \"a\", v: 1}, {n: \"b\", v: 3}]";
        let site = Site { range: 0..src.len(), control: Control::Table { rows: 2, cols: 2, heads: vec![] }, limits: Default::default() };
        let s = summarize(src, &site);
        assert_eq!(s.heat, vec![0.0, 1.0]);
        assert!(s.shape.is_empty());
    }
}
