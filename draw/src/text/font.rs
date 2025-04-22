use {
    super::{
        font_face::FontFace,
        geom::{Point, Rect},
        glyph_outline,
        glyph_outline::GlyphOutline,
        glyph_raster_image::GlyphRasterImage,
        intern::Intern,
        rasterizer::{RasterizedGlyph, Rasterizer},
    },
    makepad_rustybuzz as rustybuzz,
    rustybuzz::ttf_parser,
    std::{
        cell::RefCell,
        collections::HashMap,
        hash::{Hash, Hasher},
        rc::Rc,
    },
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FontId(usize);

impl From<usize> for FontId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<&str> for FontId {
    fn from(value: &str) -> Self {
        Self(value.intern().as_ptr() as usize)
    }
}

#[derive(Debug)]
pub struct Font {
    id: FontId,
    rasterizer: Rc<RefCell<Rasterizer>>,
    face: FontFace,
    ascender_fudge_in_ems: f32,
    descender_fudge_in_ems: f32,
    cached_glyph_outline_bounds_in_ems: RefCell<HashMap<GlyphId, Option<Rect<f32>>>>,
}

impl Font {
    pub fn new(
        id: FontId,
        rasterizer: Rc<RefCell<Rasterizer>>,
        face: FontFace,
        ascender_fudge_in_ems: f32,
        descender_fudge_in_ems: f32,
    ) -> Self {
        Self {
            id,
            rasterizer,
            face,
            ascender_fudge_in_ems,
            descender_fudge_in_ems,
            cached_glyph_outline_bounds_in_ems: RefCell::new(HashMap::new()),
        }
    }

    pub fn id(&self) -> FontId {
        self.id
    }

    pub(super) fn ttf_parser_face(&self) -> &ttf_parser::Face<'_> {
        self.face.as_ttf_parser_face()
    }

    pub(super) fn rustybuzz_face(&self) -> &rustybuzz::Face<'_> {
        self.face.as_rustybuzz_face()
    }

    pub fn units_per_em(&self) -> f32 {
        self.ttf_parser_face().units_per_em() as f32
    }

    pub fn ascender_in_ems(&self) -> f32 {
        self.ttf_parser_face().ascender() as f32 / self.units_per_em() + self.ascender_fudge_in_ems
    }

    pub fn descender_in_ems(&self) -> f32 {
        self.ttf_parser_face().descender() as f32 / self.units_per_em()
            + self.descender_fudge_in_ems
    }

    pub fn line_gap_in_ems(&self) -> f32 {
        self.ttf_parser_face().line_gap() as f32 / self.units_per_em()
    }

    pub fn glyph_outline(&self, glyph_id: GlyphId) -> Option<GlyphOutline> {
        let face = self.ttf_parser_face();
        let glyph_id = ttf_parser::GlyphId(glyph_id);
        let mut builder = glyph_outline::Builder::new();
        let bounds = face.outline_glyph(glyph_id, &mut builder)?;
        let min = Point::new(bounds.x_min as f32, bounds.y_min as f32);
        let max = Point::new(bounds.x_max as f32, bounds.y_max as f32);
        Some(builder.finish(Rect::new(min, max - min), self.units_per_em()))
    }

    pub fn glyph_outline_bounds_in_ems(
        &self,
        glyph_id: GlyphId,
        out_outline: &mut Option<GlyphOutline>,
    ) -> Option<Rect<f32>> {
        if let Some(bounds_in_ems) = self
            .cached_glyph_outline_bounds_in_ems
            .borrow()
            .get(&glyph_id)
        {
            return *bounds_in_ems;
        }
        if let Some(outline) = self.glyph_outline(glyph_id) {
            let bounds_in_ems = outline.bounds_in_ems();
            *out_outline = Some(outline);
            self.cached_glyph_outline_bounds_in_ems
                .borrow_mut()
                .insert(glyph_id, Some(bounds_in_ems));
            Some(bounds_in_ems)
        } else {
            None
        }
    }

    pub fn glyph_raster_image(
        &self,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<GlyphRasterImage<'_>> {
        let face = self.ttf_parser_face();
        let glyph_id = ttf_parser::GlyphId(glyph_id);
        let image = face.glyph_raster_image(glyph_id, dpxs_per_em as u16)?;
        GlyphRasterImage::from_raster_glyph_image(image)
    }

    pub fn rasterize_glyph(&self, glyph_id: GlyphId, dpxs_per_em: f32) -> Option<RasterizedGlyph> {
        self.rasterizer
            .borrow_mut()
            .rasterize_glyph(self, glyph_id, dpxs_per_em)
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
