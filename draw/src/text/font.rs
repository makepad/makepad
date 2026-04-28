use {
    super::{
        font_face::FontFace,
        geom::{Point, Rect},
        glyph_outline,
        glyph_outline::GlyphOutline,
        glyph_raster_image::GlyphRasterImage,
        intern::Intern,
        loader::FontData,
        rasterizer::{RasterizedGlyph, Rasterizer},
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
    rasterizer: Rc<RefCell<Rasterizer>>,
    face: FontFace,
    units_per_em: f32,
    ascender_in_ems: f32,
    descender_in_ems: f32,
    line_gap_in_ems: f32,
    cached_glyph_outlines: RefCell<FxHashMap<GlyphId, Option<GlyphOutline>>>,
}

impl Font {
    pub fn new(
        id: FontId,
        rasterizer: Rc<RefCell<Rasterizer>>,
        face: FontFace,
        ascender_fudge_in_ems: f32,
        descender_fudge_in_ems: f32,
    ) -> Self {
        let (units_per_em, ascender_in_ems, descender_in_ems, line_gap_in_ems) = face
            .with_ttf_parser_face(|face| {
                let units_per_em = face.units_per_em() as f32;
                (
                    units_per_em,
                    face.ascender() as f32 / units_per_em + ascender_fudge_in_ems,
                    face.descender() as f32 / units_per_em + descender_fudge_in_ems,
                    face.line_gap() as f32 / units_per_em,
                )
            });
        Self {
            id,
            rasterizer,
            face,
            units_per_em,
            ascender_in_ems,
            descender_in_ems,
            line_gap_in_ems,
            cached_glyph_outlines: RefCell::new(FxHashMap::default()),
        }
    }

    pub fn id(&self) -> FontId {
        self.id
    }

    pub fn data(&self) -> &FontData {
        self.face.data()
    }

    pub(super) fn with_ttf_parser_face<R>(&self, f: impl FnOnce(&ttf_parser::Face<'_>) -> R) -> R {
        self.face.with_ttf_parser_face(f)
    }

    pub(super) fn with_rustybuzz_face<R>(&self, f: impl FnOnce(&rustybuzz::Face<'_>) -> R) -> R {
        self.face.with_rustybuzz_face(f)
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

    pub fn glyph_outline(&self, glyph_id: GlyphId) -> Option<GlyphOutline> {
        if let Some(outline) = self.cached_glyph_outlines.borrow().get(&glyph_id) {
            return outline.clone();
        }

        let units_per_em = self.units_per_em;
        let outline = self.with_ttf_parser_face(|face| {
            let glyph_id = ttf_parser::GlyphId(glyph_id);
            let mut builder = glyph_outline::Builder::new();
            let bounds = face.outline_glyph(glyph_id, &mut builder)?;
            let min = Point::new(bounds.x_min as f32, bounds.y_min as f32);
            let max = Point::new(bounds.x_max as f32, bounds.y_max as f32);
            Some(builder.finish(Rect::new(min, max - min), units_per_em))
        });

        self.cached_glyph_outlines
            .borrow_mut()
            .insert(glyph_id, outline.clone());
        outline
    }

    pub fn glyph_outline_bounds_in_ems(
        &self,
        glyph_id: GlyphId,
        out_outline: &mut Option<GlyphOutline>,
    ) -> Option<Rect<f32>> {
        // Check the outline cache first — it stores the full outline,
        // from which we can derive bounds.
        if let Some(cached) = self.cached_glyph_outlines.borrow().get(&glyph_id) {
            *out_outline = cached.clone();
            return cached.as_ref().map(|o| o.bounds_in_ems());
        }

        // Not cached yet — compute via glyph_outline() which will populate the cache.
        if let Some(outline) = self.glyph_outline(glyph_id) {
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

    pub fn rasterize_glyph(&self, glyph_id: GlyphId, dpxs_per_em: f32) -> Option<RasterizedGlyph> {
        self.rasterizer
            .borrow_mut()
            .rasterize_glyph(self, glyph_id, dpxs_per_em)
    }

    pub fn rasterize_glyph_stable_fallback(
        &self,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        self.rasterizer
            .borrow_mut()
            .rasterize_glyph_stable_fallback(self, glyph_id, dpxs_per_em)
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

#[cfg(test)]
mod tests {
    use super::{Font, FontId};
    use crate::{
        makepad_platform::SharedBytes,
        text::{
            font_face::FontFace,
            layouter,
            loader::FontData,
            rasterizer::{AtlasKind, Rasterizer},
        },
    };
    use std::{cell::RefCell, path::PathBuf, rc::Rc};

    fn bundled_emoji_font_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../widgets/resources/NotoColorEmoji.ttf")
    }

    fn load_font_data(path: PathBuf) -> FontData {
        SharedBytes::from_file_mmap_or_read(path).expect("font bytes should load")
    }

    fn make_font(path: PathBuf) -> Font {
        Font::new(
            FontId::from(0xE0E1_u64),
            Rc::new(RefCell::new(Rasterizer::new(
                layouter::Settings::default().loader.rasterizer,
            ))),
            FontFace::from_data_and_index(load_font_data(path), 0).expect("font face should load"),
            0.0,
            0.0,
        )
    }

    #[test]
    fn noto_color_emoji_prefers_raster_images() {
        let font = make_font(bundled_emoji_font_path());
        let glyph_id = font
            .with_ttf_parser_face(|face| face.glyph_index('😀').map(|glyph| glyph.0))
            .expect("emoji glyph should exist");
        let dpxs_per_em = 128.0;

        assert!(
            font.has_glyph_raster_image(glyph_id, dpxs_per_em),
            "emoji glyph should expose a raster image"
        );

        let rasterized = font
            .rasterize_glyph(glyph_id, dpxs_per_em)
            .expect("emoji glyph should rasterize");
        assert_eq!(rasterized.atlas_kind, AtlasKind::Color);
    }
}
