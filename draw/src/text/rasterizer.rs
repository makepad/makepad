use super::{
    font::{Font, GlyphId},
    font_atlas::{ColorAtlas, GlyphImage, GlyphImageKey, GrayscaleAtlas},
    geom::{Rect, Size},
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
        let padding = self.sdfer.settings().padding;
        let atlas_size = self.grayscale_atlas.size();
        let dpxs_per_em = if dpxs_per_em < 16.0 {
            16.0
        } else if dpxs_per_em < 64.0 {
            64.0
        } else {
            256.0
        };
        let dpxs_per_em = dpxs_per_em * 2.0;
        let outline = font.glyph_outline(glyph_id)?;
        let coverage_size = outline.image_size(dpxs_per_em);
        let atlas_bounds =
            match self
                .grayscale_atlas
                .get_or_allocate_glyph_image(GlyphImageKey {
                    font_id: font.id(),
                    glyph_id,
                    size: coverage_size + Size::from(self.sdfer.settings().padding) * 2,
                })? {
                GlyphImage::Cached(rect) => rect,
                GlyphImage::Allocated(mut sdf) => {
                    let mut coverage = Image::new(coverage_size);
                    outline.rasterize(
                        dpxs_per_em,
                        &mut coverage.subimage_mut(coverage.size().into()),
                    );
                    self.sdfer
                        .coverage_to_sdf(&coverage.subimage(coverage.size().into()), &mut sdf);
                    sdf.bounds()
                }
            };

        return Some(RasterizedGlyph {
            bounds_in_dpxs: outline.bounds_in_dpxs(dpxs_per_em).pad(padding as f32),
            dpxs_per_em,
            atlas_kind: AtlasKind::Grayscale,
            atlas_bounds,
            atlas_size,
        });
    }

    fn rasterize_glyph_raster_image(
        &mut self,
        font: &Font,
        glyph_id: GlyphId,
        dpxs_per_em: f32,
    ) -> Option<RasterizedGlyph> {
        let raster_image = font.glyph_raster_image(glyph_id, dpxs_per_em)?;
        let atlas_bounds = match self
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
            bounds_in_dpxs: raster_image.bounds_in_pxs(),
            dpxs_per_em: raster_image.dpxs_per_em(),
            atlas_kind: AtlasKind::Color,
            atlas_bounds,
            atlas_size: self.color_atlas.size(),
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
