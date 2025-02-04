use {
    super::{
        atlas::Atlas,
        faces::Faces,
        geometry::{Point, Rect, Size},
        outline,
        outline::Outline,
        pixels::{Bgra, R},
        raster_image::GlyphRasterImage,
    },
    makepad_rustybuzz as rustybuzz,
    rustybuzz::ttf_parser,
    std::{
        cell::RefCell,
        hash::{Hash, Hasher},
        rc::Rc,
    },
};

pub type FontId = String;

#[derive(Debug)]
pub struct Font {
    id: FontId,
    grayscale_atlas: Rc<RefCell<Atlas<R<u8>>>>,
    color_atlas: Rc<RefCell<Atlas<Bgra<u8>>>>,
    faces: Faces,
}

impl Font {
    pub fn new(
        id: FontId,
        grayscale_atlas: Rc<RefCell<Atlas<R<u8>>>>,
        color_atlas: Rc<RefCell<Atlas<Bgra<u8>>>>,
        faces: Faces,
    ) -> Self {
        Self {
            id,
            grayscale_atlas,
            color_atlas,
            faces,
        }
    }

    pub fn font_id(&self) -> &FontId {
        &self.id
    }

    pub(super) fn ttf_parser_face(&self) -> &ttf_parser::Face<'_> {
        self.faces.ttf_parser_face()
    }

    pub(super) fn rustybuzz_face(&self) -> &rustybuzz::Face<'_> {
        self.faces.rustybuzz_face()
    }

    pub fn units_per_em(&self) -> f32 {
        self.ttf_parser_face().units_per_em() as f32
    }

    pub fn glyph_outline(&self, id: GlyphId, pxs_per_em: f32) -> Option<Outline> {
        let face = self.ttf_parser_face();
        let glyph_id = ttf_parser::GlyphId(id);
        let mut builder = outline::Builder::new();
        let bounds = face.outline_glyph(glyph_id, &mut builder)?;
        let min = Point::new(bounds.x_min as f32, bounds.y_min as f32);
        let max = Point::new(bounds.x_max as f32, bounds.y_max as f32);
        Some(builder.finish(Rect::new(min, max - min), pxs_per_em, self.units_per_em()))
    }

    pub fn glyph_raster_image(&self, id: GlyphId, pxs_per_em: f32) -> Option<GlyphRasterImage<'_>> {
        let face = self.ttf_parser_face();
        let glyph_id = ttf_parser::GlyphId(id);
        let image = face.glyph_raster_image(glyph_id, pxs_per_em as u16)?;
        GlyphRasterImage::from_raster_glyph_image(image)
    }

    pub fn allocate_glyph(&self, id: GlyphId, pxs_per_em: f32) -> Option<AllocatedGlyph> {
        if let Some(outline) = self.glyph_outline(id, pxs_per_em) {
            let mut atlas = self.grayscale_atlas.borrow_mut();
            let mut image = atlas.allocate_image(outline.image_size())?;
            outline.rasterize(&mut image);
            let image_bounds = image.bounds();
            return Some(AllocatedGlyph {
                bounds_in_pxs: outline.bounds_in_pxs(),
                pxs_per_em: outline.pxs_per_em(),
                atlas_kind: AtlasKind::Grayscale,
                atlas_size: atlas.size(),
                image_bounds,
            });
        }
        if let Some(raster_image) = self.glyph_raster_image(id, pxs_per_em) {
            let mut atlas = self.color_atlas.borrow_mut();
            let mut image = atlas.allocate_image(raster_image.size())?;
            raster_image.decode(&mut image);
            let image_bounds = image.bounds();
            return Some(AllocatedGlyph {
                bounds_in_pxs: raster_image.bounds_in_pxs(),
                pxs_per_em: raster_image.pxs_per_em(),
                atlas_kind: AtlasKind::Color,
                atlas_size: atlas.size(),
                image_bounds,
            });
        }
        None
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
pub struct AllocatedGlyph {
    pub bounds_in_pxs: Rect<f32>,
    pub pxs_per_em: f32,
    pub atlas_kind: AtlasKind,
    pub atlas_size: Size<usize>,
    pub image_bounds: Rect<usize>,
}

#[derive(Clone, Copy, Debug)]
pub enum AtlasKind {
    Grayscale,
    Color,
}
