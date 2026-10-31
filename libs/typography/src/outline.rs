//! Glyph outlines as paths (PDOOM-PARITY F3): each glyph's contours,
//! flattened to polylines and placed where the layout put the glyph, for
//! tracing, morphing, offsetting, sampling points and debris.
//!
//! Flattening is deterministic IEEE arithmetic (no transcendental calls):
//! a quadratic or cubic curve is cut into the fewest equal-parameter pieces
//! whose chord error stays under `tolerance` (output units).

use super::font::OutlineFont;
use super::layout::TextLayout;
use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};

/// One closed contour of a placed glyph.
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    /// Index into the layout's glyphs.
    pub glyph: usize,
    /// Closed polyline (the first point is not repeated), y up.
    pub points: Vec<[f32; 2]>,
    /// Signed area: the font's outer contours and holes have opposite
    /// signs (TrueType outers are clockwise, negative here; CFF the
    /// reverse); [`Contour::is_hole`] compares it with the glyph's largest.
    pub area: f32,
}

impl Contour {
    pub fn perimeter(&self) -> f32 {
        let n = self.points.len();
        (0..n).map(|i| dist(self.points[i], self.points[(i + 1) % n])).sum()
    }

    /// Points at `count` equal steps of arc length around the contour,
    /// from its first point.
    pub fn resample(&self, count: usize) -> Vec<[f32; 2]> {
        let mut closed = self.points.clone();
        if let Some(&p) = self.points.first() {
            closed.push(p);
        }
        resample(&closed, count)
    }
}

/// Whether contour `c` is a hole: its winding is opposite to the largest
/// contour of the same glyph.
pub fn is_hole(contours: &[Contour], c: usize) -> bool {
    let g = contours[c].glyph;
    let outer = contours.iter().filter(|k| k.glyph == g).max_by(|a, b| a.area.abs().total_cmp(&b.area.abs())).map_or(0.0, |k| k.area);
    contours[c].area * outer < 0.0
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]) * (a[0] - b[0]) + (a[1] - b[1]) * (a[1] - b[1])).sqrt()
}

/// `count` points at equal arc-length steps along an open polyline
/// (the ends included).
pub fn resample(points: &[[f32; 2]], count: usize) -> Vec<[f32; 2]> {
    if points.len() < 2 || count < 2 {
        return points.iter().take(count).copied().collect();
    }
    let mut cum = vec![0.0f32];
    for w in points.windows(2) {
        cum.push(cum.last().unwrap() + dist(w[0], w[1]));
    }
    let total = *cum.last().unwrap();
    let mut out = Vec::with_capacity(count);
    let mut seg = 0usize;
    for k in 0..count {
        let s = total * k as f32 / (count - 1) as f32;
        while seg + 2 < cum.len() && cum[seg + 1] < s {
            seg += 1;
        }
        let (a, b) = (points[seg], points[seg + 1]);
        let len = cum[seg + 1] - cum[seg];
        let t = if len > 0.0 { ((s - cum[seg]) / len).clamp(0.0, 1.0) } else { 0.0 };
        out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
    }
    out
}

/// Flattens an outline into closed polylines, in font units times `scale`,
/// offset by `origin`.
struct Flatten {
    scale: f32,
    origin: [f32; 2],
    /// Tolerance in font units.
    tol: f32,
    contours: Vec<Vec<[f32; 2]>>,
    cur: Vec<[f32; 2]>,
    last: [f32; 2],
}

impl Flatten {
    fn at(&self, x: f32, y: f32) -> [f32; 2] {
        [self.origin[0] + x * self.scale, self.origin[1] + y * self.scale]
    }
    fn pieces(&self, dev: f32) -> usize {
        // Chord error of n equal pieces <= dev / (8 n^2).
        let n = (dev / (8.0 * self.tol)).sqrt().ceil();
        if n.is_finite() { (n as usize).clamp(1, 64) } else { 1 }
    }
    fn flush(&mut self) {
        let mut c = std::mem::take(&mut self.cur);
        if c.len() > 1 && c.first() == c.last() {
            c.pop();
        }
        if c.len() >= 3 {
            self.contours.push(c);
        }
    }
}

impl OutlineBuilder for Flatten {
    fn move_to(&mut self, x: f32, y: f32) {
        self.flush();
        self.last = [x, y];
        let p = self.at(x, y);
        self.cur.push(p);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.last = [x, y];
        let p = self.at(x, y);
        self.cur.push(p);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let p0 = self.last;
        let dev = ((p0[0] - 2.0 * x1 + x).powi(2) + (p0[1] - 2.0 * y1 + y).powi(2)).sqrt();
        let n = self.pieces(dev);
        for k in 1..=n {
            let t = k as f32 / n as f32;
            let u = 1.0 - t;
            let (px, py) = (u * u * p0[0] + 2.0 * u * t * x1 + t * t * x, u * u * p0[1] + 2.0 * u * t * y1 + t * t * y);
            let p = self.at(px, py);
            self.cur.push(p);
        }
        self.last = [x, y];
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let p0 = self.last;
        let d1 = ((p0[0] - 2.0 * x1 + x2).powi(2) + (p0[1] - 2.0 * y1 + y2).powi(2)).sqrt();
        let d2 = ((x1 - 2.0 * x2 + x).powi(2) + (y1 - 2.0 * y2 + y).powi(2)).sqrt();
        let n = self.pieces(6.0 * d1.max(d2));
        for k in 1..=n {
            let t = k as f32 / n as f32;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            let p = self.at(a * p0[0] + b * x1 + c * x2 + d * x, a * p0[1] + b * y1 + c * y2 + d * y);
            self.cur.push(p);
        }
        self.last = [x, y];
    }
    fn close(&mut self) {
        self.flush();
    }
}

fn signed_area(p: &[[f32; 2]]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1]).sum::<f32>() * 0.5
}

/// One glyph's contours at `size`, its origin at `origin`.
pub fn glyph_contours(font: &OutlineFont, glyph: u16, size: f32, origin: [f32; 2], tolerance: f32) -> Vec<Vec<[f32; 2]>> {
    font.with_face(|face| {
        let scale = size / face.units_per_em() as f32;
        let tol = (tolerance.max(1e-6) / scale.max(1e-9)).max(1e-3);
        let mut b = Flatten { scale, origin, tol, contours: Vec::new(), cur: Vec::new(), last: [0.0, 0.0] };
        face.outline_glyph(GlyphId(glyph), &mut b);
        b.flush();
        b.contours
    })
    .unwrap_or_default()
}

/// Every contour of every glyph of `layout`, placed (layout units).
pub fn outlines(font: &OutlineFont, layout: &TextLayout, tolerance: f32) -> Vec<Contour> {
    let mut out = Vec::new();
    for (i, g) in layout.glyphs.iter().enumerate() {
        if !g.has_ink() {
            continue;
        }
        for points in glyph_contours(font, g.glyph, layout.size, [g.x, g.y], tolerance) {
            let area = signed_area(&points);
            out.push(Contour { glyph: i, points, area });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::font::test_font;
    use super::super::layout::{layout, TextStyle};
    use super::*;

    #[test]
    fn outlines_follow_the_layout_and_know_their_holes() {
        let f = test_font("IBMPlexSans-Text.ttf");
        let l = layout(&f, "Oi", &TextStyle { size: 100.0, ..TextStyle::default() });
        let c = outlines(&f, &l, 0.1);
        let o: Vec<usize> = (0..c.len()).filter(|&k| c[k].glyph == 0).collect();
        assert_eq!(o.len(), 2, "O has an outer contour and a counter");
        assert_eq!(o.iter().filter(|&&k| is_hole(&c, k)).count(), 1);
        // Every point lies in its glyph's ink box.
        for k in &c {
            let ink = l.glyphs[k.glyph].ink;
            assert!(k.points.iter().all(|p| p[0] >= ink[0] - 0.5 && p[0] <= ink[2] + 0.5 && p[1] >= ink[1] - 0.5 && p[1] <= ink[3] + 0.5));
        }
        // A finer tolerance gives more points on the same shape.
        let fine = outlines(&f, &l, 0.01);
        assert!(fine[0].points.len() > c[0].points.len());
        assert!((fine[0].perimeter() - c[0].perimeter()).abs() < c[0].perimeter() * 0.01);
        // A variable font away from its default instance still outlines.
        let rf = test_font("RobotoFlex.ttf").with_axes(&[("wdth", 120.0), ("wght", 900.0)]);
        let foom = layout(&rf, "FOOM", &TextStyle { size: 100.0, ..TextStyle::default() });
        assert!(outlines(&rf, &foom, 0.1).len() >= 6, "F, two O with counters, M");
        for (file, axes) in [("Inter.ttf", vec![("wght", 800.0f32)]), ("Inter.ttf", vec![]), ("RobotoFlex.ttf", vec![("wght", 900.0)])] {
            let f = test_font(file).with_axes(&axes.iter().map(|(t, v)| (*t, *v)).collect::<Vec<_>>());
            for text in ["M", "FOOM", "SPARKS"] {
                let l = layout(&f, text, &TextStyle { size: 200.0, ..TextStyle::default() });
                assert!(!outlines(&f, &l, 0.5).is_empty(), "{file} {axes:?} {text}: {:?}", l.glyphs);
            }
        }
        let r = c[0].resample(32);
        assert_eq!(r.len(), 32);
        assert_eq!(r[0], c[0].points[0]);
    }
}
