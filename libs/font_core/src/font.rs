use {
    super::{
        font_face::{FontData, FontFace},
        geom::Rect,
        glyph_outline,
        glyph_outline::GlyphOutline,
        glyph_raster_image::GlyphRasterImage,
        intern::Intern,
    },
    fxhash::FxHashMap,
    rustybuzz,
    rustybuzz::ttf_parser,
    std::{
        cell::RefCell,
        hash::{Hash, Hasher},
        rc::Rc,
    },
};

/// Cap on the per-font glyph-outline cache (distinct glyphs). Large enough that typical Latin
/// usage never reaches it, but bounds CJK/emoji-heavy sessions from growing without limit.
const MAX_CACHED_GLYPH_OUTLINES: usize = 8192;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FontId(u64);

impl From<u64> for FontId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<&str> for FontId {
    fn from(value: &str) -> Self {
        Self(value.intern().as_ptr() as u64)
    }
}

#[derive(Debug)]
pub struct Font {
    id: FontId,
    face: FontFace,
    units_per_em: f32,
    ascender_in_ems: f32,
    descender_in_ems: f32,
    line_gap_in_ems: f32,
    cap_height_in_ems: f32,
    cached_glyph_outlines: RefCell<FxHashMap<GlyphId, Option<Rc<GlyphOutline>>>>,
}

impl Font {
    pub fn new(
        id: FontId,
        face: FontFace,
        ascender_fudge_in_ems: f32,
        descender_fudge_in_ems: f32,
    ) -> Self {
        let (units_per_em, ascender_in_ems, descender_in_ems, line_gap_in_ems, cap_height_in_ems) =
            face.with_ttf_parser_face(|face| {
                let units_per_em = face.units_per_em() as f32;
                (
                    units_per_em,
                    face.ascender() as f32 / units_per_em + ascender_fudge_in_ems,
                    face.descender() as f32 / units_per_em + descender_fudge_in_ems,
                    face.line_gap() as f32 / units_per_em,
                    cap_height_in_units(face) / units_per_em,
                )
            });
        Self {
            id,
            face,
            units_per_em,
            ascender_in_ems,
            descender_in_ems,
            line_gap_in_ems,
            cap_height_in_ems,
            cached_glyph_outlines: RefCell::new(FxHashMap::default()),
        }
    }

    pub fn id(&self) -> FontId {
        self.id
    }

    pub fn data(&self) -> &FontData {
        self.face.data()
    }

    pub fn face(&self) -> &FontFace {
        &self.face
    }

    /// The variation axis values this font is read at (`(tag, value)`).
    pub fn variation_coords(&self) -> Vec<(u32, f32)> {
        self.face.variation_coords()
    }

    pub fn with_ttf_parser_face<R>(&self, f: impl FnOnce(&ttf_parser::Face<'_>) -> R) -> R {
        self.face.with_ttf_parser_face(f)
    }

    pub fn with_rustybuzz_face<R>(&self, f: impl FnOnce(&rustybuzz::Face<'_>) -> R) -> R {
        self.face.with_rustybuzz_face(f)
    }

    /// Same output as `rustybuzz::shape`, with each distinct shape plan compiled once per face.
    pub fn shape(
        &self,
        features: &[rustybuzz::Feature],
        buffer: rustybuzz::UnicodeBuffer,
    ) -> rustybuzz::GlyphBuffer {
        self.face.shape(features, buffer)
    }

    pub fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    pub fn ascender_in_ems(&self) -> f32 {
        self.ascender_in_ems
    }

    pub fn descender_in_ems(&self) -> f32 {
        self.descender_in_ems
    }

    pub fn line_gap_in_ems(&self) -> f32 {
        self.line_gap_in_ems
    }

    /// Height of a flat-topped capital above the baseline.
    ///
    /// This is the ink metric — where the eye puts the top of a line of text —
    /// as opposed to the ascender, which is a line-box metric and reaches
    /// further up. `0.0` when the face has no usable capital (icon fonts, some
    /// CJK faces); callers must treat that as "don't know".
    pub fn cap_height_in_ems(&self) -> f32 {
        self.cap_height_in_ems
    }

    pub fn glyph_outline(&self, glyph_id: GlyphId) -> Option<GlyphOutline> {
        self.glyph_outline_rc(glyph_id)
            .map(|outline| (*outline).clone())
    }

    /// Like [`Self::glyph_outline`], but returns the cache's shared `Rc` so callers
    /// on hot per-frame paths avoid deep-copying the outline's command list.
    pub fn glyph_outline_rc(&self, glyph_id: GlyphId) -> Option<Rc<GlyphOutline>> {
        if let Some(outline) = self.cached_glyph_outlines.borrow().get(&glyph_id) {
            return outline.clone();
        }

        let units_per_em = self.units_per_em;
        let outline = self.with_ttf_parser_face(|face| {
            let glyph_id = ttf_parser::GlyphId(glyph_id);
            let mut builder = glyph_outline::Builder::new();
            face.outline_glyph(glyph_id, &mut builder)?;
            builder.finish(units_per_em).map(Rc::new)
        });

        // Fonts whose outlines live in Apple's proprietary `hvgl` table
        // (PingFang on macOS 26+) parse fine but yield no outlines through
        // ttf_parser; ask CoreText — which decodes hvgl in-OS — instead.
        #[cfg(target_os = "macos")]
        let outline = outline
            .or_else(|| self.face.coretext_glyph_outline(glyph_id, units_per_em).map(Rc::new));

        {
            let mut cache = self.cached_glyph_outlines.borrow_mut();
            // Bound the per-font outline cache. The cap is generous, so this only triggers
            // for scripts with thousands of distinct glyphs (e.g. CJK); clearing simply forces
            // the currently-visible glyphs to be re-extracted from the font face on next use.
            if cache.len() >= MAX_CACHED_GLYPH_OUTLINES {
                cache.clear();
            }
            cache.insert(glyph_id, outline.clone());
        }
        outline
    }

    pub fn glyph_outline_bounds_in_ems(
        &self,
        glyph_id: GlyphId,
        out_outline: &mut Option<Rc<GlyphOutline>>,
    ) -> Option<Rect<f32>> {
        // Check the outline cache first — it stores the full outline,
        // from which we can derive bounds.
        if let Some(cached) = self.cached_glyph_outlines.borrow().get(&glyph_id) {
            *out_outline = cached.clone();
            return cached.as_ref().map(|o| o.bounds_in_ems());
        }

        // Not cached yet — compute via glyph_outline_rc() which will populate the cache.
        if let Some(outline) = self.glyph_outline_rc(glyph_id) {
            let bounds_in_ems = outline.bounds_in_ems();
            *out_outline = Some(outline);
            Some(bounds_in_ems)
        } else {
            None
        }
    }

    pub fn with_glyph_raster_image<R>(
        &self,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
        f: impl FnOnce(GlyphRasterImage<'_>) -> R,
    ) -> Option<R> {
        self.with_ttf_parser_face(|face| {
            let glyph_id = ttf_parser::GlyphId(glyph_id);
            let image = face.glyph_raster_image(glyph_id, dpxs_per_em as u16)?;
            let raster = GlyphRasterImage::from_raster_glyph_image(image)?;
            Some(f(raster))
        })
    }

    pub fn has_glyph_raster_image(&self, glyph_id: GlyphId, dpxs_per_em: f32) -> bool {
        self.with_ttf_parser_face(|face| {
            let glyph_id = ttf_parser::GlyphId(glyph_id);
            face.glyph_raster_image(glyph_id, dpxs_per_em as u16)
                .is_some()
        })
    }
}

impl Eq for Font {}

impl Hash for Font {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl PartialEq for Font {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

pub type GlyphId = u16;

/// Cap height in font units, or `0.0` when the face has none to give.
///
/// `OS/2` carries `sCapHeight` from version 2 on, and that is the metric the
/// designer intended. Older or sloppier faces leave it out or zero it, so fall
/// back to the ink of a capital `H` — flat-topped in every Latin design, which
/// is exactly what the metric means. Anything without either (icon fonts,
/// symbol fonts, some CJK faces) reports `0.0`, and callers keep their hands
/// off the layout rather than guessing.
fn cap_height_in_units(face: &ttf_parser::Face<'_>) -> f32 {
    if let Some(cap_height) = face.capital_height().filter(|value| *value > 0) {
        return cap_height as f32;
    }
    face.glyph_index('H')
        .and_then(|glyph_id| face.glyph_bounding_box(glyph_id))
        .map_or(0.0, |bbox| bbox.y_max.max(0) as f32)
}

