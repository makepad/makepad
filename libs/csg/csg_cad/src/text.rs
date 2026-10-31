//! Text as sketch regions: glyph outlines from a TrueType/OpenType font,
//! flattened and nested into outlines with holes, laid out on a baseline.

use crate::region::{regions_from_loops, Loop, Region, P2};
use crate::sketch::Tolerance;

/// How a line of text is laid out.
#[derive(Clone, Debug)]
pub struct TextStyle {
    /// Height of a capital letter (mm).
    pub cap_height: f64,
    /// Extra space between letters (mm).
    pub spacing: f64,
    /// Line spacing as a factor of the cap height (for `\n`).
    pub line_height: f64,
    /// 0 left, 0.5 centre, 1 right (of the widest line), at x = 0.
    pub align: f64,
    /// 0 baseline at y = 0, 0.5 caps centred on y = 0.
    pub valign: f64,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle { cap_height: 5.0, spacing: 0.0, line_height: 1.6, align: 0.0, valign: 0.0 }
    }
}

struct Flatten {
    loops: Vec<Loop>,
    cur: Loop,
    at: P2,
    scale: f64,
    dx: f64,
    dy: f64,
    steps: usize,
}

impl Flatten {
    fn p(&self, x: f32, y: f32) -> P2 {
        [x as f64 * self.scale + self.dx, y as f64 * self.scale + self.dy]
    }
}

impl ttf_parser::OutlineBuilder for Flatten {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.cur.len() >= 3 {
            self.loops.push(std::mem::take(&mut self.cur));
        }
        self.cur.clear();
        self.at = self.p(x, y);
        self.cur.push(self.at);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.at = self.p(x, y);
        self.cur.push(self.at);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (p0, c, p1) = (self.at, self.p(x1, y1), self.p(x, y));
        for i in 1..=self.steps {
            let t = i as f64 / self.steps as f64;
            let u = 1.0 - t;
            self.cur.push([u * u * p0[0] + 2.0 * u * t * c[0] + t * t * p1[0], u * u * p0[1] + 2.0 * u * t * c[1] + t * t * p1[1]]);
        }
        self.at = p1;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (p0, c1, c2, p1) = (self.at, self.p(x1, y1), self.p(x2, y2), self.p(x, y));
        for i in 1..=self.steps {
            let t = i as f64 / self.steps as f64;
            let u = 1.0 - t;
            let f = |a: f64, b: f64, c: f64, d: f64| u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d;
            self.cur.push([f(p0[0], c1[0], c2[0], p1[0]), f(p0[1], c1[1], c2[1], p1[1])]);
        }
        self.at = p1;
    }
    fn close(&mut self) {
        if self.cur.len() >= 3 {
            self.loops.push(std::mem::take(&mut self.cur));
        }
        self.cur.clear();
    }
}

/// The regions of `text` set in `font` (TrueType/OpenType bytes).
pub fn text_regions(font: &[u8], text: &str, style: &TextStyle, tol: &Tolerance) -> Result<Vec<Region>, String> {
    let face = ttf_parser::Face::parse(font, 0).map_err(|e| format!("the font does not read: {e:?}"))?;
    if text.trim().is_empty() {
        return Err("the text is empty".into());
    }
    if !(style.cap_height > 0.0) {
        return Err("the text needs a positive height".into());
    }
    let upem = face.units_per_em() as f64;
    let cap_units = face.capital_height().map(|h| h as f64).filter(|h| *h > 0.0).unwrap_or(upem * 0.7);
    let scale = style.cap_height / cap_units;
    // Curve flattening: finer for larger letters.
    let steps = ((style.cap_height / (tol.chord * 20.0).max(0.01)).sqrt().ceil() as usize).clamp(3, 16);
    let lines: Vec<&str> = text.split('\n').collect();
    let mut widths = Vec::new();
    for line in &lines {
        let mut w = 0.0;
        for ch in line.chars() {
            let gid = face.glyph_index(ch).or_else(|| face.glyph_index('?'));
            if let Some(g) = gid {
                w += face.glyph_hor_advance(g).unwrap_or(0) as f64 * scale + style.spacing;
            }
        }
        widths.push((w - style.spacing).max(0.0));
    }
    let widest = widths.iter().copied().fold(0.0, f64::max);
    let mut loops = Vec::new();
    let mut missing = Vec::new();
    let line_step = style.cap_height * style.line_height;
    let total_h = style.cap_height + line_step * (lines.len() as f64 - 1.0);
    let y_top = -style.valign * total_h + line_step * (lines.len() as f64 - 1.0);
    for (li, line) in lines.iter().enumerate() {
        let mut x = -style.align * widest + (widest - widths[li]) * style.align;
        let y = y_top - line_step * li as f64;
        for ch in line.chars() {
            let gid = match face.glyph_index(ch) {
                Some(g) => g,
                None => {
                    missing.push(ch);
                    match face.glyph_index('?') {
                        Some(g) => g,
                        None => continue,
                    }
                }
            };
            let mut f = Flatten { loops: Vec::new(), cur: Vec::new(), at: [0.0; 2], scale, dx: x, dy: y, steps };
            face.outline_glyph(gid, &mut f);
            if f.cur.len() >= 3 {
                let l = std::mem::take(&mut f.cur);
                f.loops.push(l);
            }
            loops.extend(f.loops);
            x += face.glyph_hor_advance(gid).unwrap_or(0) as f64 * scale + style.spacing;
        }
    }
    if !missing.is_empty() && loops.is_empty() {
        return Err(format!("the font has none of the characters {:?}", missing.iter().collect::<String>()));
    }
    let regions = regions_from_loops(loops);
    if regions.is_empty() {
        return Err("the text has no outlines (only spaces?)".into());
    }
    Ok(regions)
}
