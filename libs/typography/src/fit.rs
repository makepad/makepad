//! Fitting type to space (PDOOM-PARITY F4): the size (and line breaks) that
//! make a text fill a width, a column, an area or a slot, for one text or a
//! group that must share one size.
//!
//! Every solver works on a measure: the width of a run of text at size 1
//! (advances, kerning and tracking scale with the size, so a width at size
//! `s` is `s` times it). [`font_measure`] builds one from a font.

use super::font::OutlineFont;
use super::layout::{layout, TextStyle};

/// What to fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fit {
    /// One line, as wide as this.
    Width(f32),
    /// Every line (the text's own `\n` lines, or `lines` balanced breaks)
    /// scaled on its own to this width: a justified stack of lines.
    Column { width: f32, lines: usize },
    /// One size for all, wrapped at spaces, the largest that fits.
    Area { width: f32, height: f32 },
    /// As `Area` in a rect (x, y, w, h), with the block aligned in it
    /// (`align`: 0 = left/top, 0.5 = centre, 1 = right/bottom).
    Slot { rect: [f32; 4], align: [f32; 2] },
}

/// A fitted line.
#[derive(Clone, Debug, PartialEq)]
pub struct FitLine {
    pub text: String,
    pub size: f32,
    /// Top-left of the line box (y down, block origin at the fit's origin).
    pub x: f32,
    pub y: f32,
    pub width: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    pub lines: Vec<FitLine>,
    /// The size (the smallest line size for a column).
    pub size: f32,
    pub width: f32,
    pub height: f32,
}

/// Width at size 1 of a run, and the line height at size 1.
pub trait Measure {
    fn width(&self, text: &str) -> f32;
    fn line_height(&self) -> f32;
}

/// A [`Measure`] over a font and style (its `size` is ignored).
pub struct FontMeasure<'a> {
    pub font: &'a OutlineFont,
    pub style: TextStyle,
}

pub fn font_measure<'a>(font: &'a OutlineFont, style: &TextStyle) -> FontMeasure<'a> {
    FontMeasure { font, style: TextStyle { size: 1.0, ..style.clone() } }
}

impl Measure for FontMeasure<'_> {
    fn width(&self, text: &str) -> f32 {
        layout(self.font, text, &self.style).width()
    }
    fn line_height(&self) -> f32 {
        let m = self.font.metrics().scaled(1.0);
        m.line_height() * self.style.line_spacing
    }
}

/// Greedy word wrap of `text` into lines no wider than `width` (units of
/// the measure); a word wider than the width gets a line of its own.
pub fn wrap(m: &dyn Measure, text: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let trial = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if cur.is_empty() || m.width(&trial) <= width {
                cur = trial;
            } else {
                lines.push(std::mem::take(&mut cur));
                cur = word.to_string();
            }
        }
        lines.push(cur);
    }
    lines
}

/// Breaks `text` into `n` lines at spaces so the widest is as narrow as
/// possible (balanced lines for a column).
pub fn balance(m: &dyn Measure, text: &str, n: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let n = n.clamp(1, words.len().max(1));
    if words.len() <= 1 || n == 1 {
        return vec![words.join(" ")];
    }
    // Smallest width W such that greedy wrap at W gives <= n lines.
    let total = m.width(&words.join(" "));
    let (mut lo, mut hi) = (words.iter().map(|w| m.width(w)).fold(0.0, f32::max), total);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if wrap(m, &words.join(" "), mid).len() <= n {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    wrap(m, &words.join(" "), hi)
}

/// Solves `fit` for `text`. `max_size` caps the size (0 = none).
pub fn fit(m: &dyn Measure, text: &str, fit: Fit, max_size: f32) -> Fitted {
    let cap = |s: f32| if max_size > 0.0 { s.min(max_size) } else { s };
    let lh = m.line_height();
    match fit {
        Fit::Width(w) => {
            let one = text.replace('\n', " ");
            let size = cap(w / m.width(&one).max(1e-6));
            let width = m.width(&one) * size;
            Fitted { lines: vec![FitLine { text: one, size, x: 0.0, y: 0.0, width }], size, width, height: lh * size }
        }
        Fit::Column { width, lines } => {
            let rows = if text.contains('\n') || lines == 0 { text.split('\n').map(str::to_string).collect() } else { balance(m, text, lines) };
            let mut y = 0.0;
            let mut out = Vec::new();
            let mut smallest = f32::MAX;
            for r in rows {
                let size = cap(width / m.width(&r).max(1e-6));
                smallest = smallest.min(size);
                out.push(FitLine { width: m.width(&r) * size, text: r, size, x: 0.0, y });
                y += lh * size;
            }
            Fitted { lines: out, size: if smallest == f32::MAX { 0.0 } else { smallest }, width, height: y }
        }
        Fit::Area { width, height } => area(m, text, width, height, cap, [0.0, 0.0, width, height], [0.0, 0.0]),
        Fit::Slot { rect, align } => area(m, text, rect[2], rect[3], cap, rect, align),
    }
}

fn area(m: &dyn Measure, text: &str, width: f32, height: f32, cap: impl Fn(f32) -> f32, rect: [f32; 4], align: [f32; 2]) -> Fitted {
    let lh = m.line_height();
    let fits = |s: f32| {
        let lines = wrap(m, text, width / s);
        let wide = lines.iter().map(|l| m.width(l)).fold(0.0, f32::max) * s;
        wide <= width * 1.0001 && lines.len() as f32 * lh * s <= height * 1.0001
    };
    let (mut lo, mut hi) = (0.0f32, height / lh.max(1e-6));
    for _ in 0..48 {
        let mid = 0.5 * (lo + hi);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let size = cap(lo);
    let lines = if size > 0.0 { wrap(m, text, width / size) } else { Vec::new() };
    let block_h = lines.len() as f32 * lh * size;
    let block_w = lines.iter().map(|l| m.width(l)).fold(0.0, f32::max) * size;
    let y0 = rect[1] + (rect[3] - block_h) * align[1];
    let out = lines
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let w = m.width(&t) * size;
            FitLine { x: rect[0] + (rect[2] - w) * align[0], y: y0 + i as f32 * lh * size, width: w, text: t, size }
        })
        .collect();
    Fitted { lines: out, size, width: block_w, height: block_h }
}

/// Fits several texts to one shared size: each is solved alone and all take
/// the smallest (a group of labels or slams that must match).
pub fn fit_group(m: &dyn Measure, texts: &[&str], fit_each: Fit, max_size: f32) -> Vec<Fitted> {
    let solo: Vec<Fitted> = texts.iter().map(|t| fit(m, t, fit_each, max_size)).collect();
    let shared = solo.iter().map(|f| f.size).fold(f32::MAX, f32::min);
    texts.iter().map(|t| fit(m, t, fit_each, shared)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A monospaced stand-in: every char 0.5 wide, line height 1.2.
    struct Mono;
    impl Measure for Mono {
        fn width(&self, t: &str) -> f32 {
            t.chars().count() as f32 * 0.5
        }
        fn line_height(&self) -> f32 {
            1.2
        }
    }

    #[test]
    fn width_column_area_slot_and_groups() {
        let w = fit(&Mono, "HELLO", Fit::Width(250.0), 0.0);
        assert!((w.size - 100.0).abs() < 1e-3 && (w.width - 250.0).abs() < 1e-3);
        let c = fit(&Mono, "I'M UPPING MY P(DOOM)", Fit::Column { width: 100.0, lines: 3 }, 0.0);
        assert_eq!(c.lines.len(), 3);
        assert!(c.lines.iter().all(|l| (l.width - 100.0).abs() < 1e-3), "each line fills the column");
        assert!(c.lines[1].y > c.lines[0].y);
        let a = fit(&Mono, "the quick brown fox jumps over the lazy dog", Fit::Area { width: 100.0, height: 60.0 }, 0.0);
        assert!(a.width <= 100.01 && a.height <= 60.01 && a.size > 0.0);
        // A little bigger no longer fits.
        let lines = wrap(&Mono, "the quick brown fox jumps over the lazy dog", 100.0 / (a.size * 1.02));
        let too_big = lines.len() as f32 * 1.2 * a.size * 1.02 > 60.0 || lines.iter().any(|l| Mono.width(l) * a.size * 1.02 > 100.0);
        assert!(too_big);
        let s = fit(&Mono, "centred", Fit::Slot { rect: [10.0, 20.0, 200.0, 100.0], align: [0.5, 0.5] }, 30.0);
        assert_eq!(s.size, 30.0, "max size caps");
        let l = &s.lines[0];
        assert!((l.x + l.width * 0.5 - 110.0).abs() < 1e-3 && (l.y + s.height * 0.5 - 70.0).abs() < 1e-3);
        let g = fit_group(&Mono, &["A", "LONGER"], Fit::Width(60.0), 0.0);
        assert_eq!(g[0].size, g[1].size);
        assert!((g[1].width - 60.0).abs() < 1e-3);
    }

    #[test]
    fn balanced_breaks_even_out_lines() {
        let b = balance(&Mono, "aaaa bb cc dddd", 2);
        assert_eq!(b, vec!["aaaa bb".to_string(), "cc dddd".to_string()]);
    }
}
