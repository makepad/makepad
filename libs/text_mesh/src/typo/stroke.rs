//! Single-stroke fonts (PDOOM-PARITY F5): glyphs as pen strokes rather
//! than filled outlines, for plotter lettering, handwriting and write-on.
//!
//! - [`StrokeFont::parse`] reads our `.strokefont` text format (below);
//! - [`stroke_layout`] sets text with **optical kerning**: every glyph
//!   carries left and right ink profiles (the nearest ink per horizontal
//!   band), and each pair is spaced so a soft minimum of the band gaps,
//!   blended with their mean, equals the style's spacing. There are no
//!   kerning tables: round, straight and diagonal neighbours space evenly;
//! - [`write_on`] draws a layout up to a length along its strokes, with the
//!   pen head (position and direction) as an anchor for sparks or a nib;
//! - [`voice_length`] turns sung-word timings into that length so writing
//!   follows a voice word by word and never runs ahead of it.
//!
//! # The format
//!
//! ```text
//! # comment
//! font "Name" em 14 cap 10 x 7 asc 10.5 desc -3 space 4.5
//! glyph A 10 : M 0 0 L 5 10 L 10 0 ; M 1.8 3.6 L 8.2 3.6
//! glyph U+0020 4.5 :
//! ```
//!
//! A glyph line is the char (or `U+hex`), its fallback advance, `:` and
//! commands in font units, y up, baseline 0: `M x y` starts a stroke,
//! `L x y` draws a line, `Q cx cy x y` and `C c1x c1y c2x c2y x y` curves,
//! `A cx cy rx ry a0 a1` an elliptical arc from angle `a0` to `a1`
//! (degrees, counter-clockwise; it joins the pen with a line, or starts a
//! stroke when the pen is up), and `;` lifts the pen.

use super::karaoke::SungWord;
use std::collections::HashMap;

/// Bands of the ink profiles, from the descender to the ascender.
const BANDS: usize = 32;

#[derive(Clone, Debug)]
pub struct StrokeGlyph {
    /// Polylines in font units.
    pub strokes: Vec<Vec<[f32; 2]>>,
    pub advance: f32,
    /// Ink extent per band (None: no ink in the band).
    left: Vec<Option<f32>>,
    right: Vec<Option<f32>>,
    /// Ink x range (0, 0 for an ink-less glyph).
    pub x_min: f32,
    pub x_max: f32,
}

#[derive(Clone, Debug)]
pub struct StrokeFont {
    pub name: String,
    pub em: f32,
    pub cap_height: f32,
    pub x_height: f32,
    pub ascender: f32,
    pub descender: f32,
    /// The word space, font units.
    pub space: f32,
    glyphs: HashMap<char, StrokeGlyph>,
}

/// Our bundled single-stroke fonts (drawn for Makepad).
pub const BUNDLED: &[(&str, &str)] = &[("technical", include_str!("../../resources/stroke/technical.strokefont"))];

fn arc_points(cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32) -> Vec<[f32; 2]> {
    let sweep = (a1 - a0).abs();
    let n = ((sweep / 10.0).ceil() as usize).max(2);
    (0..=n)
        .map(|k| {
            let a = (a0 + (a1 - a0) * k as f32 / n as f32).to_radians();
            [cx + rx * a.cos(), cy + ry * a.sin()]
        })
        .collect()
}

fn parse_char(s: &str) -> Option<char> {
    if let Some(hex) = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+")) {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
    }
    let mut it = s.chars();
    let c = it.next()?;
    it.next().is_none().then_some(c)
}

fn parse_strokes(cmds: &str) -> Result<Vec<Vec<[f32; 2]>>, String> {
    let toks: Vec<&str> = cmds.split_whitespace().collect();
    let mut strokes: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut pen_down = false;
    let mut i = 0;
    let num = |i: usize| -> Result<f32, String> { toks.get(i).ok_or_else(|| "a command is missing numbers".to_string())?.parse::<f32>().map_err(|_| format!("`{}` is not a number", toks[i])) };
    while i < toks.len() {
        let t = toks[i];
        match t {
            ";" => {
                pen_down = false;
                i += 1;
            }
            "M" => {
                strokes.push(vec![[num(i + 1)?, num(i + 2)?]]);
                pen_down = true;
                i += 3;
            }
            "L" => {
                if !pen_down {
                    return Err("`L` with the pen up (start with `M`)".into());
                }
                strokes.last_mut().unwrap().push([num(i + 1)?, num(i + 2)?]);
                i += 3;
            }
            "Q" | "C" => {
                if !pen_down {
                    return Err(format!("`{t}` with the pen up"));
                }
                let s = strokes.last_mut().unwrap();
                let p0 = *s.last().unwrap();
                if t == "Q" {
                    let (c, p) = ([num(i + 1)?, num(i + 2)?], [num(i + 3)?, num(i + 4)?]);
                    for k in 1..=12 {
                        let u = k as f32 / 12.0;
                        let v = 1.0 - u;
                        s.push([v * v * p0[0] + 2.0 * v * u * c[0] + u * u * p[0], v * v * p0[1] + 2.0 * v * u * c[1] + u * u * p[1]]);
                    }
                    i += 5;
                } else {
                    let (c1, c2, p) = ([num(i + 1)?, num(i + 2)?], [num(i + 3)?, num(i + 4)?], [num(i + 5)?, num(i + 6)?]);
                    for k in 1..=16 {
                        let u = k as f32 / 16.0;
                        let v = 1.0 - u;
                        let (a, b, c, d) = (v * v * v, 3.0 * v * v * u, 3.0 * v * u * u, u * u * u);
                        s.push([a * p0[0] + b * c1[0] + c * c2[0] + d * p[0], a * p0[1] + b * c1[1] + c * c2[1] + d * p[1]]);
                    }
                    i += 7;
                }
            }
            "A" => {
                let pts = arc_points(num(i + 1)?, num(i + 2)?, num(i + 3)?, num(i + 4)?, num(i + 5)?, num(i + 6)?);
                if pen_down {
                    let s = strokes.last_mut().unwrap();
                    let joined = s.last().is_some_and(|l| (l[0] - pts[0][0]).abs() < 1e-3 && (l[1] - pts[0][1]).abs() < 1e-3);
                    s.extend_from_slice(if joined { &pts[1..] } else { &pts[..] });
                } else {
                    strokes.push(pts);
                    pen_down = true;
                }
                i += 7;
            }
            _ => return Err(format!("unknown command `{t}` (M, L, Q, C, A or ;)")),
        }
    }
    Ok(strokes.into_iter().filter(|s| !s.is_empty()).collect())
}

impl StrokeFont {
    /// Parses a `.strokefont` text (see the module docs).
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut f = StrokeFont { name: String::new(), em: 14.0, cap_height: 10.0, x_height: 7.0, ascender: 10.5, descender: -3.0, space: 4.5, glyphs: HashMap::new() };
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let err = |e: String| format!("line {}: {e}", n + 1);
            if let Some(rest) = line.strip_prefix("font ") {
                let (name, rest) = match rest.trim_start().strip_prefix('"') {
                    Some(r) => {
                        let end = r.find('"').ok_or_else(|| err("unterminated font name".into()))?;
                        (r[..end].to_string(), &r[end + 1..])
                    }
                    None => (String::new(), rest),
                };
                f.name = name;
                let kv: Vec<&str> = rest.split_whitespace().collect();
                for pair in kv.chunks(2) {
                    let [k, v] = pair else { return Err(err("font metrics come in `key value` pairs".into())) };
                    let v: f32 = v.parse().map_err(|_| err(format!("`{v}` is not a number")))?;
                    match *k {
                        "em" => f.em = v,
                        "cap" => f.cap_height = v,
                        "x" => f.x_height = v,
                        "asc" => f.ascender = v,
                        "desc" => f.descender = v,
                        "space" => f.space = v,
                        _ => return Err(err(format!("unknown font metric `{k}`"))),
                    }
                }
            } else if let Some(rest) = line.strip_prefix("glyph ") {
                // The char first (it may itself be `:`), then `<advance> : <commands>`.
                let rest = rest.trim_start();
                let (ch, rest) = rest.split_once(char::is_whitespace).ok_or_else(|| err("a glyph is `glyph <char> <advance> : <commands>`".into()))?;
                let (adv, cmds) = rest.split_once(':').ok_or_else(|| err("a glyph is `glyph <char> <advance> : <commands>`".into()))?;
                let c = parse_char(ch).ok_or_else(|| err(format!("`{ch}` is not one char or U+hex")))?;
                let advance: f32 = adv.trim().parse().map_err(|_| err("the glyph's advance".into()))?;
                let strokes = parse_strokes(cmds).map_err(err)?;
                let glyph = f.profile(strokes, advance);
                f.glyphs.insert(c, glyph);
            } else {
                return Err(err("expected `font`, `glyph` or a comment".into()));
            }
        }
        if f.em <= 0.0 {
            return Err("the em must be positive".into());
        }
        Ok(f)
    }

    /// A bundled font by name (`"technical"`).
    pub fn bundled(name: &str) -> Option<Self> {
        BUNDLED.iter().find(|(n, _)| *n == name).map(|(_, text)| Self::parse(text).expect("bundled stroke fonts parse"))
    }

    pub fn glyph(&self, c: char) -> Option<&StrokeGlyph> {
        self.glyphs.get(&c)
    }

    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        self.glyphs.keys().copied()
    }

    fn band(&self, y: f32) -> Option<usize> {
        let (lo, hi) = (self.descender - 1.0, self.ascender + 1.0);
        let b = ((y - lo) / (hi - lo) * BANDS as f32).floor();
        (b >= 0.0 && b < BANDS as f32).then_some(b as usize)
    }

    fn profile(&self, strokes: Vec<Vec<[f32; 2]>>, advance: f32) -> StrokeGlyph {
        let mut left = vec![None::<f32>; BANDS];
        let mut right = vec![None::<f32>; BANDS];
        let (mut x_min, mut x_max) = (f32::MAX, f32::MIN);
        for s in &strokes {
            let mut mark = |p: [f32; 2]| {
                x_min = x_min.min(p[0]);
                x_max = x_max.max(p[0]);
                if let Some(b) = self.band(p[1]) {
                    left[b] = Some(left[b].map_or(p[0], |l: f32| l.min(p[0])));
                    right[b] = Some(right[b].map_or(p[0], |r: f32| r.max(p[0])));
                }
            };
            if s.len() == 1 {
                mark(s[0]);
            }
            for w in s.windows(2) {
                let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
                let n = ((d / 0.1).ceil() as usize).max(1);
                for k in 0..=n {
                    let t = k as f32 / n as f32;
                    mark([w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t]);
                }
            }
        }
        if x_min > x_max {
            (x_min, x_max) = (0.0, 0.0);
        }
        StrokeGlyph { strokes, advance, left, right, x_min, x_max }
    }

    /// Where glyph `b`'s origin goes after glyph `a`'s (at 0), in font
    /// units, for an optical gap of `gap`: the soft minimum of the band
    /// gaps (each band also seeing its neighbours, so diagonals count),
    /// blended with their mean.
    pub fn pair_offset(&self, a: &StrokeGlyph, b: &StrokeGlyph, gap: f32) -> f32 {
        let tau = 0.08 * self.em;
        let mut gaps = Vec::new();
        for band in 0..BANDS {
            let Some(r) = a.right[band] else { continue };
            // The nearest ink of b in this band or its neighbours; a
            // neighbour's gap counts a band's height more.
            let h = (self.ascender - self.descender + 2.0) / BANDS as f32;
            let mut best: Option<f32> = None;
            for (db, extra) in [(0isize, 0.0f32), (-1, h), (1, h)] {
                let k = band as isize + db;
                if !(0..BANDS as isize).contains(&k) {
                    continue;
                }
                if let Some(l) = b.left[k as usize] {
                    let g = (l - r).hypot(extra) * if l < r { -1.0 } else { 1.0 };
                    best = Some(best.map_or(g, |x: f32| x.min(g)));
                }
            }
            if let Some(g) = best {
                gaps.push(g);
            }
        }
        if gaps.is_empty() {
            // No facing ink (a mark beside a letter's empty band).
            return a.x_max - b.x_min + gap;
        }
        let mean = gaps.iter().sum::<f32>() / gaps.len() as f32;
        let min = gaps.iter().copied().fold(f32::MAX, f32::min);
        let soft = min - tau * (gaps.iter().map(|g| (-(g - min) / tau).exp()).sum::<f32>() / gaps.len() as f32).ln();
        let measure = 0.7 * soft + 0.3 * mean;
        gap - measure
    }
}

/// How a stroke text is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeStyle {
    /// The em in output units.
    pub size: f32,
    /// The optical gap between letters, in ems.
    pub spacing: f32,
    /// Word space multiplier (1 = the font's).
    pub word_space: f32,
    /// false: plain advances, no optical kerning.
    pub optical: bool,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self { size: 1.0, spacing: 0.12, word_space: 1.0, optical: true }
    }
}

/// A placed stroke.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedStroke {
    pub char_index: usize,
    pub word: Option<usize>,
    pub points: Vec<[f32; 2]>,
    pub length: f32,
    /// Distance along the layout's strokes where this one starts.
    pub start: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrokeLayout {
    pub strokes: Vec<PlacedStroke>,
    /// Sum of the strokes' lengths (pen travel not counted).
    pub length: f32,
    pub width: f32,
    pub size: f32,
    /// Chars missing from the font (drawn as nothing, advanced by a space).
    pub missing: Vec<char>,
}

fn poly_len(p: &[[f32; 2]]) -> f32 {
    p.windows(2).map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt()).sum()
}

/// Sets one line of `text` (y up, baseline 0).
pub fn stroke_layout(font: &StrokeFont, text: &str, style: &StrokeStyle) -> StrokeLayout {
    let k = style.size / font.em;
    let gap = style.spacing * font.em;
    let mut strokes = Vec::new();
    let mut missing = Vec::new();
    let mut pen = 0.0f32;
    let mut prev: Option<(&StrokeGlyph, f32)> = None;
    let mut word = 0usize;
    let mut in_word = false;
    let mut total = 0.0f32;
    let mut width = 0.0f32;
    for (ci, c) in text.chars().enumerate() {
        if c.is_whitespace() {
            let adv = font.space * style.word_space;
            pen = match prev {
                Some((g, o)) if style.optical => o + g.x_max + adv,
                _ => pen + adv,
            };
            prev = None;
            in_word = false;
            continue;
        }
        if !in_word {
            in_word = true;
            word += 1;
        }
        let Some(g) = font.glyph(c) else {
            missing.push(c);
            pen += font.space;
            prev = None;
            continue;
        };
        let origin = match prev {
            Some((p, o)) if style.optical => o + font.pair_offset(p, g, gap),
            Some((p, o)) => o + p.advance,
            None => pen - if style.optical { g.x_min } else { 0.0 },
        };
        for s in &g.strokes {
            let points: Vec<[f32; 2]> = s.iter().map(|p| [(origin + p[0]) * k, p[1] * k]).collect();
            let length = poly_len(&points);
            strokes.push(PlacedStroke { char_index: ci, word: Some(word - 1), points, length, start: total });
            total += length;
        }
        width = width.max((origin + g.x_max) * k);
        prev = Some((g, origin));
        pen = origin + g.advance;
    }
    StrokeLayout { strokes, length: total, width, size: style.size, missing }
}

/// The pen at a moment of writing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenHead {
    pub pos: [f32; 2],
    /// Unit direction of travel.
    pub dir: [f32; 2],
    pub stroke: usize,
    pub char_index: usize,
}

/// The strokes written up to `length` along the layout (each whole or cut),
/// and the pen head; `None` head before anything is drawn or once done.
pub fn write_on(layout: &StrokeLayout, length: f32) -> (Vec<Vec<[f32; 2]>>, Option<PenHead>) {
    let mut out = Vec::new();
    let mut head = None;
    for (i, s) in layout.strokes.iter().enumerate() {
        if length <= s.start {
            break;
        }
        if length >= s.start + s.length {
            out.push(s.points.clone());
            continue;
        }
        let mut left = length - s.start;
        let mut part = vec![s.points[0]];
        for w in s.points.windows(2) {
            let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
            if d >= left {
                let t = if d > 0.0 { left / d } else { 0.0 };
                let p = [w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t];
                part.push(p);
                let dir = if d > 0.0 { [(w[1][0] - w[0][0]) / d, (w[1][1] - w[0][1]) / d] } else { [1.0, 0.0] };
                head = Some(PenHead { pos: p, dir, stroke: i, char_index: s.char_index });
                break;
            }
            left -= d;
            part.push(w[1]);
        }
        out.push(part);
        break;
    }
    (out, head)
}

/// How much of `layout` is written at time `t` when each word is written
/// over its sung span: nothing of a word before it starts, all of it once
/// it ends, in proportion between (no run-ahead). `words[i]` times the
/// layout's word `i`.
pub fn voice_length(layout: &StrokeLayout, words: &[SungWord], t: f32) -> f32 {
    let mut written = 0.0;
    let mut i = 0;
    while i < layout.strokes.len() {
        let w = layout.strokes[i].word;
        let mut len = 0.0;
        while i < layout.strokes.len() && layout.strokes[i].word == w {
            len += layout.strokes[i].length;
            i += 1;
        }
        let frac = match w.and_then(|w| words.get(w)) {
            Some(sw) if t < sw.start => 0.0,
            Some(sw) if t >= sw.end || sw.end <= sw.start => 1.0,
            Some(sw) => (t - sw.start) / (sw.end - sw.start),
            None => 1.0,
        };
        if frac < 1.0 {
            return written + len * frac;
        }
        written += len;
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_technical_font_covers_ascii() {
        let f = StrokeFont::bundled("technical").unwrap();
        for c in ' '..='~' {
            if c != ' ' {
                assert!(f.glyph(c).is_some_and(|g| !g.strokes.is_empty()), "`{c}` is missing");
            }
        }
        let l = stroke_layout(&f, "The quick brown fox, 0123456789!", &StrokeStyle { size: 14.0, ..StrokeStyle::default() });
        assert!(l.missing.is_empty());
        assert!(l.length > 100.0);
    }

    #[test]
    fn optical_kerning_evens_out_gaps() {
        let f = StrokeFont::bundled("technical").unwrap();
        let style = StrokeStyle { size: 14.0, ..StrokeStyle::default() };
        // The ink gap between the two letters of each pair.
        let gap = |s: &str| {
            let l = stroke_layout(&f, s, &style);
            let first = l.strokes.iter().filter(|p| p.char_index == 0).flat_map(|p| p.points.iter()).map(|p| p[0]).fold(f32::MIN, f32::max);
            let second = l.strokes.iter().filter(|p| p.char_index == 1).flat_map(|p| p.points.iter()).map(|p| p[0]).fold(f32::MAX, f32::min);
            second - first
        };
        // Straight-straight pairs sit at the spacing; round and diagonal
        // pairs tuck closer (their ink only meets at a point).
        let hh = gap("HH");
        assert!((hh - 0.12 * 14.0).abs() < 0.3, "HH gap {hh}");
        assert!(gap("OO") < hh && gap("AV") < hh, "OO {} AV {} vs HH {hh}", gap("OO"), gap("AV"));
        assert!(gap("AV") < 0.0, "A and V interlock");
        let plain = stroke_layout(&f, "AV", &StrokeStyle { optical: false, ..style });
        assert!(plain.width > stroke_layout(&f, "AV", &style).width);
    }

    #[test]
    fn write_on_and_voice_timing() {
        let f = StrokeFont::bundled("technical").unwrap();
        let l = stroke_layout(&f, "LT IO", &StrokeStyle { size: 14.0, ..StrokeStyle::default() });
        let (none, head) = write_on(&l, 0.0);
        assert!(none.is_empty() && head.is_none());
        let half = l.length * 0.5;
        let (part, head) = write_on(&l, half);
        let drawn: f32 = part.iter().map(|p| poly_len(p)).sum();
        assert!((drawn - half).abs() < 1e-3, "{drawn} vs {half}");
        let h = head.unwrap();
        assert_eq!(part.last().unwrap().last().copied(), Some(h.pos));
        let (all, head) = write_on(&l, l.length + 1.0);
        assert_eq!(all.len(), l.strokes.len());
        assert!(head.is_none());
        let words = vec![SungWord { chars: 0..2, start: 1.0, end: 2.0, syllables: vec![] }, SungWord { chars: 3..5, start: 3.0, end: 3.5, syllables: vec![] }];
        let first: f32 = l.strokes.iter().filter(|s| s.word == Some(0)).map(|s| s.length).sum();
        assert_eq!(voice_length(&l, &words, 0.5), 0.0);
        assert!((voice_length(&l, &words, 1.5) - first * 0.5).abs() < 1e-3);
        assert!((voice_length(&l, &words, 2.5) - first).abs() < 1e-3, "no run-ahead into the second word");
        assert!((voice_length(&l, &words, 9.0) - l.length).abs() < 1e-3);
    }

    #[test]
    fn parse_errors_name_the_line() {
        let e = StrokeFont::parse("font \"x\" em 10\nglyph A 5 : L 0 0").unwrap_err();
        assert!(e.starts_with("line 2:"), "{e}");
        assert!(StrokeFont::parse("glyph B 5 : M 0 0 X 1").is_err());
    }
}
