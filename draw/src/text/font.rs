use {
    super::{
        font_face::{FontFace, FontFaceDefinition},
        geometry::{Point, Rect, Size},
        image_atlas::ImageAtlas,
        outline,
        outline::Outline,
        pixels::{Bgra, R},
        raster_image::RasterImage,
    },
    makepad_rustybuzz as rustybuzz,
    rustybuzz::ttf_parser,
    std::{
        cell::RefCell,
        hash::{Hash, Hasher},
        rc::Rc,
    },
};

pub type FontId = Rc<str>;

#[derive(Debug)]
pub struct Font {
    id: FontId,
    grayscale_atlas: Rc<RefCell<ImageAtlas<R<u8>>>>,
    color_atlas: Rc<RefCell<ImageAtlas<Bgra<u8>>>>,
    face: FontFace,
}

impl Font {
    pub fn new(
        id: FontId,
        grayscale_atlas: Rc<RefCell<ImageAtlas<R<u8>>>>,
        color_atlas: Rc<RefCell<ImageAtlas<Bgra<u8>>>>,
        face_definition: FontFaceDefinition,
    ) -> Option<Self> {
        Some(Self {
            id,
            grayscale_atlas,
            color_atlas,
            face: FontFace::from_definition(face_definition)?,
        })
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

    pub fn glyph_outline(&self, glyph_id: GlyphId, pxs_per_em: f32) -> Option<Outline> {
        let face = self.ttf_parser_face();
        let glyph_id = ttf_parser::GlyphId(glyph_id);
        let mut builder = outline::Builder::new();
        let bounds = face.outline_glyph(glyph_id, &mut builder)?;
        let min = Point::new(bounds.x_min as f32, bounds.y_min as f32);
        let max = Point::new(bounds.x_max as f32, bounds.y_max as f32);
        Some(builder.finish(Rect::new(min, max - min), pxs_per_em, self.units_per_em()))
    }

    pub fn glyph_raster_image(
        &self,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterImage<'_>> {
        let face = self.ttf_parser_face();
        let glyph_id = ttf_parser::GlyphId(glyph_id);
        let image = face.glyph_raster_image(glyph_id, dpxs_per_em as u16)?;
        RasterImage::from_raster_glyph_image(image)
    }

    pub fn allocate_glyph(&self, glyph_id: GlyphId, dpx_per_em: f32) -> Option<AllocatedGlyph> {
        if let Some(outline) = self.glyph_outline(glyph_id, dpx_per_em) {
            let mut atlas = self.grayscale_atlas.borrow_mut();
            let mut image = atlas.allocate_image(outline.image_size())?;
            outline.rasterize(&mut image);
            let image_bounds = image.bounds();
            return Some(AllocatedGlyph {
                bounds_in_dpxs: outline.bounds_in_pxs(),
                dpxs_per_em: outline.dpxs_per_em(),
                atlas_kind: AtlasKind::Grayscale,
                atlas_size: atlas.size(),
                image_bounds,
            });
        }
        if let Some(raster_image) = self.glyph_raster_image(glyph_id, dpx_per_em) {
            let mut atlas = self.color_atlas.borrow_mut();
            let mut image = atlas.allocate_image(raster_image.size())?;
            raster_image.decode(&mut image);
            let image_bounds = image.bounds();
            return Some(AllocatedGlyph {
                bounds_in_dpxs: raster_image.bounds_in_pxs(),
                dpxs_per_em: raster_image.dpxs_per_em(),
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
    pub bounds_in_dpxs: Rect<f32>,
    pub dpxs_per_em: f32,
    pub atlas_kind: AtlasKind,
    pub atlas_size: Size<usize>,
    pub image_bounds: Rect<usize>,
}

#[derive(Clone, Copy, Debug)]
pub enum AtlasKind {
    Grayscale,
    Color,
}
