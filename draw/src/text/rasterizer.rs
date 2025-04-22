use super::{
    font::{Font, GlyphId},
    font_atlas::{ColorAtlas, GlyphImage, GlyphImageKey, GrayscaleAtlas},
    geom::{Point, Rect, Size},
    image::{Image, Rgba, Subimage, R},
    sdfer,
    sdfer::Sdfer,
};

#[derive(Debug)]
pub struct Rasterizer {
    sdfer: Sdfer,
    grayscale_atlas: GrayscaleAtlas,
    color_atlas: ColorAtlas,
}

impl Rasterizer {
    pub fn new(settings: Settings) -> Self {
        Self {
            sdfer: Sdfer::new(settings.sdfer),
            grayscale_atlas: GrayscaleAtlas::new(settings.grayscale_atlas_size),
            color_atlas: ColorAtlas::new(settings.grayscale_atlas_size),
        }
    }

    pub fn sdfer_settings(&self) -> sdfer::Settings {
        self.sdfer.settings()
    }

    pub fn grayscale_atlas_size(&self) -> Size<usize> {
        self.grayscale_atlas.size()
    }

    pub fn color_atlas_size(&self) -> Size<usize> {
        self.color_atlas.size()
    }

    pub fn grayscale_atlas_image(&self) -> &Image<R> {
        self.grayscale_atlas.image()
    }

    pub fn color_atlas_image(&self) -> &Image<Rgba> {
        self.color_atlas.image()
    }

    pub fn reset_grayscale_atlas_if_needed(&mut self) -> bool {
        if self.grayscale_atlas.needs_reset() {
            self.grayscale_atlas.reset();
            true
        } else {
            false
        }
    }

    pub fn reset_color_atlas_if_needed(&mut self) -> bool {
        if self.color_atlas.needs_reset() {
            self.color_atlas.reset();
            true
        } else {
            false
        }
    }

    pub fn take_grayscale_atlas_dirty_image(&mut self) -> Subimage<'_, R> {
        self.grayscale_atlas.take_dirty_image()
    }

    pub fn take_color_atlas_dirty_image(&mut self) -> Subimage<'_, Rgba> {
        self.color_atlas.take_dirty_image()
    }

    pub fn rasterize_glyph(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        if let Some(rasterized_glyph) = self.rasterize_glyph_outline(font, glyph_id, dpxs_per_em) {
            return Some(rasterized_glyph);
        };
        if let Some(rasterized_glyph) =
            self.rasterize_glyph_raster_image(font, glyph_id, dpxs_per_em)
        {
            return Some(rasterized_glyph);
        }
        None
    }

    fn rasterize_glyph_outline(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        let dpxs_per_em = if dpxs_per_em < 32.0 { 32.0 } else { 64.0 };
        let dpxs_per_em = dpxs_per_em * 2.0;
        let mut outline = None;
        let bounds_in_ems = font.glyph_outline_bounds_in_ems(glyph_id, &mut outline)?;
        let atlas_image_size = glyph_outline_image_size(bounds_in_ems.size, dpxs_per_em);
        let atlas_image_padding = self.sdfer.settings().padding;
        let atlas_image_bounds =
            match self
                .grayscale_atlas
                .get_or_allocate_glyph_image(GlyphImageKey {
                    font_id: font.id(),
                    glyph_id,
                    size: atlas_image_size + Size::from(self.sdfer.settings().padding) * 2,
                })? {
                GlyphImage::Cached(rect) => rect,
                GlyphImage::Allocated(mut sdf) => {
                    let outline = outline.unwrap_or_else(|| font.glyph_outline(glyph_id).unwrap());
                    let mut coverage = Image::new(atlas_image_size);
                    outline.rasterize(
                        dpxs_per_em,
                        &mut coverage.subimage_mut(atlas_image_size.into()),
                    );
                    self.sdfer
                        .coverage_to_sdf(&coverage.subimage(atlas_image_size.into()), &mut sdf);
                    sdf.bounds()
                }
            };

        return Some(RasterizedGlyph {
            atlas_kind: AtlasKind::Grayscale,
            atlas_size: self.grayscale_atlas_size(),
            atlas_image_bounds,
            atlas_image_padding,
            origin_in_dpxs: bounds_in_ems.origin * dpxs_per_em,
            dpxs_per_em,
        });
    }

    fn rasterize_glyph_raster_image(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        let raster_image = font.glyph_raster_image(glyph_id, dpxs_per_em)?;
        let atlas_image_bounds =
            match self
                .color_atlas
                .get_or_allocate_glyph_image(GlyphImageKey {
                    font_id: font.id(),
                    glyph_id,
                    size: raster_image.decode_size(),
                })? {
                GlyphImage::Cached(rect) => rect,
                GlyphImage::Allocated(mut image) => {
                    raster_image.decode(&mut image);
                    image.bounds()
                }
            };
        return Some(RasterizedGlyph {
            atlas_kind: AtlasKind::Color,
            atlas_size: self.color_atlas.size(),
            atlas_image_bounds,
            atlas_image_padding: 0,
            origin_in_dpxs: raster_image.origin_in_dpxs(),
            dpxs_per_em: raster_image.dpxs_per_em(),
        });
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub sdfer: sdfer::Settings,
    pub grayscale_atlas_size: Size<usize>,
    pub color_atlas_size: Size<usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct RasterizedGlyph {
    pub atlas_kind: AtlasKind,
    pub atlas_size: Size<usize>,
    pub atlas_image_bounds: Rect<usize>,
    pub atlas_image_padding: usize,
    pub origin_in_dpxs: Point<f32>,
    pub dpxs_per_em: f32,
}

#[derive(Clone, Copy, Debug)]
pub enum AtlasKind {
    Grayscale,
    Color,
}

fn glyph_outline_image_size(size_in_ems: Size<f32>, dpxs_per_em: f32) -> Size<usize> {
    let size_in_dpxs = size_in_ems * dpxs_per_em;
    Size::new(
        size_in_dpxs.width.ceil() as usize,
        size_in_dpxs.height.ceil() as usize,
    )
}
