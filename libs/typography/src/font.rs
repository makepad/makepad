//! A font as geometry reads it: shared bytes, a face index and axis values,
//! `Send + Sync`, parsed on use (no `Cx`, no atlas, no cache), so the
//! typography functions and the kernel host components can run on any
//! worker thread and give the same numbers everywhere.

use rustybuzz::ttf_parser;
use std::sync::Arc;

/// A variation axis of a font.
#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    /// The four-letter tag (`wght`, `wdth`, `slnt`, `opsz`, ...).
    pub tag: String,
    pub min: f32,
    pub default: f32,
    pub max: f32,
}

/// Vertical metrics in font units (y up, baseline 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    pub units_per_em: f32,
    pub ascender: f32,
    pub descender: f32,
    pub line_gap: f32,
    /// Capital height (`OS/2`), or the height of `H` when the table has none.
    pub cap_height: f32,
    /// x-height (`OS/2`), or the height of `x`.
    pub x_height: f32,
}

impl FontMetrics {
    /// Metrics scaled to `size` (the em in output units).
    pub fn scaled(&self, size: f32) -> FontMetrics {
        let k = size / self.units_per_em;
        FontMetrics { units_per_em: size, ascender: self.ascender * k, descender: self.descender * k, line_gap: self.line_gap * k, cap_height: self.cap_height * k, x_height: self.x_height * k }
    }

    /// Baseline-to-baseline distance at this scale.
    pub fn line_height(&self) -> f32 {
        self.ascender - self.descender + self.line_gap
    }
}

/// Font bytes plus the axis values it is read at.
#[derive(Clone)]
pub struct OutlineFont {
    data: Arc<[u8]>,
    index: u32,
    /// (tag, value), clamped to the font's axes; unknown tags are dropped.
    variations: Vec<([u8; 4], f32)>,
}

impl std::fmt::Debug for OutlineFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OutlineFont({} bytes, face {}, {:?})", self.data.len(), self.index, self.variations.iter().map(|(t, v)| (String::from_utf8_lossy(t).to_string(), *v)).collect::<Vec<_>>())
    }
}

impl PartialEq for OutlineFont {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.data, &other.data) && self.index == other.index && self.variations == other.variations
    }
}

/// A four-letter axis tag from text (`"wdth"`), padded with spaces.
pub fn tag_of(name: &str) -> Option<[u8; 4]> {
    let b = name.as_bytes();
    if b.is_empty() || b.len() > 4 || !b.iter().all(|c| c.is_ascii_graphic()) {
        return None;
    }
    let mut t = [b' '; 4];
    t[..b.len()].copy_from_slice(b);
    Some(t)
}

impl OutlineFont {
    /// Checks that the bytes parse as a face.
    pub fn new(data: Arc<[u8]>, index: u32) -> Result<Self, String> {
        ttf_parser::Face::parse(&data, index).map_err(|e| format!("not a font face ({e:?})"))?;
        Ok(Self { data, index, variations: Vec::new() })
    }

    pub fn from_file(path: &std::path::Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::new(bytes.into(), 0)
    }

    /// The font at these axis values (`("wdth", 125.0)`, ...), each clamped
    /// to the font's range; tags the font does not have are ignored (a
    /// static font reads the same at every value).
    pub fn with_axes(&self, axes: &[(&str, f32)]) -> Self {
        let known = self.axes();
        let mut variations = self.variations.clone();
        for (name, value) in axes {
            let Some(tag) = tag_of(name) else { continue };
            let Some(axis) = known.iter().find(|a| tag_of(&a.tag) == Some(tag)) else { continue };
            let v = if value.is_finite() { value.clamp(axis.min, axis.max) } else { axis.default };
            match variations.iter_mut().find(|(t, _)| *t == tag) {
                Some(slot) => slot.1 = v,
                None => variations.push((tag, v)),
            }
        }
        variations.sort_by(|a, b| a.0.cmp(&b.0));
        Self { data: self.data.clone(), index: self.index, variations }
    }

    /// The axis values this font is read at, as `(tag, value)` pairs.
    pub fn variations(&self) -> Vec<(u32, f32)> {
        self.variations.iter().map(|(t, v)| (u32::from_be_bytes(*t), *v)).collect()
    }

    pub fn data(&self) -> &Arc<[u8]> {
        &self.data
    }

    pub fn index(&self) -> u32 {
        self.index
    }

    /// The font's variation axes (empty for a static font).
    pub fn axes(&self) -> Vec<Axis> {
        let Ok(face) = ttf_parser::Face::parse(&self.data, self.index) else { return Vec::new() };
        face.variation_axes()
            .into_iter()
            .map(|a| Axis { tag: String::from_utf8_lossy(&a.tag.to_bytes()).trim_end().to_string(), min: a.min_value, default: a.def_value, max: a.max_value })
            .collect()
    }

    /// Run `f` on the shaping face at this font's axis values.
    pub fn with_face<R>(&self, f: impl FnOnce(&rustybuzz::Face) -> R) -> Option<R> {
        let mut face = rustybuzz::Face::from_slice(&self.data, self.index)?;
        for (tag, value) in &self.variations {
            face.set_variation(ttf_parser::Tag::from_bytes(tag), *value);
        }
        Some(f(&face))
    }

    pub fn metrics(&self) -> FontMetrics {
        self.with_face(|face| {
            let ink_top = |c: char| face.glyph_index(c).and_then(|g| face.glyph_bounding_box(g)).map(|r| r.y_max as f32);
            let upem = face.units_per_em() as f32;
            FontMetrics {
                units_per_em: upem,
                ascender: face.ascender() as f32,
                descender: face.descender() as f32,
                line_gap: face.line_gap() as f32,
                cap_height: face.capital_height().filter(|h| *h > 0).map(|h| h as f32).or_else(|| ink_top('H')).unwrap_or(upem * 0.7),
                x_height: face.x_height().filter(|h| *h > 0).map(|h| h as f32).or_else(|| ink_top('x')).unwrap_or(upem * 0.5),
            }
        })
        .unwrap_or(FontMetrics { units_per_em: 1000.0, ascender: 800.0, descender: -200.0, line_gap: 0.0, cap_height: 700.0, x_height: 500.0 })
    }
}

/// Makepad's bundled fonts directory (`widgets/resources`).
pub fn bundled_fonts_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../widgets/resources")
}

#[cfg(test)]
pub(crate) fn test_font(file: &str) -> OutlineFont {
    OutlineFont::from_file(&bundled_fonts_dir().join(file)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roboto_flex_axes_clamp_and_change_the_face() {
        let f = test_font("RobotoFlex.ttf");
        let axes = f.axes();
        let wdth = axes.iter().find(|a| a.tag == "wdth").expect("Roboto Flex has wdth");
        assert!(wdth.min <= 25.0 && wdth.max >= 151.0, "{wdth:?}");
        let wide = f.with_axes(&[("wdth", 500.0), ("nope", 3.0)]);
        assert_eq!(wide.variations(), vec![(u32::from_be_bytes(*b"wdth"), wdth.max)]);
        let adv = |f: &OutlineFont| f.with_face(|face| face.glyph_hor_advance(face.glyph_index('H').unwrap()).unwrap()).unwrap();
        let narrow = f.with_axes(&[("wdth", 25.0)]);
        assert!(adv(&wide) > adv(&narrow), "wdth changes advances: {} vs {}", adv(&wide), adv(&narrow));
        let m = f.metrics();
        assert!(m.cap_height > m.x_height && m.x_height > 0.0);
    }
}
