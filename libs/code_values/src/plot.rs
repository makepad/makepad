//! A number list ([`Control::Data`]) as a plot: its values over their index
//! (or over seconds, letters, words: [`DataAxis`]), or its x, y pairs as a
//! shape. A point dragged writes only its number's bytes; shift snaps to
//! round values. A long list (more than [`DENSE`] points) is drawn as its
//! min/max envelope, one mark per pixel column, and a drag edits the item
//! nearest the pointer.

use crate::data::*;
use crate::*;

/// The plot editor's size.
pub(crate) const PLOT_W: f64 = 400.0;
pub(crate) const PLOT_H: f64 = 182.0;
/// Above this many points the plot draws an envelope, not handles.
pub(crate) const DENSE: usize = 400;

/// The plot inside the plot editor's box.
pub(crate) fn plot_area(rect: Rect) -> Rect {
    Rect { pos: rect.pos + dvec2(12.0, 32.0), size: rect.size - dvec2(24.0, 32.0 + 30.0) }
}

/// The plot's toggle between values and x, y points.
pub(crate) fn plot_toggle(rect: Rect) -> Rect {
    Rect { pos: dvec2(rect.pos.x + rect.size.x - 70.0, rect.pos.y + 5.0), size: dvec2(62.0, 16.0) }
}

/// How a plot maps a list's items into its box: values over their index,
/// or x, y pairs as points (one scale for both, y up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlotMap {
    pub pairs: bool,
    pub area: Rect,
    pub x0: f64,
    pub x1: f64,
    pub y0: f64,
    pub y1: f64,
}

impl PlotMap {
    pub fn fit(items: &[(Range<usize>, f64)], pairs: bool, area: Rect) -> Self {
        if pairs {
            let (x0, x1) = plot_range(items.iter().step_by(2).map(|(_, v)| *v));
            let (y0, y1) = plot_range(items.iter().skip(1).step_by(2).map(|(_, v)| *v));
            // One scale for both: the tighter axis widens about its middle.
            let scale = (area.size.x / (x1 - x0)).min(area.size.y / (y1 - y0));
            let (wx, wy) = (area.size.x / scale * 0.5, area.size.y / scale * 0.5);
            let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
            Self { pairs, area, x0: cx - wx, x1: cx + wx, y0: cy - wy, y1: cy + wy }
        } else {
            let (y0, y1) = plot_range(items.iter().map(|(_, v)| *v));
            Self { pairs, area, x0: 0.0, x1: items.len().max(1) as f64, y0, y1 }
        }
    }

    /// The point count.
    pub fn count(&self, items: &[(Range<usize>, f64)]) -> usize {
        if self.pairs { items.len() / 2 } else { items.len() }
    }

    /// Point `k`'s place on screen.
    pub fn point(&self, items: &[(Range<usize>, f64)], k: usize) -> DVec2 {
        if self.pairs { self.to_screen(items[2 * k].1, items[2 * k + 1].1) } else { self.to_screen(k as f64 + 0.5, items[k].1) }
    }

    pub fn to_screen(&self, x: f64, y: f64) -> DVec2 {
        let a = self.area;
        dvec2(a.pos.x + (x - self.x0) / (self.x1 - self.x0) * a.size.x, a.pos.y + a.size.y * (1.0 - ((y - self.y0) / (self.y1 - self.y0)).clamp(0.0, 1.0)))
    }

    pub fn from_screen(&self, p: DVec2) -> (f64, f64) {
        let a = self.area;
        let u = ((p.x - a.pos.x) / a.size.x).clamp(0.0, 1.0);
        let v = 1.0 - ((p.y - a.pos.y) / a.size.y).clamp(0.0, 1.0);
        (self.x0 + u * (self.x1 - self.x0), self.y0 + v * (self.y1 - self.y0))
    }

    /// The point under `abs`: for values the item of its column, for pairs
    /// the nearest within reach.
    pub fn point_at(&self, items: &[(Range<usize>, f64)], abs: DVec2) -> Option<usize> {
        let n = self.count(items);
        if n == 0 {
            return None;
        }
        if !self.pairs {
            let (x, _) = self.from_screen(abs);
            return Some((x.floor().max(0.0) as usize).min(n - 1));
        }
        let mut best = (f64::MAX, 0);
        for k in 0..n {
            let d = (self.point(items, k) - abs).length();
            if d < best.0 {
                best = (d, k);
            }
        }
        (best.0 < 14.0 || n > DENSE && best.0 < 40.0).then_some(best.1)
    }
}

/// Per pixel column of `width`, the lowest and highest of `values` drawn
/// in it (items spread evenly across): one mark per column.
pub(crate) fn envelope(values: impl ExactSizeIterator<Item = f64>, width: usize) -> Vec<(f64, f64)> {
    let n = values.len();
    let cols = width.max(1);
    let mut out = vec![(f64::MAX, f64::MIN); cols.min(n.max(1))];
    let m = out.len();
    for (k, v) in values.enumerate() {
        let c = (k * m / n.max(1)).min(m - 1);
        let e = &mut out[c];
        *e = (e.0.min(v), e.1.max(v));
    }
    out
}

/// A point being dragged in a plot: which, the plot's mapping as the drag
/// began, and its text as written (a pair's two numbers and what is
/// between them).
#[derive(Clone, Debug)]
pub(crate) struct PlotDrag {
    pub point: usize,
    /// From the pointer to the point as pressed (it moves without a jump).
    pub grab: DVec2,
    pub map: PlotMap,
    pub first: String,
    pub sep: String,
    pub second: String,
}

/// A value's text at the scale of its axis.
fn fmt_short(v: f64) -> String {
    let a = v.abs();
    if a >= 10000.0 {
        format!("{:.0}", v)
    } else if a >= 100.0 {
        fmt_axis((v * 10.0).round() / 10.0)
    } else {
        fmt_axis(v)
    }
}

/// A list's range as the chips and plots write it: `lo…hi`.
pub fn fmt_range(lo: f64, hi: f64) -> String {
    format!("{}…{}", fmt_short(lo), fmt_short(hi))
}

impl CodeValues {
    /// A list's numbers, read once per change of the source.
    pub(crate) fn list_items(&mut self, start: usize) -> Rc<Vec<(Range<usize>, f64)>> {
        self.caches.list_items(&self.source, start)
    }

    pub(crate) fn draw_plot_popup(&mut self, cx: &mut Cx2d, site: usize, rect: Rect, hover: Option<usize>, drag: Option<&PlotDrag>, pairs: bool) {
        self.boxed(cx, rect, vec4(0.08, 0.09, 0.11, 1.0), vec4(1.0, 1.0, 1.0, 0.16), 6.0);
        let Some(s) = self.sites.get(site).cloned() else { return };
        let (name, axis) = match &s.control {
            Control::Data { name, axis, .. } => (name.clone(), axis.clone()),
            _ => (String::new(), DataAxis::Index),
        };
        let items = self.list_items(s.range.start);
        let map = drag.map_or_else(|| PlotMap::fit(&items, pairs, plot_area(rect)), |d| d.map);
        let area = map.area;
        let n = map.count(&items);
        let dense = n > DENSE;
        self.boxed(cx, Rect { pos: area.pos - dvec2(2.0, 2.0), size: area.size + dvec2(4.0, 4.0) }, vec4(0.047, 0.055, 0.067, 1.0), vec4(1.0, 1.0, 1.0, 0.07), 3.0);
        // The header: tabs, name, count; the values / x, y toggle.
        self.draw_tabs(cx, rect, false);
        let mut x = crate::grid::tabs(rect)[1].pos.x + 46.0;
        if !name.is_empty() {
            self.draw_label.color = vec4(0.92, 0.93, 0.95, 1.0);
            self.draw_label.draw_abs(cx, dvec2(x, rect.pos.y + 7.0), &name);
            x += self.text_w(cx, &name, false) + 10.0;
        }
        self.draw_glyph.color = vec4(0.55, 0.58, 0.63, 1.0);
        self.draw_glyph.draw_abs(cx, dvec2(x, rect.pos.y + 8.0), &format!("{n} {}", if pairs { "points" } else { "items" }));
        if items.len() % 2 == 0 && !items.is_empty() {
            let t = plot_toggle(rect);
            self.boxed(cx, t, vec4(1.0, 1.0, 1.0, 0.04), vec4(1.0, 1.0, 1.0, 0.14), 3.0);
            self.draw_glyph.color = vec4(0.75, 0.77, 0.81, 1.0);
            self.draw_glyph.draw_abs(cx, t.pos + dvec2(6.0, 3.0), if pairs { "as values" } else { "as shape" });
        }
        // Zero, when in view.
        if map.y0 < 0.0 && map.y1 > 0.0 {
            let y = map.to_screen(0.0, 0.0).y;
            self.solid(cx, Rect { pos: dvec2(area.pos.x, y.round()), size: dvec2(area.size.x, 1.0) }, vec4(1.0, 1.0, 1.0, 0.12));
        }
        if pairs && map.x0 < 0.0 && map.x1 > 0.0 {
            let x = map.to_screen(0.0, 0.0).x;
            self.solid(cx, Rect { pos: dvec2(x.round(), area.pos.y), size: dvec2(1.0, area.size.y) }, vec4(1.0, 1.0, 1.0, 0.12));
        }
        // The x ticks: seconds, or a round step of the index.
        let tick_ink = vec4(0.5, 0.53, 0.58, 1.0);
        if !pairs && n > 0 {
            let (t0, t1, at): (f64, f64, Box<dyn Fn(f64) -> f64>) = match axis {
                DataAxis::Time { per_second, start } if per_second > 0.0 => (start, start + n as f64 / per_second, Box::new(move |t: f64| (t - start) * per_second)),
                _ => (0.0, n as f64, Box::new(|k: f64| k + 0.5)),
            };
            let step = nice_step(t1 - t0).max(if matches!(axis, DataAxis::Time { .. }) { 0.0 } else { 1.0 });
            let mut t = (t0 / step).ceil() * step;
            let mut last_x = f64::MIN;
            while t <= t1 + 1e-9 {
                let px = map.to_screen(at(t), 0.0).x;
                if px >= area.pos.x && px <= area.pos.x + area.size.x {
                    self.solid(cx, Rect { pos: dvec2(px.round(), area.pos.y + area.size.y + 2.0), size: dvec2(1.0, 4.0) }, vec4(1.0, 1.0, 1.0, 0.25));
                    let label = if matches!(axis, DataAxis::Time { .. }) { format!("{}s", fmt_axis(t)) } else { fmt_axis(t) };
                    let w = self.text_w(cx, &label, true);
                    if px - w * 0.5 > last_x + 6.0 {
                        self.draw_glyph.color = tick_ink;
                        self.draw_glyph.draw_abs(cx, dvec2(px - w * 0.5, area.pos.y + area.size.y + 7.0), &label);
                        last_x = px + w * 0.5;
                    }
                }
                t += step;
            }
        }
        // The axes' names: y as its range, x as what the index means.
        let (lo, hi) = items.iter().skip(usize::from(pairs)).step_by(if pairs { 2 } else { 1 }).fold((f64::MAX, f64::MIN), |(lo, hi), (_, v)| (lo.min(*v), hi.max(*v)));
        if lo <= hi {
            self.draw_glyph.color = vec4(0.62, 0.66, 0.72, 1.0);
            self.draw_glyph.draw_abs(cx, area.pos + dvec2(4.0, 2.0), &format!("y {}", fmt_range(lo, hi)));
        }
        let xname = if pairs { "x" } else { axis.label() };
        let w = self.text_w(cx, xname, true);
        self.draw_glyph.color = vec4(0.62, 0.66, 0.72, 1.0);
        self.draw_glyph.draw_abs(cx, dvec2(area.pos.x + area.size.x - w, rect.pos.y + rect.size.y - 13.0), xname);
        // The list: an envelope per pixel column, or its points.
        let line = vec4(0.55, 0.78, 0.95, 0.75);
        if !pairs {
            if dense {
                let cols = area.size.x.floor() as usize;
                let env = envelope(items.iter().map(|(_, v)| *v), cols);
                let m = env.len();
                for (c, (a, b)) in env.iter().enumerate() {
                    if a > b {
                        continue;
                    }
                    let x = area.pos.x + c as f64 * area.size.x / m as f64;
                    let (ya, yb) = (map.to_screen(0.0, *b).y, map.to_screen(0.0, *a).y);
                    self.solid(cx, Rect { pos: dvec2(x.floor(), ya - 0.5), size: dvec2(1.0, (yb - ya).max(0.0) + 1.0) }, line);
                }
            } else {
                for k in 1..n {
                    self.seg(cx, map.point(&items, k - 1), map.point(&items, k), 1.3, vec4(0.55, 0.78, 0.95, 0.55));
                }
            }
        } else if n > 1 {
            // The shape: its outline, a vertex per pixel at most.
            let mut prev = map.point(&items, 0);
            let mut drawn = 0usize;
            for k in 1..n {
                let p = map.point(&items, k);
                if (p - prev).length() < 1.5 && k + 1 < n {
                    continue;
                }
                self.seg(cx, prev, p, 1.3, vec4(0.55, 0.78, 0.95, 0.6));
                prev = p;
                drawn += 1;
                if drawn > 6000 {
                    break;
                }
            }
        }
        let lit = drag.map(|d| d.point).or(hover);
        if !dense {
            for k in 0..n {
                let p = map.point(&items, k);
                let on = lit == Some(k);
                let r = if on { 4.5 } else { 3.0 };
                self.dot(cx, p, r + 1.0, vec4(0.0, 0.0, 0.0, 0.55));
                self.dot(cx, p, r, if on { vec4(1.0, 0.55, 0.25, 1.0) } else { vec4(0.75, 0.88, 1.0, 1.0) });
            }
        }
        // The point under the pointer: lit, with its value beside it.
        if let Some(k) = lit.filter(|k| *k < n) {
            let p = map.point(&items, k);
            if dense {
                self.solid(cx, Rect { pos: dvec2(p.x.round(), area.pos.y), size: dvec2(1.0, area.size.y) }, vec4(1.0, 0.55, 0.25, 0.35));
                self.dot(cx, p, 5.0, vec4(0.0, 0.0, 0.0, 0.55));
                self.dot(cx, p, 4.0, vec4(1.0, 0.55, 0.25, 1.0));
            }
            let text = if pairs {
                format!("{k}   {}, {}", &self.source[items[2 * k].0.clone()], &self.source[items[2 * k + 1].0.clone()])
            } else {
                let at = match axis {
                    DataAxis::Time { per_second, start } if per_second > 0.0 => format!("{}s", fmt_axis(start + k as f64 / per_second)),
                    _ => format!("[{k}]"),
                };
                format!("{at}   {}", &self.source[items[k].0.clone()])
            };
            self.tooltip(cx, p, &text, Rect { pos: dvec2(rect.pos.x, area.pos.y - 4.0), size: dvec2(rect.size.x, rect.pos.y + rect.size.y - area.pos.y + 4.0) });
        }
    }

    /// A small label box beside `at`, kept inside `within`.
    pub(crate) fn tooltip(&mut self, cx: &mut Cx2d, at: DVec2, text: &str, within: Rect) {
        let w = self.text_w(cx, text, false) + 14.0;
        let h = 19.0;
        let mut pos = at + dvec2(10.0, -h - 8.0);
        if pos.x + w > within.pos.x + within.size.x - 4.0 {
            pos.x = at.x - w - 10.0;
        }
        if pos.y < within.pos.y + 4.0 {
            pos.y = at.y + 10.0;
        }
        pos.x = pos.x.max(within.pos.x + 4.0);
        let r = Rect { pos, size: dvec2(w, h) };
        self.boxed(cx, r, vec4(0.13, 0.14, 0.17, 0.97), vec4(1.0, 0.48, 0.18, 0.7), 4.0);
        self.draw_label.color = vec4(0.94, 0.95, 0.97, 1.0);
        self.draw_label.draw_abs(cx, r.pos + dvec2(7.0, 4.0), text);
    }

    pub(crate) fn plot_event(&mut self, cx: &mut Cx, event: &Event, site: usize, rect: Rect, hover: Option<usize>, drag: Option<PlotDrag>, pairs: bool) -> CodeValuesResponse {
        let mut r = CodeValuesResponse::default();
        let start = self.sites.get(site).map_or(0, |s| s.range.start);
        let items = self.list_items(start);
        let map = PlotMap::fit(&items, pairs, plot_area(rect));
        let point_at = |abs: DVec2| -> Option<usize> { if rect.contains(abs) && (Rect { pos: map.area.pos - dvec2(6.0, 6.0), size: map.area.size + dvec2(12.0, 12.0) }).contains(abs) { map.point_at(&items, abs) } else { None } };
        match event {
            Event::MouseDown(e) if !rect.contains(e.abs) => {
                self.popup = None;
                self.gesture = None;
                r.consumed = true;
                r.redraw = true;
            }
            Event::MouseDown(e) if crate::grid::tabs(rect)[1].contains(e.abs) => {
                // The grid tab.
                let mut grid = crate::grid::GridState::new(Some(pairs));
                if let Some(model) = self.grid_model(site, Some(pairs), 0) {
                    grid.col_w = self.grid_col_w(cx, &model);
                    let size = crate::grid::grid_size(&model, grid.col_w);
                    self.popup = Some(Popup::Grid { site, rect: Rect { pos: rect.pos, size }, grid });
                }
                r.consumed = true;
                r.redraw = true;
            }
            Event::MouseDown(e) if items.len() % 2 == 0 && !items.is_empty() && plot_toggle(rect).contains(e.abs) => {
                let h = if pairs { PLOT_H } else { PLOT_H * 1.6 };
                let rect = Rect { pos: rect.pos, size: dvec2(rect.size.x, h) };
                self.popup = Some(Popup::Plot { site, rect, hover: None, drag: None, pairs: !pairs });
                r.consumed = true;
                r.redraw = true;
            }
            Event::MouseDown(e) => {
                if let Some(k) = point_at(e.abs) {
                    let (span, first, sep, second) = if pairs {
                        let (x, y) = (&items[2 * k].0, &items[2 * k + 1].0);
                        (x.start..y.end, self.source[x.clone()].to_string(), self.source[x.end..y.start].to_string(), self.source[y.clone()].to_string())
                    } else {
                        (items[k].0.clone(), self.source[items[k].0.clone()].to_string(), String::new(), String::new())
                    };
                    let id = self.next_gesture();
                    let written = self.source[span.clone()].to_string();
                    r.focus = Some(span.start);
                    self.gesture = Some(Gesture { id, span, last: written.clone(), original: written, scrub: None });
                    let p = map.point(&items, k);
                    let grab = if pairs || (p.y - e.abs.y).abs() < 12.0 { p - e.abs } else { dvec2(0.0, 0.0) };
                    let drag = PlotDrag { point: k, grab, map, first, sep, second };
                    self.popup = Some(Popup::Plot { site, rect, hover: Some(k), drag: Some(drag), pairs });
                }
                r.consumed = true;
                r.redraw = true;
            }
            Event::MouseMove(e) => {
                if let Some(d) = &drag {
                    self.drag_point(&mut r, d, e.abs, e.modifiers.shift, false);
                    r.redraw = true;
                } else {
                    let over = point_at(e.abs);
                    if over != hover {
                        self.popup = Some(Popup::Plot { site, rect, hover: over, drag: None, pairs });
                        r.redraw = true;
                    }
                    if over.is_some() {
                        cx.set_cursor(MouseCursor::Hand);
                    }
                }
                r.consumed = true;
            }
            Event::MouseUp(e) => {
                if let Some(d) = &drag {
                    self.drag_point(&mut r, d, e.abs, e.modifiers.shift, true);
                    self.popup = Some(Popup::Plot { site, rect, hover, drag: None, pairs });
                    r.redraw = true;
                }
                r.consumed = true;
            }
            Event::KeyDown(k) if k.key_code == KeyCode::Escape || k.key_code == KeyCode::ReturnKey => {
                if drag.is_some() {
                    self.revert(&mut r);
                }
                self.popup = None;
                self.gesture = None;
                r.consumed = true;
                r.redraw = true;
            }
            Event::Scroll(_) | Event::KeyDown(_) | Event::TextInput(_) => r.consumed = true,
            _ => {}
        }
        if r.redraw {
            self.pop_list.redraw(cx);
        }
        r
    }

    /// The dragged point follows the pointer: a value its height, a pair
    /// both. Shift: round values (a tenth of the plot's span).
    pub(crate) fn drag_point(&mut self, out: &mut CodeValuesResponse, drag: &PlotDrag, abs: DVec2, snap: bool, done: bool) {
        let (x, y) = drag.map.from_screen(abs + drag.grab);
        let m = drag.map;
        let fmt = |v: f64, written: &str, span: f64| if snap { fmt_snapped(v, nice_step(span)) } else { fmt_item(v, written, span) };
        let text = if m.pairs {
            format!("{}{}{}", fmt(x, &drag.first, m.x1 - m.x0), drag.sep, fmt(y, &drag.second, m.y1 - m.y0))
        } else {
            fmt(y, &drag.first, m.y1 - m.y0)
        };
        if done && self.gesture.as_ref().is_some_and(|g| g.last == g.original && text == g.original) {
            // A press that moved nothing writes nothing.
            self.gesture = None;
            return;
        }
        self.write(out, text, done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_draw_one_mark_per_column() {
        let values: Vec<f64> = (0..140_000).map(|k| (k as f64 * 0.001).sin()).collect();
        let env = envelope(values.iter().copied(), 300);
        assert_eq!(env.len(), 300);
        assert!(env.iter().all(|(a, b)| a <= b));
        // Fewer items than columns: one per item.
        let env = envelope([1.0, 3.0, 2.0].into_iter(), 300);
        assert_eq!(env, vec![(1.0, 1.0), (3.0, 3.0), (2.0, 2.0)]);
    }

    #[test]
    fn a_dense_list_picks_its_column() {
        let items: Vec<(Range<usize>, f64)> = (0..10_000).map(|k| (k..k + 1, k as f64)).collect();
        let area = Rect { pos: dvec2(0.0, 0.0), size: dvec2(100.0, 50.0) };
        let map = PlotMap::fit(&items, false, area);
        assert_eq!(map.point_at(&items, dvec2(50.0, 10.0)), Some(5000));
        assert_eq!(map.point_at(&items, dvec2(100.0, 10.0)), Some(9999));
    }

    #[test]
    fn ranges_read_short() {
        assert_eq!(fmt_range(0.1, 4.2), "0.1…4.2");
        assert_eq!(fmt_range(-12345.6, 310.0), "-12346…310");
    }
}
