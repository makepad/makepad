use {
    crate::{
        font_atlas::CxFontAtlas,
        font_loader::FontLoader,
        glyph_rasterizer::Params,
        makepad_platform::{math_f64, math_usize::SizeUsize},
        makepad_vector::geometry::{AffineTransformation, Transform, Vector},
    },
    sdfer::esdt::{Params as SdfParams, ReusableBuffers},
    std::fmt,
};

pub struct SdfGlyphRasterizer {
    params: SdfParams,
    buffers: Option<ReusableBuffers>,
}

impl SdfGlyphRasterizer {
    pub fn new() -> Self {
        Self {
            params: SdfParams {
                pad: 4,
                radius: 8.0,
                cutoff: 0.25,
                ..Default::default()
            },
            buffers: None,
        }
    }

    pub fn rasterize_sdf_glyph(
        &mut self,
        font_loader: &mut FontLoader,
        font_atlas: &mut CxFontAtlas,
        Params {
            font_id,
            atlas_page_id,
            glyph_id,
        }: Params,
        output: &mut Vec<u8>,
    ) -> SizeUsize {
        let font = font_loader[font_id].as_mut().unwrap();
        let atlas_page = &font.atlas_pages[atlas_page_id];
        let glyph = font
            .owned_font_face
            .with_ref(|face| font.ttf_font.get_glyph_by_id(face, glyph_id).unwrap());
        let atlas_glyph = atlas_page.atlas_glyphs.get(&glyph_id).unwrap();

        let font_scale_pixels = atlas_page.font_size_in_device_pixels;

        // HACK(eddyb) ideally these values computed by `DrawText::draw_inner`
        // would be kept in each `CxFontsAtlasTodo`, to avoid recomputation here.
        let render_pad_dpx = 2.0;
        let render_wh = math_f64::dvec2(
            ((glyph.bounds.p_max.x - glyph.bounds.p_min.x) * font_scale_pixels).ceil()
                + render_pad_dpx * 2.0,
            ((glyph.bounds.p_max.y - glyph.bounds.p_min.y) * font_scale_pixels).ceil()
                + render_pad_dpx * 2.0,
        );

        // NOTE(eddyb) `+ 1.0` is because the texture coordinate rectangle
        // formed by `t1` and `t2` is *inclusive*, see also the comment in
        // `alloc_atlas_glyph` (about its `- 1` counterpart to this `+ 1.0`).
        let atlas_alloc_wh = math_f64::dvec2(
            (atlas_glyph.t2.x - atlas_glyph.t1.x) as f64 * font_atlas.texture_size.x + 1.0,
            (atlas_glyph.t2.y - atlas_glyph.t1.y) as f64 * font_atlas.texture_size.y + 1.0,
        );

        // HACK(eddyb) because `render_wh` can be larger than the `glyph.bounds`
        // scaled by `font_scale_pixels`, and `alloc_atlas_glyph` performs some
        // non-trivial scaling on `render_wh` to get better SDF quality, this
        // division is required to properly map the glyph outline into atlas
        // space, *without* encroaching into the extra space `render_wh` added.
        let atlas_scaling = atlas_alloc_wh / render_wh;

        let transform = AffineTransformation::identity()
            .translate(Vector::new(-glyph.bounds.p_min.x, -glyph.bounds.p_min.y))
            .uniform_scale(font_scale_pixels)
            .translate(Vector::new(render_pad_dpx, render_pad_dpx))
            .scale(Vector::new(atlas_scaling.x, atlas_scaling.y));
        let commands = glyph
            .outline
            .iter()
            .map(move |command| command.transform(&transform));

        // FIXME(eddyb) try reusing this buffer.
        let mut glyph_rast = sdfer::Image2d::<_, Vec<_>>::new(
            atlas_alloc_wh.x.ceil() as usize,
            atlas_alloc_wh.y.ceil() as usize,
        );

        let mut cur = ab_glyph_rasterizer::point(0.0, 0.0);
        let to_ab =
            |p: makepad_vector::geometry::Point| ab_glyph_rasterizer::point(p.x as f32, p.y as f32);
        commands
            .fold(
                ab_glyph_rasterizer::Rasterizer::new(glyph_rast.width(), glyph_rast.height()),
                |mut rasterizer, cmd| match cmd {
                    makepad_vector::path::PathCommand::MoveTo(p) => {
                        cur = to_ab(p);
                        rasterizer
                    }
                    makepad_vector::path::PathCommand::LineTo(p1) => {
                        let (p0, p1) = (cur, to_ab(p1));
                        rasterizer.draw_line(p0, p1);
                        cur = p1;
                        rasterizer
                    }
                    makepad_vector::path::PathCommand::ArcTo(..) => {
                        unreachable!("font glyphs should not use arcs");
                    }
                    makepad_vector::path::PathCommand::QuadraticTo(p1, p2) => {
                        let (p0, p1, p2) = (cur, to_ab(p1), to_ab(p2));
                        rasterizer.draw_quad(p0, p1, p2);
                        cur = p2;
                        rasterizer
                    }
                    makepad_vector::path::PathCommand::CubicTo(p1, p2, p3) => {
                        let (p0, p1, p2, p3) = (cur, to_ab(p1), to_ab(p2), to_ab(p3));
                        rasterizer.draw_cubic(p0, p1, p2, p3);
                        cur = p3;
                        rasterizer
                    }
                    makepad_vector::path::PathCommand::Close => rasterizer,
                },
            )
            .for_each_pixel_2d(|x, y, a| {
                glyph_rast[(x as usize, y as usize)] = sdfer::Unorm8::encode(a);
            });

        let (glyph_out, new_reuse_bufs) =
            sdfer::esdt::glyph_to_sdf(&mut glyph_rast, self.params, self.buffers.take());
        self.buffers = Some(new_reuse_bufs);
        for y in 0..glyph_out.height() {
            for x in 0..glyph_out.width() {
                output.push(glyph_out[(x, y)].to_bits());
            }
        }

        SizeUsize::new(glyph_out.width(), glyph_out.height())
    }
}

impl fmt::Debug for SdfGlyphRasterizer {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FontRasterizer").finish()
    }
}
