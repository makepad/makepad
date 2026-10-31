//! A font crate's scaled-metrics layer, shaped like the general-ecosystem glyph crates: a `Font`
//! trait, a `ScaledFont<F: Font>` trait whose metrics are default methods over the font's
//! unscaled ones, blanket impls for references, a `PxScaleFont<F>` wrapper and a layout loop that
//! calls `descent`/`ascent`/`h_advance`/`kern` through generics and trait objects. The loop is hot,
//! so these tier up to O3 (tools/run/fixture/font_tier.sh: the O3 snapshot of
//! `ScaledFont::descent` must finish, and the boot bar must not stay on `tier1 locked:`).

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PxScale {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlyphId(pub u16);

pub trait Font {
    fn units_per_em(&self) -> Option<f32>;
    fn ascent_unscaled(&self) -> f32;
    fn descent_unscaled(&self) -> f32;
    fn line_gap_unscaled(&self) -> f32;
    fn glyph_id(&self, c: char) -> GlyphId;
    fn h_advance_unscaled(&self, id: GlyphId) -> f32;
    fn kern_unscaled(&self, first: GlyphId, second: GlyphId) -> f32;

    fn pt_to_px_scale(&self, pt_size: f32) -> Option<PxScale> {
        let upe = self.units_per_em()?;
        let height = self.ascent_unscaled() - self.descent_unscaled();
        let s = pt_size * height / upe;
        Some(PxScale { x: s, y: s })
    }

    fn as_scaled<S: Into<PxScale>>(&self, scale: S) -> PxScaleFont<&'_ Self>
    where
        Self: Sized,
    {
        PxScaleFont { font: self, scale: scale.into() }
    }

    fn into_scaled<S: Into<PxScale>>(self, scale: S) -> PxScaleFont<Self>
    where
        Self: Sized,
    {
        PxScaleFont { font: self, scale: scale.into() }
    }
}

impl<F: Font> Font for &F {
    fn units_per_em(&self) -> Option<f32> { (*self).units_per_em() }
    fn ascent_unscaled(&self) -> f32 { (*self).ascent_unscaled() }
    fn descent_unscaled(&self) -> f32 { (*self).descent_unscaled() }
    fn line_gap_unscaled(&self) -> f32 { (*self).line_gap_unscaled() }
    fn glyph_id(&self, c: char) -> GlyphId { (*self).glyph_id(c) }
    fn h_advance_unscaled(&self, id: GlyphId) -> f32 { (*self).h_advance_unscaled(id) }
    fn kern_unscaled(&self, a: GlyphId, b: GlyphId) -> f32 { (*self).kern_unscaled(a, b) }
}

impl From<f32> for PxScale {
    fn from(s: f32) -> Self { PxScale { x: s, y: s } }
}

pub trait ScaledFont<F: Font> {
    fn scale(&self) -> PxScale;
    fn font(&self) -> &F;

    fn h_scale_factor(&self) -> f32 {
        self.scale().x / self.font().height_unscaled()
    }
    fn v_scale_factor(&self) -> f32 {
        self.scale().y / self.font().height_unscaled()
    }
    fn ascent(&self) -> f32 {
        self.v_scale_factor() * self.font().ascent_unscaled()
    }
    fn descent(&self) -> f32 {
        self.v_scale_factor() * self.font().descent_unscaled()
    }
    fn height(&self) -> f32 {
        self.ascent() - self.descent()
    }
    fn line_gap(&self) -> f32 {
        self.v_scale_factor() * self.font().line_gap_unscaled()
    }
    fn glyph_id(&self, c: char) -> GlyphId {
        self.font().glyph_id(c)
    }
    fn h_advance(&self, id: GlyphId) -> f32 {
        self.h_scale_factor() * self.font().h_advance_unscaled(id)
    }
    fn kern(&self, a: GlyphId, b: GlyphId) -> f32 {
        self.h_scale_factor() * self.font().kern_unscaled(a, b)
    }
}

trait HeightUnscaled {
    fn height_unscaled(&self) -> f32;
}
impl<F: Font + ?Sized> HeightUnscaled for F {
    fn height_unscaled(&self) -> f32 {
        self.ascent_unscaled() - self.descent_unscaled()
    }
}

impl<F: Font, SF: ScaledFont<F>> ScaledFont<F> for &SF {
    fn scale(&self) -> PxScale { (*self).scale() }
    fn font(&self) -> &F { (*self).font() }
}

#[derive(Clone, Copy, Debug)]
pub struct PxScaleFont<F> {
    pub font: F,
    pub scale: PxScale,
}

impl<F: Font> ScaledFont<F> for PxScaleFont<F> {
    fn scale(&self) -> PxScale { self.scale }
    fn font(&self) -> &F { &self.font }
}

/// A font's tables, the way a parsed face keeps them.
pub struct FontVec {
    upe: u16,
    ascent: i16,
    descent: i16,
    gap: i16,
    cmap: HashMap<char, GlyphId>,
    hmtx: Vec<u16>,
    kern: HashMap<(GlyphId, GlyphId), i16>,
}

impl FontVec {
    pub fn synthetic(seed: u32) -> FontVec {
        let mut cmap = HashMap::new();
        let mut hmtx = vec![500u16];
        let mut kern = HashMap::new();
        let mut x = seed | 1;
        for (i, c) in (' '..='~').enumerate() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let id = GlyphId(i as u16 + 1);
            cmap.insert(c, id);
            hmtx.push(400 + (x % 400) as u16);
            if x % 7 == 0 {
                kern.insert((id, GlyphId(((x >> 8) % 95) as u16 + 1)), -((x % 90) as i16));
            }
        }
        FontVec { upe: 2048, ascent: 1900, descent: -500, gap: 67, cmap, hmtx, kern }
    }
}

impl Font for FontVec {
    fn units_per_em(&self) -> Option<f32> { Some(self.upe as f32) }
    fn ascent_unscaled(&self) -> f32 { self.ascent as f32 }
    fn descent_unscaled(&self) -> f32 { self.descent as f32 }
    fn line_gap_unscaled(&self) -> f32 { self.gap as f32 }
    fn glyph_id(&self, c: char) -> GlyphId { self.cmap.get(&c).copied().unwrap_or(GlyphId(0)) }
    fn h_advance_unscaled(&self, id: GlyphId) -> f32 { self.hmtx.get(id.0 as usize).copied().unwrap_or(0) as f32 }
    fn kern_unscaled(&self, a: GlyphId, b: GlyphId) -> f32 { self.kern.get(&(a, b)).copied().unwrap_or(0) as f32 }
}

/// Lays `text` out in lines of `width` px: one glyph position per char.
pub fn layout<F: Font, SF: ScaledFont<F>>(font: SF, text: &str, width: f32, out: &mut Vec<(GlyphId, f32, f32)>) -> f32 {
    let v_advance = font.height() + font.line_gap();
    let mut caret = (0.0f32, font.ascent());
    let mut last: Option<GlyphId> = None;
    for c in text.chars() {
        if c.is_control() {
            if c == '\n' {
                caret = (0.0, caret.1 + v_advance);
                last = None;
            }
            continue;
        }
        let id = font.glyph_id(c);
        if let Some(p) = last.take() {
            caret.0 += font.kern(p, id);
        }
        let adv = font.h_advance(id);
        if !c.is_whitespace() && caret.0 + adv > width {
            caret = (0.0, caret.1 + v_advance);
        }
        out.push((id, caret.0, caret.1));
        caret.0 += adv;
        last = Some(id);
    }
    caret.1 - font.descent()
}

/// The text block's metrics through a trait object (a renderer's dyn font cache).
pub fn block_height(fonts: &[&dyn ScaledFont<FontVec>], lines: usize) -> f32 {
    let mut h = 0.0;
    for f in fonts {
        h += f.ascent() - f.descent() + (lines as f32 - 1.0) * (f.height() + f.line_gap());
    }
    h
}

fn main() {
    let secs: u64 = std::env::var("FONT_TIER_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let fonts: Vec<FontVec> = (0..3).map(|i| FontVec::synthetic(0x9e37_79b9 ^ i)).collect();
    let text = "The quick brown fox jumps over the lazy dog. 0123456789 {}[]()<>!?\nPack my box with five dozen liquor jugs.\n";
    let t0 = std::time::Instant::now();
    let mut out = Vec::new();
    let mut sum = 0.0f64;
    let mut rounds = 0u64;
    while t0.elapsed().as_secs() < secs {
        for (i, f) in fonts.iter().enumerate() {
            let px = 12.0 + i as f32 * 3.0;
            let scaled = f.as_scaled(px);
            out.clear();
            sum += layout(&scaled, text, 240.0 + rounds as f32 % 80.0, &mut out) as f64;
            sum += out.len() as f64;
            let pt = f.pt_to_px_scale(px).unwrap();
            let owned = FontVec::synthetic(7 + i as u32).into_scaled(pt);
            sum += owned.descent() as f64 + scaled.descent() as f64;
            let dyns: [&dyn ScaledFont<FontVec>; 1] = [&owned];
            sum += block_height(&dyns, 3) as f64;
        }
        rounds += 1;
    }
    println!("font_tier: done ({} rounds, finite {})", if rounds > 0 { "some" } else { "none" }, sum.is_finite());
}
