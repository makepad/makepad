use {
    crate::{
        font_atlas::CxFontAtlas,
        font_loader::FontLoader,
        glyph_rasterizer::Params,
    },
    makepad_platform::math_usize::SizeUsize,
};

#[derive(Debug)]
pub struct SvgGlyphRasterizer {}

impl SvgGlyphRasterizer {
    pub fn new() -> Self {
        Self {}
    }

    pub fn rasterize_svg_glyph(
        &mut self,
        _font_loader: &mut FontLoader,
        _font_atlas: &mut CxFontAtlas,
        _params: Params,
        _output: &mut Vec<u8>,
    ) -> SizeUsize {
        unimplemented!()
    }
}
