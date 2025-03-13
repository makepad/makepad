use {
    super::{
        font_atlas::{FontAtlas, GlyphImageKey},
        font_face::FontFace,
        geom::{Rect, Size},
        glyph_outline::GlyphOutline,
        glyph_raster_image::GlyphRasterImage,
        image::{Rgba, R},
        intern::Intern,
        sdfer::Sdfer,
    },
    makepad_rustybuzz as rustybuzz,
    rustybuzz::ttf_parser,
    std::{
        cell::RefCell,
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
    sdfer: Rc<RefCell<Sdfer>>,
    grayscale_atlas: Rc<RefCell<FontAtlas<R>>>,
    color_atlas: Rc<RefCell<FontAtlas<Rgba>>>,
    face: FontFace,
}

impl Font {
    pub fn new(
        id: FontId,
        sdfer: Rc<RefCell<Sdfer>>,
        grayscale_atlas: Rc<RefCell<FontAtlas<R>>>,
        color_atlas: Rc<RefCell<FontAtlas<Rgba>>>,
        face: FontFace,
    ) -> Self {
        Self {
            id,
            grayscale_atlas,
            color_atlas,
            sdfer,
            face,
        }
    }

    pub fn id(&self) -> &FontId {
        &self.id
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
        self.ttf_parser_face().ascender() as f32 / self.units_per_em()
    }

    pub fn descender_in_ems(&self) -> f32 {
        self.ttf_parser_face().descender() as f32 / self.units_per_em()
    }

    pub fn line_gap_in_ems(&self) -> f32 {
        self.ttf_parser_face().line_gap() as f32 / self.units_per_em()
    }

    pub fn glyph_outline(&self, glyph_id: GlyphId) -> Option<GlyphOutline> {
        use super::{geom::Point, glyph_outline};

        let face = self.ttf_parser_face();
        let glyph_id = ttf_parser::GlyphId(glyph_id);
        let mut builder = glyph_outline::Builder::new();
        let bounds = face.outline_glyph(glyph_id, &mut builder)?;
        let min = Point::new(bounds.x_min as f32, bounds.y_min as f32);
        let max = Point::new(bounds.x_max as f32, bounds.y_max as f32);
        Some(builder.finish(Rect::new(min, max - min), self.units_per_em()))
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
        if let Some(rasterized_glyph) = self.rasterize_glyph_outline(glyph_id, dpxs_per_em) {
            return Some(rasterized_glyph);
        };
        if let Some(rasterized_glyph) = self.rasterize_glyph_raster_image(glyph_id, dpxs_per_em) {
            return Some(rasterized_glyph);
        }
        None
    }

    fn rasterize_glyph_outline(
        &self,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        use super::image::Image;

        let outline = self.glyph_outline(glyph_id)?;
        let size_in_dpxs = outline.size_in_dpxs(dpxs_per_em);
        let largest_side_in_dpxs = size_in_dpxs.width.max(size_in_dpxs.height);
        let largest_size_in_dpxs_rounded_up =
            ((largest_side_in_dpxs).ceil() as usize).next_power_of_two() as f32;
        let scale = 2.0 * largest_size_in_dpxs_rounded_up / largest_side_in_dpxs;
        let dpxs_per_em = dpxs_per_em * scale;

        let mut coverage = Image::new(outline.image_size(dpxs_per_em));
        outline.rasterize(
            dpxs_per_em,
            &mut coverage.subimage_mut(coverage.size().into()),
        );

        let mut sdfer = self.sdfer.borrow_mut();
        let padding = sdfer.settings().padding;
        let mut atlas = self.grayscale_atlas.borrow_mut();
        let atlas_size = atlas.size();
        let mut sdf = atlas.get_or_allocate_glyph_image(GlyphImageKey {
            font_id: self.id.clone(),
            glyph_id,
            size: coverage.size() + Size::from(padding) * 2,
        })?;
        let atlas_bounds = sdf.bounds();
        sdfer.coverage_to_sdf(&coverage.subimage(coverage.size().into()), &mut sdf);
        drop(sdf);
        drop(atlas);
        drop(sdfer);

        return Some(RasterizedGlyph {
            bounds_in_dpxs: outline.bounds_in_dpxs(dpxs_per_em).pad(padding as f32),
            dpxs_per_em,
            atlas_kind: AtlasKind::Grayscale,
            atlas_bounds,
            atlas_size,
        });
    }

    fn rasterize_glyph_raster_image(
        &self,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        let raster_image = self.glyph_raster_image(glyph_id, dpxs_per_em)?;
        let mut atlas = self.color_atlas.borrow_mut();
        let mut image = atlas.get_or_allocate_glyph_image(GlyphImageKey {
            font_id: self.id.clone(),
            glyph_id,
            size: raster_image.decode_size(),
        })?;
        raster_image.decode(&mut image);
        let atlas_bounds = image.bounds();
        return Some(RasterizedGlyph {
            bounds_in_dpxs: raster_image.bounds_in_pxs(),
            dpxs_per_em: raster_image.dpxs_per_em(),
            atlas_kind: AtlasKind::Color,
            atlas_bounds,
            atlas_size: atlas.size(),
        });
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

#[derive(Clone, Copy, Debug)]
pub struct RasterizedGlyph {
    pub bounds_in_dpxs: Rect<f32>,
    pub dpxs_per_em: f32,
    pub atlas_kind: AtlasKind,
    pub atlas_bounds: Rect<usize>,
    pub atlas_size: Size<usize>,
}

#[derive(Clone, Copy, Debug)]
pub enum AtlasKind {
    Grayscale,
    Color,
}
