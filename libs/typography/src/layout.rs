//! Shaped, kerned text as numbers (PDOOM-PARITY F2): where every glyph
//! sits, how far it advances, its ink box, and the font's cap height and
//! x-height, so documents and kernels can place, split and measure type.
//!
//! Units: the em is `size`; y is up with the first baseline at 0, and later
//! lines sit lower (negative y). Shaping is rustybuzz (GSUB/GPOS kerning,
//! ligatures, features such as `lnum`); `tracking` adds `tracking * size`
//! after every glyph but a line's last.

use super::font::{FontMetrics, OutlineFont};

/// How text is set.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// The em size in output units.
    pub size: f32,
    /// Extra space after each glyph, in ems.
    pub tracking: f32,
    /// Baseline distance as a multiple of the font's line height.
    pub line_spacing: f32,
    /// OpenType features, e.g. `"lnum"`, `"-liga"`, `"ss01"`.
    pub features: Vec<String>,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self { size: 1.0, tracking: 0.0, line_spacing: 1.0, features: Vec::new() }
    }
}

/// One shaped glyph.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBox {
    pub glyph: u16,
    /// Index of the first `char` of the text this glyph shows.
    pub char_index: usize,
    /// Word ordinal (whitespace separates words); `None` for whitespace.
    pub word: Option<usize>,
    pub line: usize,
    /// Pen position of the glyph's origin (x, baseline y).
    pub x: f32,
    pub y: f32,
    pub advance: f32,
    /// Ink box relative to the text origin: x0, y0 (bottom), x1, y1 (top);
    /// empty (all x) for glyphs without ink.
    pub ink: [f32; 4],
}

impl GlyphBox {
    pub fn has_ink(&self) -> bool {
        self.ink[2] > self.ink[0]
    }

    /// Centre of the ink box, or of the advance for an ink-less glyph.
    pub fn centre(&self) -> [f32; 2] {
        if self.has_ink() {
            [(self.ink[0] + self.ink[2]) * 0.5, (self.ink[1] + self.ink[3]) * 0.5]
        } else {
            [self.x + self.advance * 0.5, self.y]
        }
    }
}

/// A laid-out text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLayout {
    pub glyphs: Vec<GlyphBox>,
    /// Font metrics at `size`.
    pub metrics: FontMetrics,
    pub size: f32,
    /// Width of each line (advances plus tracking).
    pub line_widths: Vec<f32>,
    /// Baseline distance.
    pub line_height: f32,
    /// Chars in the text.
    pub chars: usize,
    pub words: usize,
}

impl TextLayout {
    /// The widest line.
    pub fn width(&self) -> f32 {
        self.line_widths.iter().copied().fold(0.0, f32::max)
    }

    /// From the first line's ascender to the last line's descender.
    pub fn height(&self) -> f32 {
        let lines = self.line_widths.len().max(1) as f32;
        self.metrics.ascender - self.metrics.descender + (lines - 1.0) * self.line_height
    }

    /// The union of every glyph's ink.
    pub fn ink(&self) -> Option<[f32; 4]> {
        self.glyphs.iter().filter(|g| g.has_ink()).map(|g| g.ink).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
    }

    /// The pen x where `char_index` starts (for split runs: the rest of a
    /// line set as its own text lines up with this). The text's end for
    /// an index past it.
    pub fn glyph_x(&self, char_index: usize) -> f32 {
        match self.glyphs.iter().find(|g| g.char_index >= char_index) {
            Some(g) => g.x,
            None => self.glyphs.last().map_or(0.0, |g| g.x + g.advance),
        }
    }
}

fn features(style: &TextStyle) -> Vec<rustybuzz::Feature> {
    style.features.iter().filter_map(|f| f.parse::<rustybuzz::Feature>().ok()).collect()
}

/// Lays `text` out in `font` (lines split at `\n`).
pub fn layout(font: &OutlineFont, text: &str, style: &TextStyle) -> TextLayout {
    let metrics_u = font.metrics();
    let k = style.size / metrics_u.units_per_em;
    let metrics = metrics_u.scaled(style.size);
    let line_height = metrics.line_height() * style.line_spacing;
    let feats = features(style);
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    // Word ordinal per char.
    let mut word_of = Vec::with_capacity(chars.len());
    let mut words = 0usize;
    let mut in_word = false;
    for (_, c) in &chars {
        if c.is_whitespace() {
            in_word = false;
            word_of.push(None);
        } else {
            if !in_word {
                in_word = true;
                words += 1;
            }
            word_of.push(Some(words - 1));
        }
    }
    let byte_to_char = |b: usize| chars.partition_point(|(i, _)| *i < b);
    let mut glyphs = Vec::new();
    let mut line_widths = Vec::new();
    let mut line_start = 0usize;
    let tracking = style.tracking * style.size;
    font.with_face(|face| {
        for (line, line_text) in text.split('\n').enumerate() {
            let y = -(line as f32) * line_height;
            let mut buffer = rustybuzz::UnicodeBuffer::new();
            buffer.push_str(line_text);
            let shaped = rustybuzz::shape(face, &feats, buffer);
            let n = shaped.len();
            let mut pen = 0.0f32;
            for (i, (info, pos)) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()).enumerate() {
                let char_index = byte_to_char(line_start + info.cluster as usize);
                let gx = pen + pos.x_offset as f32 * k;
                let gy = y + pos.y_offset as f32 * k;
                let id = rustybuzz::ttf_parser::GlyphId(info.glyph_id as u16);
                let ink = face.glyph_bounding_box(id).map_or([gx, gy, gx, gy], |r| [gx + r.x_min as f32 * k, gy + r.y_min as f32 * k, gx + r.x_max as f32 * k, gy + r.y_max as f32 * k]);
                let mut advance = pos.x_advance as f32 * k;
                if i + 1 < n {
                    advance += tracking;
                }
                glyphs.push(GlyphBox { glyph: info.glyph_id as u16, char_index, word: word_of.get(char_index).copied().flatten(), line, x: gx, y: gy, advance, ink });
                pen += advance;
            }
            line_widths.push(pen);
            line_start += line_text.len() + 1;
        }
    });
    TextLayout { glyphs, metrics, size: style.size, line_widths, line_height, chars: chars.len(), words }
}

#[cfg(test)]
mod tests {
    use super::super::font::test_font;
    use super::*;

    #[test]
    fn kerned_layout_metrics_and_split_runs() {
        let f = test_font("IBMPlexSans-Text.ttf");
        let style = TextStyle { size: 100.0, ..TextStyle::default() };
        let av = layout(&f, "AV", &style);
        let a = layout(&f, "A", &style);
        let v = layout(&f, "V", &style);
        assert!(av.width() < a.width() + v.width(), "AV is kerned tighter than A + V: {} vs {}", av.width(), a.width() + v.width());
        let t = layout(&f, "one two\nthree", &style);
        assert_eq!(t.words, 3);
        assert_eq!(t.line_widths.len(), 2);
        let three = t.glyphs.iter().find(|g| g.line == 1).unwrap();
        assert_eq!((three.word, three.char_index), (Some(2), 8));
        assert!((three.y + t.line_height).abs() < 1e-3);
        assert!(t.glyphs.iter().any(|g| g.word.is_none()), "the space has no word");
        assert!(t.metrics.cap_height > t.metrics.x_height && t.metrics.x_height > 30.0);
        // Split runs: "two" set alone starts where glyph_x says.
        let whole = layout(&f, "one two", &style);
        assert!(whole.glyph_x(4) > layout(&f, "one ", &style).width() - 1e-3);
        assert_eq!(whole.glyph_x(99), whole.width());
        let tracked = layout(&f, "one two", &TextStyle { tracking: 0.1, ..style.clone() });
        assert!((tracked.width() - whole.width() - 6.0 * 10.0).abs() < 1e-2);
    }

    #[test]
    fn variable_width_changes_layout_and_features_apply() {
        let f = test_font("RobotoFlex.ttf");
        let style = TextStyle { size: 100.0, ..TextStyle::default() };
        let narrow = layout(&f.with_axes(&[("wdth", 25.0)]), "UPPING", &style).width();
        let wide = layout(&f.with_axes(&[("wdth", 151.0)]), "UPPING", &style).width();
        assert!(wide > narrow * 1.15, "{narrow} -> {wide}");
        let plain = layout(&f, "fi", &style);
        let no_liga = layout(&f, "fi", &TextStyle { features: vec!["-liga".into()], ..style });
        assert!(no_liga.glyphs.len() >= plain.glyphs.len());
    }
}
